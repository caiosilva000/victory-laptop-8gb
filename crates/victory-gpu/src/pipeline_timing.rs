//! Per-pipeline GPU timing (`VICTORY_PIPELINE_TIMING=1`, off by default).
//!
//! Brackets each draw/dispatch (`draw`, `draw_indexed`, `dispatch`,
//! `dispatch_mesh`, `execute_indirect`) with a pair of D3D12 timestamp queries
//! on sampled frames (every `N`th frame, default 30, configurable via
//! `VICTORY_PIPELINE_TIMING_EVERY`). Non-sampled frames record no queries.
//!
//! If a list exceeds its per-list timestamp budget, remaining calls are
//! counted in the `"unstamped"` bucket (using the tail from the last stamped
//! call's end to list close) rather than attributed to the last stamped
//! command.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct3D12::*;

use crate::host::HostList;
use crate::pipeline::host::HostPipeline;

/// Timestamps per sampled command list: slot 0 is list start, slot 1 is list
/// end, slots `2..POOL_STAMPS` hold `(start, end)` pairs for individual calls.
pub(crate) const POOL_STAMPS: u32 = 16_384;
/// Maximum individually stamped calls per list (`(16384 - 2) / 2 = 8191`).
pub(crate) const MAX_STAMPED_CALLS: usize = ((POOL_STAMPS - 2) / 2) as usize;
/// Default sampling cadence: 1 in every 30 frames.
const DEFAULT_EVERY: u64 = 30;
/// Minimum interval between INFO summary logs.
const LOG_INTERVAL: Duration = Duration::from_secs(5);
/// Number of top pipelines logged per summary.
const TOP_N: usize = 20;

/// Whether `VICTORY_PIPELINE_TIMING` is enabled (cached once; zero cost when off).
#[inline(always)]
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::switch::on("VICTORY_PIPELINE_TIMING"))
}

/// Sampling interval in frames (`VICTORY_PIPELINE_TIMING_EVERY`, default 30).
fn every() -> u64 {
    static EVERY: OnceLock<u64> = OnceLock::new();
    *EVERY.get_or_init(|| {
        std::env::var("VICTORY_PIPELINE_TIMING_EVERY")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|&n| n >= 1)
            .unwrap_or(DEFAULT_EVERY)
    })
}

#[inline]
pub(crate) fn is_sampled_frame(frame: u64, interval: u64) -> bool {
    interval > 0 && frame.is_multiple_of(interval)
}

/// Kind of pipeline for logging.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PipelineKind {
    Compute,
    Graphics,
    Mesh,
    Unstamped,
}

impl PipelineKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Compute => "compute",
            Self::Graphics => "graphics",
            Self::Mesh => "mesh",
            Self::Unstamped => "unstamped",
        }
    }
}

/// Identity of a pipeline for aggregation and logging.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PipelineId {
    pub(crate) hash: String,
    pub(crate) kind: PipelineKind,
    pub(crate) role: Option<String>,
}

impl PipelineId {
    pub(crate) fn from_host_pipeline(p: &HostPipeline) -> Arc<Self> {
        p.timing_id
            .get_or_init(|| {
                let kind = if p.compute {
                    PipelineKind::Compute
                } else if p.mesh {
                    PipelineKind::Mesh
                } else {
                    PipelineKind::Graphics
                };
                let hash = if p.stages.is_empty() {
                    "unknown".to_owned()
                } else if p.stages.len() == 1 {
                    p.stages[0].shader.hash.clone()
                } else {
                    p.stages
                        .iter()
                        .map(|s| s.shader.hash.as_str())
                        .collect::<Vec<_>>()
                        .join("+")
                };
                let role = p.stages.iter().find_map(|s| s.shader.role_name());
                Arc::new(Self { hash, kind, role })
            })
            .clone()
    }

    pub(crate) fn unstamped() -> Arc<Self> {
        static UNSTAMPED: OnceLock<Arc<PipelineId>> = OnceLock::new();
        UNSTAMPED
            .get_or_init(|| {
                Arc::new(Self {
                    hash: "unstamped".to_owned(),
                    kind: PipelineKind::Unstamped,
                    role: None,
                })
            })
            .clone()
    }

