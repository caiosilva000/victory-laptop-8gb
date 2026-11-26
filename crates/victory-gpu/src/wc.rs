//! `VICTORY_SKIP_WC_BACK=1`: skip copying shader writes back from card copies
//! of write-combining (WC) title memory whose copy size is at least
//! `VICTORY_SKIP_WC_BACK_MIN_KB` KiB (default 1024 = 1 MiB).
//!
//! Ported from `fxceless/victory-fork` PR #868 (`VICTORY_SKIP_WC_BACK` only;
//! `VICTORY_SKIP_WC_COMPARE` is intentionally omitted) with the `>= 1 MiB`
//! copy-size rule from `fxceless/victory-fork` issue #1379: small WC buffers
//! hold counts and visibility lists that the title's CPU does read.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Console `PAGE_WRITECOMBINE` protection bit stripped by `xmem`.
const PAGE_WRITECOMBINE: u32 = 0x400;

/// Default minimum copy size in KiB (1 MiB) eligible for skipping WC copy-back.
const DEFAULT_MIN_KB: u64 = 1024;

/// A reservation of `XMemVirtualAlloc`, with the console protection the title
/// asked for before `xmem` strips write-combining / uncached / GPU bits.
struct Reservation {
    len: usize,
    protect: u32,
}

static RESERVATIONS: Mutex<BTreeMap<usize, Reservation>> = Mutex::new(BTreeMap::new());

/// Per-window counters: `[skipped_copies, skipped_bytes, kept_copies, kept_bytes]`.
static STATS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// Whether `VICTORY_SKIP_WC_BACK=1` is enabled.
pub(crate) fn skip_wc_back() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("VICTORY_SKIP_WC_BACK").is_some_and(|v| v == "1");
        if on {
            let min_kb = skip_wc_back_min_kb();
            tracing::warn!(
                min_kb,
                "VICTORY_SKIP_WC_BACK=1: shader writes to write-combining title memory (>= {min_kb} KB copies) are NOT copied back"
            );
        }
        on
    })
}

/// Minimum card-copy size in KiB for `VICTORY_SKIP_WC_BACK=1`
/// (`VICTORY_SKIP_WC_BACK_MIN_KB`, default 1024).
pub(crate) fn skip_wc_back_min_kb() -> u64 {
    static MIN_KB: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *MIN_KB.get_or_init(|| {
        std::env::var("VICTORY_SKIP_WC_BACK_MIN_KB")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_MIN_KB)
    })
}

/// The runtime reserved `[at, at + len)` for the title (`XMemVirtualAlloc` at
/// an address of its choice) with the title's original console `protect`.
pub fn title_memory_reserved(at: usize, len: usize, protect: u32) {
    if !skip_wc_back() || at == 0 || len == 0 {
        return;
    }
    RESERVATIONS.lock().unwrap_or_else(|e| e.into_inner()).insert(at, Reservation { len, protect });
}

/// The title released the reservation starting at `at` (`VirtualFree` with
/// `MEM_RELEASE`).
pub fn title_memory_released(at: usize) {
    if !skip_wc_back() || at == 0 {
        return;
    }
    RESERVATIONS.lock().unwrap_or_else(|e| e.into_inner()).remove(&at);
}

