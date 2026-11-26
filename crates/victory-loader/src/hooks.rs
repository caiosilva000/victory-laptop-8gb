//! Opt-in compatibility hooks for known title builds.
//!
//! Ported from `trustno1-coder/VIctory-NVIDIA` commit `afd84de1` (originally
//! `vitutu1/victory-perf` / `ZigZagBoi1/ScarlettPort`, GPL-3.0-or-later).
//! Only per-build offsets and short byte signatures live here. Every hook
//! checks the 1.0.2.1 executable BLAKE3 hash, image bounds, and exact expected
//! bytes before writing in-process memory, and skips safely otherwise.

use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// First 16 hex digits of the BLAKE3 of the Series X 1.0.2.1 executable.
const BUILD_1_0_2_1: &str = "b99f876b74d4a77f";

/// Environment variable that turns the watchdog hook on (off by default).
pub(crate) const NO_WATCHDOG_ENV: &str = "VICTORY_NO_WATCHDOG";

/// Environment variable that pins the title's internal render scale
/// (`0.2..=1.0` of the output size). Off by default.
pub(crate) const RENDER_SCALE_ENV: &str = "VICTORY_RENDER_SCALE";

/// Environment variable that makes the title use its own temporal upscaler
/// on every console profile (Series S otherwise picks FSR). Off by default.
pub(crate) const OWN_UPSCALER_ENV: &str = "VICTORY_OWN_UPSCALER";

/// One code patch: `rva` must hold exactly `expect` before `new` is written.
struct Patch<'a> {
    what: &'static str,
    rva: usize,
    expect: &'a [u8],
    new: &'a [u8],
}

/// A patch whose bytes are computed at load time.
struct OwnedPatch {
    what: &'static str,
    rva: usize,
    expect: Vec<u8>,
    new: Vec<u8>,
}

/// Outcome of attempting to apply one in-process memory patch.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PatchOutcome {
    Applied {
        what: &'static str,
        rva: usize,
        old_len: usize,
        new_len: usize,
    },
    Skipped {
        what: &'static str,
        rva: usize,
        reason: &'static str,
    },
}

impl PatchOutcome {
    fn format_entry(&self) -> String {
        match self {
            Self::Applied {
                what,
                rva,
                old_len,
                new_len,
            } => format!("{what}@{rva:#x}({old_len}->{new_len}B)"),
            Self::Skipped { what, rva, reason } => format!("{what}@{rva:#x}({reason})"),
        }
    }
}

/// The title's watchdog thread ends the process on purpose when its main
/// thread has not advanced for about 30 one-second ticks. Two places do it:
/// the handler it calls (`mov dword ptr [0], imm32`) and, when the handler
/// returns or is unset, a fallback right after the call (`mov dword ptr [0],
/// 0; int3`). Jumping over the whole block returns the loop to its normal
/// path; the second patch makes the handler harmless if something else calls it.
const WATCHDOG: &[Patch<'static>] = &[
    Patch {
        what: "watchdog_skip",
        rva: 0x2215c27,
        // mov rax, [rip + disp32] (the handler pointer)
        expect: &[0x48, 0x8B, 0x05, 0x1A, 0x07, 0xFA, 0x04],
        // jmp +0x18 (to the loop's normal continuation), then NOPs
        new: &[0xEB, 0x18, 0x90, 0x90, 0x90, 0x90, 0x90],
    },
    Patch {
        what: "watchdog_handler",
        rva: 0x23cf683,
        expect: &[0xC7, 0x04, 0x25, 0, 0, 0, 0, 0x21, 0xD4, 0x8B, 0x4D],
        // an eleven-byte NOP
        new: &[0x66, 0x66, 0x2E, 0x0F, 0x1F, 0x84, 0, 0, 0, 0, 0],
    },
];

/// The upscaler choice: `mov eax, 6` (own upscaler) then `cmovne eax, ecx`
/// (FSR when the profile asks for it). NOPing the cmovne keeps 6.
const OWN_UPSCALER: Patch<'static> = Patch {
    what: "own_upscaler",
    rva: 0x7e9426,
    expect: &[0x0F, 0x45, 0xC1],
    new: &[0x0F, 0x1F, 0x00],
};