    fn unknown() -> Arc<Self> {
        static UNKNOWN: OnceLock<Arc<PipelineId>> = OnceLock::new();
        UNKNOWN
            .get_or_init(|| {
                Arc::new(Self {
                    hash: "unknown".to_owned(),
                    kind: PipelineKind::Graphics,
                    role: None,
                })
            })
            .clone()
    }
}

/// Pure-logic per-list recording state (separated from D3D12 calls for unit testing).
#[derive(Default)]
struct ListRecorder {
    frame: Option<u64>,
    calls: Vec<(Arc<PipelineId>, u32, u32)>,
    unstamped_calls: u64,
    max_stamped_calls: usize,
}

impl ListRecorder {
    fn new(max_stamped_calls: usize) -> Self {
        Self {
            frame: None,
            calls: Vec::new(),
            unstamped_calls: 0,
            max_stamped_calls,
        }
    }

    fn reset(&mut self) {
        self.frame = None;
        self.calls.clear();
        self.unstamped_calls = 0;
    }

    /// Allocates a `(start_slot, end_slot)` pair if within budget, or counts
    /// the call as unstamped. Returns `(is_first_on_list, Some((start, end)))`
    /// when stamped, or `(is_first_on_list, None)` when over budget.
    fn record_call(&mut self, frame: u64, id: Arc<PipelineId>) -> (bool, Option<(u32, u32)>) {
        let first = self.frame.is_none();
        if first {
            self.frame = Some(frame);
        }
        if self.calls.len() < self.max_stamped_calls {
            let start = 2 + (self.calls.len() as u32) * 2;
            let end = start + 1;
            self.calls.push((id, start, end));
            (first, Some((start, end)))
        } else {
            self.unstamped_calls += 1;
            (first, None)
        }
    }

    fn used_slots(&self) -> u32 {
        if self.frame.is_none() {
            return 0;
        }
        2 + (self.calls.len() as u32) * 2
    }

    /// Resolves raw GPU timestamp ticks into per-pipeline durations and total
    /// list duration. Any unstamped tail (`list_end - last_stamped_end`) goes
    /// strictly to `"unstamped"`, never to the last stamped call.
    fn resolve(&self, stamps: &[u64], frequency: u64) -> Option<(u64, ListSample)> {
        let frame = self.frame?;
        let needed = self.used_slots() as usize;
        if stamps.len() < needed || needed < 2 {
            return None;
        }
        let freq = frequency.max(1);
        let to_ns = |ticks: u64| -> u64 {
            ((u128::from(ticks) * 1_000_000_000u128) / u128::from(freq)) as u64
        };

        let mut sample = ListSample::default();
        let list_ticks = stamps[1].saturating_sub(stamps[0]);
        let mut last_end_tick = stamps[0];
        let mut stamped_ns = 0u64;

        for (id, start, end) in &self.calls {
            let s = stamps[*start as usize];
            let e = stamps[*end as usize];
            let ns = to_ns(e.saturating_sub(s));
            stamped_ns = stamped_ns.saturating_add(ns);
            last_end_tick = e;
            sample.add_call(id.clone(), ns);
        }

        let mut unstamped_ns = 0u64;
        if self.unstamped_calls > 0 {
            unstamped_ns = to_ns(stamps[1].saturating_sub(last_end_tick));
            sample.add_unstamped(self.unstamped_calls, unstamped_ns);
        }

        let list_ns = to_ns(list_ticks);
        sample.stamped_ns = stamped_ns;
        sample.total_ns = list_ns.max(stamped_ns.saturating_add(unstamped_ns));
        Some((frame, sample))
    }
}

/// Aggregated stats for a single pipeline within a frame or list.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PipelineStat {
    pub(crate) total_ns: u64,
    pub(crate) calls: u64,
    pub(crate) longest_ns: u64,
}

impl PipelineStat {
    fn merge(&mut self, other: Self) {
        self.total_ns = self.total_ns.saturating_add(other.total_ns);
        self.calls = self.calls.saturating_add(other.calls);
        self.longest_ns = self.longest_ns.max(other.longest_ns);
    }
}