/// Every declared title range must lie wholly inside a single WC reservation.
/// A card copy (especially an aligned loose window) can cover several.
fn all_wc_in(reservations: &BTreeMap<usize, Reservation>, ranges: &[(usize, usize)]) -> bool {
    !ranges.is_empty()
        && ranges.iter().all(|&(from, to)| {
            // Seeking from the exclusive end also detects a new reservation
            // beginning inside the range. An allocation ending at `to` fits.
            from < to
                && reservations.range(..to).next_back().is_some_and(|(&base, r)| {
                    base <= from
                        && r.protect & PAGE_WRITECOMBINE != 0
                        && base.checked_add(r.len).is_some_and(|end| to <= end)
                })
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    NotWc,
    KeptUnderMin,
    Skip,
}

fn decide(
    reservations: &BTreeMap<usize, Reservation>,
    declared: (usize, usize),
    copy_len: u64,
    min_bytes: u64,
) -> Decision {
    if !all_wc_in(reservations, &[declared]) {
        return Decision::NotWc;
    }
    if copy_len >= min_bytes {
        Decision::Skip
    } else {
        Decision::KeptUnderMin
    }
}

/// Returns `true` if a flagged card piece of `bytes` belonging to a card copy
/// of `copy_len` bytes with declared title range `declared` should NOT be
/// copied back to title RAM.
///
/// A flagged piece is clipped to its declaration. Mixed declarations or ranges
/// crossing a reservation boundary always return `false` so the normal
/// read-back covers them in full.
pub(crate) fn skip_back(declared: (usize, usize), copy_len: u64, bytes: u64) -> bool {
    if !skip_wc_back() {
        return false;
    }
    let min_bytes = skip_wc_back_min_kb().saturating_mul(1024);
    let decision = {
        let reservations = RESERVATIONS.lock().unwrap_or_else(|e| e.into_inner());
        decide(&reservations, declared, copy_len, min_bytes)
    };
    match decision {
        Decision::Skip => {
            STATS[0].fetch_add(1, Ordering::Relaxed);
            STATS[1].fetch_add(bytes, Ordering::Relaxed);
            true
        }
        Decision::KeptUnderMin => {
            STATS[2].fetch_add(1, Ordering::Relaxed);
            STATS[3].fetch_add(bytes, Ordering::Relaxed);
            false
        }
        Decision::NotWc => false,
    }
}

/// Takes `[skipped_copies, skipped_bytes, kept_copies, kept_bytes]` since the
/// last 5-second frame summary.
pub(crate) fn take_stats() -> [u64; 4] {
    [0, 1, 2, 3].map(|i| STATS[i].swap(0, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::{Decision, PAGE_WRITECOMBINE, Reservation, all_wc_in, decide};
    use std::collections::BTreeMap;

    fn reservation(len: usize, protect: u32) -> Reservation {
        Reservation { len, protect }
    }

    #[test]
    fn a_write_in_the_ordinary_half_of_a_card_window_still_comes_back() {
        const MIB: usize = 1 << 20;
        let window = 8 * MIB;
        let middle = window + 2 * MIB;
        let map = BTreeMap::from([
            (window, reservation(2 * MIB, 0x04 | PAGE_WRITECOMBINE)),
            (middle, reservation(2 * MIB, 0x04)),
        ]);
        let wc = (middle - 65536, middle);
        let ordinary = (middle, middle + 65536);
        assert!(all_wc_in(&map, &[wc]));
        assert!(!all_wc_in(&map, &[ordinary]));
        assert!(!all_wc_in(&map, &[wc, ordinary]));
        assert!(!all_wc_in(&map, &[(wc.0, ordinary.1)]));
    }

    #[test]
    fn a_declaration_must_fit_one_known_reservation() {
        let map = BTreeMap::from([
            (0x10000, reservation(0x10000, 0x04 | PAGE_WRITECOMBINE)),
            (0x20000, reservation(0x10000, 0x04 | PAGE_WRITECOMBINE)),
            (usize::MAX - 8, reservation(16, 0x04 | PAGE_WRITECOMBINE)),
        ]);
        assert!(all_wc_in(&map, &[(0x10000, 0x20000), (0x20000, 0x30000)]));
        assert!(!all_wc_in(&map, &[(0x1ffff, 0x20001)]));
        assert!(!all_wc_in(&map, &[(0x30000, 0x30001)]));
        assert!(!all_wc_in(&map, &[(0x10000, 0x10000)]));
        assert!(!all_wc_in(&map, &[(usize::MAX - 8, 1)]));
        assert!(!all_wc_in(&map, &[(usize::MAX - 8, usize::MAX)]));
    }

    #[test]
    fn wc_skip_respects_min_copy_size() {
        const MIB: u64 = 1 << 20;
        let map = BTreeMap::from([
            (0x1000_0000, reservation(4 * MIB as usize, 0x04 | PAGE_WRITECOMBINE)),
            (0x2000_0000, reservation(4 * MIB as usize, 0x04)),
        ]);
        let wc_decl = (0x1000_0000, 0x1001_0000);
        let ord_decl = (0x2000_0000, 0x2001_0000);

        // Under 1 MiB copy in WC memory is kept (issue #1379).
        assert_eq!(decide(&map, wc_decl, 64 << 10, MIB), Decision::KeptUnderMin);
        assert_eq!(decide(&map, wc_decl, MIB - 1, MIB), Decision::KeptUnderMin);
        // 1 MiB and above in WC memory is skipped.
        assert_eq!(decide(&map, wc_decl, MIB, MIB), Decision::Skip);
        assert_eq!(decide(&map, wc_decl, 4 * MIB, MIB), Decision::Skip);
        // Ordinary memory is never WC regardless of size.
        assert_eq!(decide(&map, ord_decl, 4 * MIB, MIB), Decision::NotWc);
        // Straddling reservation boundary is not WC.
        assert_eq!(
            decide(&map, (0x1000_0000, 0x1040_0001), 8 * MIB, MIB),
            Decision::NotWc
        );
    }
}