/// Read-only float constants of the 1.0.2.1 build usable as the FSR scale:
/// `(value, rva)`.
const FSR_SCALES: [(f32, usize); 4] = [
    (0.5, 0x5fc6edc),
    (0.4, 0x5fc6f90),
    (1.0 / 3.0, 0x615a240),
    (0.75, 0x6159b04),
];

fn env_flag_enabled(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| v != "0" && !v.is_empty())
}

/// The scale `VICTORY_RENDER_SCALE` asks for, if valid (`0.2..=1.0`).
fn parse_scale(text: &str) -> Option<f32> {
    let v: f32 = text.trim().parse().ok()?;
    (v.is_finite() && (0.2..=1.0).contains(&v)).then_some(v)
}

/// The render-scale patches for `scale`, with the image at `base`:
/// - the own upscaler's minimum scale (an immediate, 2/3 by default) set to
///   `scale` and its maximum made equal to the minimum (`vmaxss` -> `vmovss`),
///   so the scale is fixed;
/// - FSR's scale (a load of 2/3 on Series S) pointed at a constant equal to
///   `scale`, when this build has one, and its maximum size made the scaled
///   size (vtable slot 11 -> slot 10's function).
///
/// Returns `(patches, optional_skipped_note)` when FSR has no matching constant.
fn render_scale_patches(base: usize, scale: f32, fsr: bool) -> (Vec<OwnedPatch>, Option<String>) {
    let mut patches = vec![
        OwnedPatch {
            what: "own_upscaler_min",
            rva: 0x28f1e7e,
            expect: vec![0xAB, 0xAA, 0x2A, 0x3F],
            new: scale.to_le_bytes().to_vec(),
        },
        OwnedPatch {
            what: "own_upscaler_max",
            rva: 0x28f1e87,
            expect: vec![0xC5, 0xFA, 0x5F, 0x86, 0x90, 0, 0, 0],
            new: vec![0xC5, 0xFA, 0x10, 0x86, 0x90, 0, 0, 0],
        },
    ];
    if !fsr {
        return (patches, None);
    }
    match FSR_SCALES.iter().find(|(v, _)| (v - scale).abs() < 1e-3) {
        Some(&(_, constant)) => {
            // vmovss xmm0, [rip + disp32]; the next instruction is at +8.
            const LOAD: usize = 0x28ef6b2;
            let disp = (constant as i64 - (LOAD as i64 + 8)) as i32;
            let mut new = vec![0xC5, 0xFA, 0x10, 0x05];
            new.extend_from_slice(&disp.to_le_bytes());
            patches.push(OwnedPatch {
                what: "fsr_scale",
                rva: LOAD,
                expect: vec![0xC5, 0xFA, 0x10, 0x05, 0xA2, 0xE2, 0x86, 0x03],
                new,
            });
            patches.push(OwnedPatch {
                what: "fsr_max",
                rva: 0x6300ba8,
                expect: ((base + 0x28ef6f0) as u64).to_le_bytes().to_vec(),
                new: ((base + 0x28ef680) as u64).to_le_bytes().to_vec(),
            });
            (patches, None)
        }
        None => (
            patches,
            Some(format!(
                "fsr_scale(no FSR constant for scale {scale}; use {OWN_UPSCALER_ENV}=1)"
            )),
        ),
    }
}