#[derive(Default, Clone, Debug)]
struct ListSample {
    total_ns: u64,
    stamped_ns: u64,
    by_pipeline: HashMap<Arc<PipelineId>, PipelineStat>,
}

impl ListSample {
    fn add_call(&mut self, id: Arc<PipelineId>, ns: u64) {
        let entry = self.by_pipeline.entry(id).or_default();
        entry.total_ns = entry.total_ns.saturating_add(ns);
        entry.calls += 1;
        entry.longest_ns = entry.longest_ns.max(ns);
    }

    fn add_unstamped(&mut self, calls: u64, ns: u64) {
        let entry = self.by_pipeline.entry(PipelineId::unstamped()).or_default();
        entry.total_ns = entry.total_ns.saturating_add(ns);
        entry.calls = entry.calls.saturating_add(calls);
        entry.longest_ns = entry.longest_ns.max(ns);
    }
}

#[derive(Default, Clone, Debug)]
struct FrameSample {
    total_ns: u64,
    stamped_ns: u64,
    by_pipeline: HashMap<Arc<PipelineId>, PipelineStat>,
}

impl FrameSample {
    fn merge(&mut self, list: ListSample) {
        self.total_ns = self.total_ns.saturating_add(list.total_ns);
        self.stamped_ns = self.stamped_ns.saturating_add(list.stamped_ns);
        for (id, stat) in list.by_pipeline {
            self.by_pipeline.entry(id).or_default().merge(stat);
        }
    }

    /// Formats the header and top `limit` pipeline lines sorted by total GPU
    /// time descending.
    fn format_lines(&self, frame: u64, limit: usize) -> Vec<String> {
        let total_ms = self.total_ns as f64 / 1e6;
        let stamped_pct = if self.total_ns > 0 {
            (self.stamped_ns as f64 * 100.0 / self.total_ns as f64).clamp(0.0, 100.0)
        } else {
            0.0
        };
        let mut entries: Vec<(&Arc<PipelineId>, PipelineStat)> =
            self.by_pipeline.iter().map(|(k, v)| (k, *v)).collect();
        entries.sort_by(|(ka, va), (kb, vb)| {
            vb.total_ns
                .cmp(&va.total_ns)
                .then_with(|| ka.hash.cmp(&kb.hash))
        });

        let mut lines = Vec::with_capacity(1 + entries.len().min(limit));
        lines.push(format!(
            "card time by pipeline (sampled frame {frame}): total {total_ms:.2} ms, stamped {stamped_pct:.1}%:"
        ));
        for (id, stat) in entries.into_iter().take(limit) {
            let ms = stat.total_ns as f64 / 1e6;
            let longest_ms = stat.longest_ns as f64 / 1e6;
            match &id.role {
                Some(role) => lines.push(format!(
                    "  {} {} {ms:.2} ms {} calls longest {longest_ms:.2} ms ({role})",
                    id.hash,
                    id.kind.as_str(),
                    stat.calls
                )),
                None => lines.push(format!(
                    "  {} {} {ms:.2} ms {} calls longest {longest_ms:.2} ms",
                    id.hash,
                    id.kind.as_str(),
                    stat.calls
                )),
            }
        }
        lines
    }
}

/// Per-`HostList` timing state.
///
/// `state`: `0` = undecided for current recording, `1` = non-sampled frame,
/// `2` = actively sampling. Checked atomically so non-sampled frames never
/// touch the mutex or D3D12 query heap.
pub(crate) struct ListTiming {
    state: AtomicU8,
    heap: OnceLock<Option<(ID3D12QueryHeap, ID3D12Resource)>>,
    recorder: Mutex<ListRecorder>,
}

// SAFETY: D3D12 query heap and readback buffer are thread-safe COM pointers
// synchronized by command list recording/execution lifecycle (same as `HostList::stamps`).
unsafe impl Send for ListTiming {}
unsafe impl Sync for ListTiming {}

impl ListTiming {
    pub(crate) fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
            heap: OnceLock::new(),
            recorder: Mutex::new(ListRecorder::new(MAX_STAMPED_CALLS)),
        }
    }

    fn heap(&self) -> Option<&(ID3D12QueryHeap, ID3D12Resource)> {
        self.heap
            .get_or_init(|| crate::host::make_timestamp_heap(POOL_STAMPS))
            .as_ref()
    }

    pub(crate) fn reset(&self) {
        if !on() {
            return;
        }
        if self.state.swap(0, Ordering::Relaxed) == 2 {
            self.recorder.lock().unwrap_or_else(|e| e.into_inner()).reset();
        }
    }

    pub(crate) fn close(&self, list: &ID3D12GraphicsCommandList) {
        if !on() || self.state.load(Ordering::Relaxed) != 2 {
            return;
        }
        let used = self.recorder.lock().unwrap_or_else(|e| e.into_inner()).used_slots();
        if used < 2 {
            return;
        }
        let Some((heap, back)) = self.heap() else {
            return;
        };
        // SAFETY: open command list, our timestamp heap and readback buffer.
        unsafe {
            list.EndQuery(heap, D3D12_QUERY_TYPE_TIMESTAMP, 1);
            list.ResolveQueryData(heap, D3D12_QUERY_TYPE_TIMESTAMP, 0, used, back, 0);
        }
    }

    /// Reads back resolved timestamps after the list finishes on the GPU and
    /// merges them into the sampled frame aggregator.
    pub(crate) fn land(&self) {
        if !on() || self.state.swap(0, Ordering::Relaxed) != 2 {
            return;
        }
        let resolved = {
            let mut rec = self.recorder.lock().unwrap_or_else(|e| e.into_inner());
            let used = rec.used_slots() as usize;
            let result = if used >= 2
                && let Some((_, back)) = self.heap()
            {
                let mut data = std::ptr::null_mut();
                let range = D3D12_RANGE { Begin: 0, End: used * 8 };
                // SAFETY: readback buffer resolved by this finished list.
                let stamps = unsafe {
                    if back.Map(0, Some(&range), Some(&mut data)).is_ok() && !data.is_null() {
                        let s = std::slice::from_raw_parts(data.cast::<u64>(), used).to_vec();
                        back.Unmap(0, None);
                        Some(s)
                    } else {
                        None
                    }
                };
                stamps.and_then(|s| rec.resolve(&s, crate::host::timestamp_frequency()))
            } else {
                None
            };
            rec.reset();
            result
        };
        if let Some((frame, sample)) = resolved {
            record_sample(frame, sample);
        }
    }
}

/// RAII guard that emits the end timestamp right after the GPU command.
pub(crate) struct StampGuard<'a> {
    list: &'a HostList,
    heap: &'a ID3D12QueryHeap,
    end_slot: u32,
}

impl Drop for StampGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: open command list and our timestamp query heap.
        unsafe {
            self.list
                .list
                .EndQuery(self.heap, D3D12_QUERY_TYPE_TIMESTAMP, self.end_slot);
        }
    }
}

/// Begins timing a draw/dispatch/execute_indirect on `list` for pipeline `p`.
/// Zero cost when `VICTORY_PIPELINE_TIMING` is off.
#[inline(always)]
pub(crate) fn begin<'a>(list: &'a HostList, p: Option<&HostPipeline>) -> Option<StampGuard<'a>> {
    if !on() {
        return None;
    }
    begin_slow(list, p)
}

#[cold]
fn begin_slow<'a>(list: &'a HostList, p: Option<&HostPipeline>) -> Option<StampGuard<'a>> {
    let timing = &list.pipeline_timing;
    let state = timing.state.load(Ordering::Relaxed);
    if state == 1 {
        return None;
    }
    let frame = crate::frame::current();
    if state == 0 {
        if !is_sampled_frame(frame, every()) {
            timing.state.store(1, Ordering::Relaxed);
            return None;
        }
        timing.state.store(2, Ordering::Relaxed);
    }
    let (heap, _) = timing.heap()?;
    let id = p.map_or_else(PipelineId::unknown, PipelineId::from_host_pipeline);
    let (first, slots) = timing
        .recorder
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .record_call(frame, id);
    if first {
        // SAFETY: open command list and our timestamp query heap.
        unsafe {
            list.list.EndQuery(heap, D3D12_QUERY_TYPE_TIMESTAMP, 0);
        }
    }
    let (start_slot, end_slot) = slots?;
    // SAFETY: open command list and our timestamp query heap.
    unsafe {
        list.list
            .EndQuery(heap, D3D12_QUERY_TYPE_TIMESTAMP, start_slot);
    }
    Some(StampGuard {
        list,
        heap,
        end_slot,
    })
}