/// Applies the watchdog patches when `VICTORY_NO_WATCHDOG` is enabled.
/// Returns how many patches were applied.
///
/// # Safety
/// `base` is the mapped image of size `image_size` whose original BLAKE3 is
/// `hash`, and no title code is running yet.
pub(crate) unsafe fn disarm_watchdog(
    base: usize,
    image_size: usize,
    hash: &blake3::Hash,
    is_dll: bool,
) -> usize {
    if !env_flag_enabled(NO_WATCHDOG_ENV) {
        return 0;
    }
    if !hash.to_hex().starts_with(BUILD_1_0_2_1) {
        if !is_dll {
            let got = &hash.to_hex()[..16];
            tracing::info!(
                applied = "none",
                skipped = %format!("all(exe hash mismatch: got {got}, expected 1.0.2.1 {BUILD_1_0_2_1})"),
                "title watchdog hooks ({NO_WATCHDOG_ENV})"
            );
        }
        return 0;
    }
    let mut applied = Vec::new();
    let mut skipped = Vec::new();
    for p in WATCHDOG {
        // SAFETY: forwarded from this function's contract.
        match unsafe { apply(base, image_size, p) } {
            o @ PatchOutcome::Applied { .. } => applied.push(o.format_entry()),
            o @ PatchOutcome::Skipped { .. } => skipped.push(o.format_entry()),
        }
    }
    let count = applied.len();
    let applied_str = join_or_none(&applied);
    let skipped_str = join_or_none(&skipped);
    tracing::info!(
        applied = %applied_str,
        skipped = %skipped_str,
        "title watchdog hooks ({NO_WATCHDOG_ENV})"
    );
    count
}

/// Applies the resolution levers asked for in the environment
/// (`VICTORY_RENDER_SCALE`, `VICTORY_OWN_UPSCALER`). Returns how many applied.
///
/// # Safety
/// As [`disarm_watchdog`].
pub(crate) unsafe fn resolution_levers(
    base: usize,
    image_size: usize,
    hash: &blake3::Hash,
    is_dll: bool,
) -> usize {
    let scale = std::env::var(RENDER_SCALE_ENV).ok();
    let own = env_flag_enabled(OWN_UPSCALER_ENV);
    // SAFETY: forwarded from this function's contract.
    unsafe {
        apply_resolution_levers(base, image_size, hash, is_dll, scale.as_deref(), own)
    }
}

/// Core implementation of [`resolution_levers`] taking explicit env inputs so
/// it can be unit-tested without mutating process-global environment variables.
///
/// # Safety
/// As [`disarm_watchdog`].
unsafe fn apply_resolution_levers(
    base: usize,
    image_size: usize,
    hash: &blake3::Hash,
    is_dll: bool,
    scale_env: Option<&str>,
    own: bool,
) -> usize {
    if scale_env.is_none() && !own {
        return 0;
    }
    // Every image the loader maps comes here (the title's DLLs too); only
    // the 1.0.2.1 executable has the patched places.
    if !hash.to_hex().starts_with(BUILD_1_0_2_1) {
        if is_dll {
            tracing::debug!("resolution levers: not the 1.0.2.1 executable; nothing patched");
        } else {
            let got = &hash.to_hex()[..16];
            let scale_label = scale_env.unwrap_or("none");
            tracing::info!(
                scale = %scale_label,
                own_upscaler = own,
                applied = "none",
                skipped = %format!("all(exe hash mismatch: got {got}, expected 1.0.2.1 {BUILD_1_0_2_1})"),
                "title resolution levers"
            );
        }
        return 0;
    }

    let mut applied = Vec::new();
    let mut skipped = Vec::new();

    if own {
        // SAFETY: forwarded from this function's contract.
        match unsafe { apply(base, image_size, &OWN_UPSCALER) } {
            o @ PatchOutcome::Applied { .. } => applied.push(o.format_entry()),
            o @ PatchOutcome::Skipped { .. } => skipped.push(o.format_entry()),
        }
    }

    let scale_label = match scale_env {
        Some(text) => match parse_scale(text) {
            Some(scale) => {
                // With the own upscaler forced, FSR is left as it is.
                let (patches, fsr_skip) = render_scale_patches(base, scale, !own);
                for p in &patches {
                    let patch = Patch {
                        what: p.what,
                        rva: p.rva,
                        expect: &p.expect,
                        new: &p.new,
                    };
                    // SAFETY: forwarded from this function's contract.
                    match unsafe { apply(base, image_size, &patch) } {
                        o @ PatchOutcome::Applied { .. } => applied.push(o.format_entry()),
                        o @ PatchOutcome::Skipped { .. } => skipped.push(o.format_entry()),
                    }
                }
                if let Some(note) = fsr_skip {
                    skipped.push(note);
                }
                format!("{scale}")
            }
            None => {
                skipped.push(format!(
                    "render_scale(invalid {RENDER_SCALE_ENV}={text:?}; expected 0.2..=1.0)"
                ));
                format!("invalid({text})")
            }
        },
        None => "none".to_owned(),
    };

    let count = applied.len();
    let applied_str = join_or_none(&applied);
    let skipped_str = join_or_none(&skipped);
    tracing::info!(
        scale = %scale_label,
        own_upscaler = own,
        applied = %applied_str,
        skipped = %skipped_str,
        "title resolution levers"
    );
    count
}

fn join_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_owned()
    } else {
        items.join(", ")
    }
}

/// # Safety
/// `base..base+image_size` must be a readable/mapped range in this process and
/// no other thread may be executing or mutating `base + p.rva`.
unsafe fn apply(base: usize, image_size: usize, p: &Patch<'_>) -> PatchOutcome {
    if p.new.len() != p.expect.len() {
        return PatchOutcome::Skipped {
            what: p.what,
            rva: p.rva,
            reason: "length mismatch",
        };
    }
    if p.rva
        .checked_add(p.expect.len())
        .is_none_or(|end| end > image_size)
    {
        return PatchOutcome::Skipped {
            what: p.what,
            rva: p.rva,
            reason: "out of image bounds",
        };
    }
    let at = (base + p.rva) as *mut u8;
    // SAFETY: bounds checked against `image_size` above.
    let now = unsafe { std::slice::from_raw_parts(at, p.expect.len()) };
    if now != p.expect {
        return PatchOutcome::Skipped {
            what: p.what,
            rva: p.rva,
            reason: "byte pattern mismatch",
        };
    }
    let mut old = 0;
    // SAFETY: changing the protection of those bytes only, restored below.
    if unsafe { VirtualProtect(at as _, p.new.len(), PAGE_EXECUTE_READWRITE, &mut old) } == 0 {
        return PatchOutcome::Skipped {
            what: p.what,
            rva: p.rva,
            reason: "VirtualProtect failed",
        };
    }
    // SAFETY: writable now; the bytes are the ones checked above.
    unsafe {
        std::ptr::copy_nonoverlapping(p.new.as_ptr(), at, p.new.len());
        VirtualProtect(at as _, p.new.len(), old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), at as _, p.new.len());
    }
    PatchOutcome::Applied {
        what: p.what,
        rva: p.rva,
        old_len: p.expect.len(),
        new_len: p.new.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_outside_the_floor_are_refused() {
        assert_eq!(parse_scale("0.2"), Some(0.2));
        assert_eq!(parse_scale("0.5"), Some(0.5));
        assert_eq!(parse_scale(" 1 "), Some(1.0));
        assert_eq!(parse_scale("1.0"), Some(1.0));
        assert_eq!(parse_scale("0.19"), None);
        assert_eq!(parse_scale("0.1"), None);
        assert_eq!(parse_scale("0.0"), None);
        assert_eq!(parse_scale("-0.5"), None);
        assert_eq!(parse_scale("1.01"), None);
        assert_eq!(parse_scale("1.5"), None);
        assert_eq!(parse_scale("half"), None);
        assert_eq!(parse_scale(""), None);
        assert_eq!(parse_scale("NaN"), None);
        assert_eq!(parse_scale("inf"), None);
    }

    #[test]
    fn half_scale_patches_both_upscalers() {
        let base = 0x7ff0_0000_0000;
        let (p, skip) = render_scale_patches(base, 0.5, true);
        assert!(skip.is_none());
        assert_eq!(p.len(), 4);
        assert_eq!(p[0].new, 0.5f32.to_le_bytes());
        // The FSR load now reads 0.5 at 0x5fc6edc: disp from the next instruction.
        assert_eq!(p[2].new, [0xC5, 0xFA, 0x10, 0x05, 0x22, 0x78, 0x6D, 0x03]);
        assert_eq!(p[3].new, ((base + 0x28ef680) as u64).to_le_bytes());
        assert!(p.iter().all(|p| p.new.len() == p.expect.len()));
    }

    #[test]
    fn odd_scales_patch_only_the_own_upscaler() {
        let (p, skip) = render_scale_patches(0, 0.6, true);
        assert_eq!(p.len(), 2);
        assert!(skip.is_some());

        let (p, skip) = render_scale_patches(0, 0.5, false);
        assert_eq!(p.len(), 2);
        assert!(skip.is_none());
    }

    #[test]
    fn pattern_mismatch_skips_without_modifying_memory() {
        let mut buf = vec![0xCCu8; 32];
        buf[8..11].copy_from_slice(&[0x90, 0x90, 0x90]);
        let snapshot = buf.clone();
        let patch = Patch {
            what: "test_mismatch",
            rva: 8,
            expect: &[0x0F, 0x45, 0xC1],
            new: &[0x0F, 0x1F, 0x00],
        };
        // SAFETY: `buf` is a live, valid allocation of length `buf.len()`.
        let outcome = unsafe { apply(buf.as_mut_ptr() as usize, buf.len(), &patch) };
        assert_eq!(
            outcome,
            PatchOutcome::Skipped {
                what: "test_mismatch",
                rva: 8,
                reason: "byte pattern mismatch",
            }
        );
        assert_eq!(buf, snapshot);
    }

    #[test]
    fn out_of_bounds_patch_is_skipped_safely() {
        let mut buf = vec![0u8; 8];
        let patch = Patch {
            what: "test_oob",
            rva: 6,
            expect: &[0, 0, 0, 0],
            new: &[1, 1, 1, 1],
        };
        // SAFETY: `buf` is a live allocation; `apply` checks bounds before reading.
        let outcome = unsafe { apply(buf.as_mut_ptr() as usize, buf.len(), &patch) };
        assert_eq!(
            outcome,
            PatchOutcome::Skipped {
                what: "test_oob",
                rva: 6,
                reason: "out of image bounds",
            }
        );
    }

    #[test]
    fn matching_pattern_applies_and_reports_byte_counts() {
        let mut buf = vec![0xAAu8; 32];
        buf[8..11].copy_from_slice(&[0x0F, 0x45, 0xC1]);
        let patch = Patch {
            what: "own_upscaler",
            rva: 8,
            expect: &[0x0F, 0x45, 0xC1],
            new: &[0x0F, 0x1F, 0x00],
        };
        // SAFETY: `buf` is a live, valid allocation of length `buf.len()`.
        let outcome = unsafe { apply(buf.as_mut_ptr() as usize, buf.len(), &patch) };
        assert_eq!(
            outcome,
            PatchOutcome::Applied {
                what: "own_upscaler",
                rva: 8,
                old_len: 3,
                new_len: 3,
            }
        );
        assert_eq!(&buf[8..11], &[0x0F, 0x1F, 0x00]);
        assert_eq!(outcome.format_entry(), "own_upscaler@0x8(3->3B)");
    }

    #[test]
    fn invalid_scale_and_wrong_hash_apply_no_patches() {
        let mut buf = vec![0u8; 16];
        let wrong_hash = blake3::hash(b"not-the-1.0.2.1-exe");
        // SAFETY: hash mismatch returns before touching memory.
        let applied = unsafe {
            apply_resolution_levers(
                buf.as_mut_ptr() as usize,
                buf.len(),
                &wrong_hash,
                false,
                Some("0.5"),
                true,
            )
        };
        assert_eq!(applied, 0);

        // No env vars -> no patches regardless of hash.
        // SAFETY: returns immediately when neither env var is active.
        let applied = unsafe {
            apply_resolution_levers(
                buf.as_mut_ptr() as usize,
                buf.len(),
                &wrong_hash,
                false,
                None,
                false,
            )
        };
        assert_eq!(applied, 0);

        // Matching 1.0.2.1 hash prefix with invalid scale -> 0 patches applied.
        let matching_hash = blake3::Hash::from_hex(
            "b99f876b74d4a77f000000000000000000000000000000000000000000000000",
        )
        .unwrap();
        // SAFETY: invalid scale never reads or writes `buf`.
        let applied = unsafe {
            apply_resolution_levers(
                buf.as_mut_ptr() as usize,
                buf.len(),
                &matching_hash,
                false,
                Some("0.1"),
                false,
            )
        };
        assert_eq!(applied, 0);
    }
}