struct Aggregator {
    frames: BTreeMap<u64, FrameSample>,
    last_logged: Option<Instant>,
}

static AGGREGATOR: Mutex<Aggregator> = Mutex::new(Aggregator {
    frames: BTreeMap::new(),
    last_logged: None,
});

fn record_sample(frame: u64, sample: ListSample) {
    let mut agg = AGGREGATOR.lock().unwrap_or_else(|e| e.into_inner());
    agg.frames.entry(frame).or_default().merge(sample);
    // Keep only the most recent 4 sampled frames in memory.
    while agg.frames.len() > 4 {
        if let Some((&oldest, _)) = agg.frames.first_key_value() {
            agg.frames.remove(&oldest);
        }
    }
}

/// Logs the latest completed sampled frame roughly every 5 seconds.
#[inline]
pub(crate) fn maybe_log() {
    if !on() {
        return;
    }
    maybe_log_slow();
}

#[cold]
fn maybe_log_slow() {
    let now = Instant::now();
    let current = crate::frame::current();
    let lines = {
        let mut agg = AGGREGATOR.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(last) = agg.last_logged
            && now.duration_since(last) < LOG_INTERVAL
        {
            return;
        }
        // Prefer the newest sampled frame strictly before `current` so all of
        // its submissions have landed.
        let picked = agg
            .frames
            .keys()
            .rev()
            .copied()
            .find(|&f| f < current);
        let Some(frame) = picked else {
            return;
        };
        let Some(sample) = agg.frames.remove(&frame) else {
            return;
        };
        // Drop any older unlogged sampled frames.
        agg.frames.retain(|&f, _| f > frame);
        agg.last_logged = Some(now);
        sample.format_lines(frame, TOP_N)
    };
    for line in lines {
        tracing::info!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(hash: &str, kind: PipelineKind, role: Option<&str>) -> Arc<PipelineId> {
        Arc::new(PipelineId {
            hash: hash.to_owned(),
            kind,
            role: role.map(str::to_owned),
        })
    }

    #[test]
    fn disabled_records_no_stamps() {
        // Default env has VICTORY_PIPELINE_TIMING unset -> off.
        if std::env::var_os("VICTORY_PIPELINE_TIMING").is_none() {
            assert!(!on());
        }
        // Non-sampled frames produce no stamps.
        assert!(is_sampled_frame(0, 30));
        assert!(!is_sampled_frame(1, 30));
        assert!(!is_sampled_frame(29, 30));
        assert!(is_sampled_frame(30, 30));
        assert!(!is_sampled_frame(31, 30));
        assert!(is_sampled_frame(60, 30));

        // Fresh recorder with no calls uses 0 slots and resolves to None.
        let rec = ListRecorder::new(MAX_STAMPED_CALLS);
        assert_eq!(rec.used_slots(), 0);
        assert!(rec.resolve(&[], 1_000_000_000).is_none());
    }

    #[test]
    fn budget_overflow_goes_to_unstamped_not_last_command() {
        // Budget of 2 stamped calls -> 6 timestamp slots total.
        let mut rec = ListRecorder::new(2);
        let p_a = pid("aaaa000000000001", PipelineKind::Compute, Some("PassA"));
        let p_b = pid("bbbb000000000002", PipelineKind::Graphics, None);
        let p_c = pid("cccc000000000003", PipelineKind::Mesh, None);

        assert_eq!(rec.record_call(30, p_a.clone()), (true, Some((2, 3))));
        assert_eq!(rec.record_call(30, p_b.clone()), (false, Some((4, 5))));
        // Over budget: must return None and count as unstamped.
        assert_eq!(rec.record_call(30, p_c.clone()), (false, None));
        assert_eq!(rec.record_call(30, p_c.clone()), (false, None));
        assert_eq!(rec.used_slots(), 6);
        assert_eq!(rec.unstamped_calls, 2);

        // Frequency = 1 GHz -> 1 tick = 1 ns.
        // slot 0 (list_start) = 1_000_000 (1 ms)
        // slot 1 (list_end)   = 13_100_000 (13.1 ms -> total 12.1 ms)
        // call 0 (p_a): 1_000_000 .. 2_000_000 (1.0 ms)
        // call 1 (p_b): 2_100_000 .. 3_100_000 (1.0 ms)
        // unstamped tail after call 1: 3_100_000 .. 13_100_000 (10.0 ms)
        let stamps = [
            1_000_000,
            13_100_000,
            1_000_000,
            2_000_000,
            2_100_000,
            3_100_000,
        ];
        let (frame, sample) = rec.resolve(&stamps, 1_000_000_000).expect("resolved");
        assert_eq!(frame, 30);
        assert_eq!(sample.total_ns, 12_100_000);
        assert_eq!(sample.stamped_ns, 2_000_000);

        // p_b MUST only get its own 1.0 ms, NOT the 10.0 ms tail!
        let stat_a = sample.by_pipeline.get(&p_a).copied().expect("p_a");
        let stat_b = sample.by_pipeline.get(&p_b).copied().expect("p_b");
        let stat_u = sample
            .by_pipeline
            .get(&PipelineId::unstamped())
            .copied()
            .expect("unstamped");
        assert!(sample.by_pipeline.get(&p_c).is_none());

        assert_eq!(stat_a, PipelineStat { total_ns: 1_000_000, calls: 1, longest_ns: 1_000_000 });
        assert_eq!(stat_b, PipelineStat { total_ns: 1_000_000, calls: 1, longest_ns: 1_000_000 });
        assert_eq!(stat_u, PipelineStat { total_ns: 10_000_000, calls: 2, longest_ns: 10_000_000 });
    }

    #[test]
    fn sorting_and_formatting_orders_descending_and_truncates_top_20() {
        let mut frame = FrameSample::default();
        let mut list = ListSample::default();
        // Add 25 distinct pipelines; pipeline 7 has 2 calls (total 7.0 ms, longest 5.0 ms).
        let probe = pid("3592426e3e9142d8", PipelineKind::Compute, Some("ProbeGather"));
        list.add_call(probe.clone(), 5_000_000);
        list.add_call(probe.clone(), 2_000_000);

        for i in 1u64..=24 {
            let kind = match i % 3 {
                0 => PipelineKind::Compute,
                1 => PipelineKind::Graphics,
                _ => PipelineKind::Mesh,
            };
            let id = pid(&format!("{i:016x}"), kind, None);
            // i * 0.25 ms -> max is 6.00 ms (below probe's 7.00 ms).
            list.add_call(id, i * 250_000);
        }
        let stamped: u64 = list.by_pipeline.values().map(|s| s.total_ns).sum();
        list.stamped_ns = stamped;
        list.total_ns = stamped;
        frame.merge(list);

        let lines = frame.format_lines(60, TOP_N);
        assert_eq!(lines.len(), 1 + TOP_N);
        assert!(lines[0].starts_with("card time by pipeline (sampled frame 60): total "));
        assert!(lines[0].contains("stamped 100.0%:"));
        // First row must be ProbeGather with 7.00 ms, 2 calls, longest 5.00 ms.
        assert_eq!(
            lines[1],
            "  3592426e3e9142d8 compute 7.00 ms 2 calls longest 5.00 ms (ProbeGather)"
        );
        // Second row must be i=24 (6.00 ms), third i=23 (5.75 ms).
        assert_eq!(
            lines[2],
            "  0000000000000018 compute 6.00 ms 1 calls longest 6.00 ms"
        );
        assert_eq!(
            lines[3],
            "  0000000000000017 mesh 5.75 ms 1 calls longest 5.75 ms"
        );
    }
}
