//! Stream health: live alerts while streaming and a post-stream report.
//!
//! Input is periodic [`StatsSample`]s the Windows app reads from obs-websocket
//! (`GetStats` + `GetStreamStatus`). OBS loses frames in three ways, each with
//! its own cause and fix:
//!
//! | OBS counter                 | Plain language                      |
//! |-----------------------------|-------------------------------------|
//! | dropped frames / congestion | "Your internet can't keep up"       |
//! | output skipped (encoding)   | "Your PC can't encode fast enough"  |
//! | render skipped (rendering)  | "Your game is starving OBS"         |

use crate::finding::{rank, Area, Finding, FixKind, Severity};
use crate::score::{score_area, Score};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[cfg(test)]
mod tests;

/// One reading of OBS's counters. Counters are cumulative since the output
/// (or OBS) started; a decrease means a restart and is handled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StatsSample {
    /// Milliseconds on any monotonic clock.
    pub t_ms: u64,
    pub cpu_percent: f32,
    pub active_fps: f32,
    pub avg_frame_render_ms: f32,
    /// Frames OBS failed to render in time (GetStats renderSkippedFrames).
    pub render_skipped: u64,
    pub render_total: u64,
    /// Frames skipped by the encoder/output thread (GetStats outputSkippedFrames).
    pub output_skipped: u64,
    pub output_total: u64,
    /// Frames dropped by the network, if the app can read them.
    pub network_dropped: Option<u64>,
    pub network_total: Option<u64>,
    /// GetStreamStatus outputCongestion, 0..1.
    pub congestion: f32,
    /// GetStreamStatus outputBytes (cumulative).
    pub bytes_sent: u64,
    pub reconnecting: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertKind {
    Network,
    Encoding,
    Rendering,
    LowBitrate,
    Reconnecting,
}

const ALL_KINDS: [AlertKind; 5] = [
    AlertKind::Network,
    AlertKind::Encoding,
    AlertKind::Rendering,
    AlertKind::LowBitrate,
    AlertKind::Reconnecting,
];

/// A live alert transition: raised when a problem starts, cleared when it has
/// been gone long enough.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alert {
    pub kind: AlertKind,
    /// true = problem started, false = problem cleared.
    pub raised: bool,
    pub t_ms: u64,
    pub title: String,
    pub cause: String,
    pub fix: String,
    /// Loss percent for frame alerts, measured kbps for bitrate alerts.
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorConfig {
    /// Sliding window the alerts look at.
    pub window_ms: u64,
    /// Raise a frame alert above this lost ratio (0.01 = 1%).
    pub raise_loss_ratio: f64,
    /// A raised alert may clear once loss stays below this ratio...
    pub clear_loss_ratio: f64,
    /// ...for this long.
    pub clear_after_ms: u64,
    /// Average congestion that counts as a network problem.
    pub congestion_raise: f64,
    /// Need at least this many frames in the window before judging.
    pub min_window_frames: u64,
    /// The bitrate set in OBS; enables low-bitrate alerts.
    pub configured_bitrate_kbps: Option<u32>,
    /// Alert when measured bitrate falls below this share of the configured one.
    pub low_bitrate_ratio: f64,
    /// x264 (CPU) vs hardware encoder, changes the encoding advice.
    pub encoder_is_cpu: bool,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        MonitorConfig {
            window_ms: 30_000,
            raise_loss_ratio: 0.01,
            clear_loss_ratio: 0.003,
            clear_after_ms: 30_000,
            congestion_raise: 0.3,
            min_window_frames: 60,
            configured_bitrate_kbps: None,
            low_bitrate_ratio: 0.7,
            encoder_is_cpu: false,
        }
    }
}

/// Loss ratios and bitrate over the current window.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct WindowStats {
    pub duration_ms: u64,
    pub render_loss: Option<f64>,
    pub encoding_loss: Option<f64>,
    pub network_loss: Option<f64>,
    pub avg_congestion: f64,
    pub bitrate_kbps: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default)]
struct KindState {
    active: bool,
    good_since: Option<u64>,
}

/// Ingests samples and emits alert transitions with hysteresis, so a single
/// bad second doesn't spam the streamer.
#[derive(Debug, Clone)]
pub struct HealthMonitor {
    cfg: MonitorConfig,
    samples: VecDeque<StatsSample>,
    state: [KindState; 5],
}

fn idx(k: AlertKind) -> usize {
    ALL_KINDS.iter().position(|x| *x == k).unwrap()
}

fn ratio(lost: u64, total: u64, min_total: u64) -> Option<f64> {
    (total >= min_total && total > 0).then(|| lost as f64 / total as f64)
}

fn counters_reset(prev: &StatsSample, cur: &StatsSample) -> bool {
    cur.render_total < prev.render_total
        || cur.output_total < prev.output_total
        || cur.bytes_sent < prev.bytes_sent
        || matches!((prev.network_total, cur.network_total), (Some(p), Some(c)) if c < p)
}

impl HealthMonitor {
    pub fn new(cfg: MonitorConfig) -> Self {
        HealthMonitor { cfg, samples: VecDeque::new(), state: [KindState::default(); 5] }
    }

    pub fn config(&self) -> &MonitorConfig {
        &self.cfg
    }

    /// Kinds currently raised.
    pub fn active_alerts(&self) -> Vec<AlertKind> {
        ALL_KINDS.iter().copied().filter(|k| self.state[idx(*k)].active).collect()
    }

    /// Stats over the current window.
    pub fn window_stats(&self) -> WindowStats {
        let (Some(first), Some(last)) = (self.samples.front(), self.samples.back()) else {
            return WindowStats::default();
        };
        let min = self.cfg.min_window_frames;
        let network_loss = match (first.network_dropped, last.network_dropped, first.network_total, last.network_total) {
            (Some(d0), Some(d1), Some(t0), Some(t1)) => ratio(d1.saturating_sub(d0), t1.saturating_sub(t0), min),
            _ => None,
        };
        let duration_ms = last.t_ms.saturating_sub(first.t_ms);
        let n = self.samples.len() as f64;
        WindowStats {
            duration_ms,
            render_loss: ratio(last.render_skipped.saturating_sub(first.render_skipped), last.render_total.saturating_sub(first.render_total), min),
            encoding_loss: ratio(last.output_skipped.saturating_sub(first.output_skipped), last.output_total.saturating_sub(first.output_total), min),
            network_loss,
            avg_congestion: self.samples.iter().map(|s| s.congestion as f64).sum::<f64>() / n,
            bitrate_kbps: (duration_ms > 0).then(|| last.bytes_sent.saturating_sub(first.bytes_sent) as f64 * 8.0 / duration_ms as f64),
        }
    }

    /// Feed one sample; returns alerts that were raised or cleared by it.
    pub fn ingest(&mut self, sample: StatsSample) -> Vec<Alert> {
        if let Some(prev) = self.samples.back() {
            if counters_reset(prev, &sample) || sample.t_ms < prev.t_ms {
                self.samples.clear();
            }
        }
        let now = sample.t_ms;
        let reconnecting = sample.reconnecting;
        self.samples.push_back(sample);
        while let Some(front) = self.samples.front() {
            if now.saturating_sub(front.t_ms) > self.cfg.window_ms && self.samples.len() > 2 {
                self.samples.pop_front();
            } else {
                break;
            }
        }

        let w = self.window_stats();
        let mut out = Vec::new();

        // Reconnecting is immediate both ways.
        self.transition(AlertKind::Reconnecting, reconnecting, !reconnecting, true, now, 0.0, &mut out);

        let raise = self.cfg.raise_loss_ratio;
        let clear = self.cfg.clear_loss_ratio;
        let cong = self.cfg.congestion_raise;

        let net_bad = w.network_loss.is_some_and(|r| r > raise) || w.avg_congestion > cong;
        let net_good = w.network_loss.is_none_or(|r| r < clear) && w.avg_congestion < cong / 2.0;
        let net_val = w.network_loss.map(|r| r * 100.0).unwrap_or(w.avg_congestion * 100.0);
        self.transition(AlertKind::Network, net_bad && !reconnecting, net_good, false, now, net_val, &mut out);

        if let Some(r) = w.encoding_loss {
            self.transition(AlertKind::Encoding, r > raise, r < clear, false, now, r * 100.0, &mut out);
        }
        if let Some(r) = w.render_loss {
            self.transition(AlertKind::Rendering, r > raise, r < clear, false, now, r * 100.0, &mut out);
        }

        if let (Some(cfg_kbps), Some(kbps)) = (self.cfg.configured_bitrate_kbps, w.bitrate_kbps) {
            if w.duration_ms >= 10_000 {
                let target = cfg_kbps as f64;
                let bad = kbps < target * self.cfg.low_bitrate_ratio && !reconnecting;
                let good = kbps > target * 0.85;
                self.transition(AlertKind::LowBitrate, bad, good, false, now, kbps.round(), &mut out);
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn transition(&mut self, kind: AlertKind, bad: bool, good: bool, immediate: bool, now: u64, value: f64, out: &mut Vec<Alert>) {
        let clear_after = self.cfg.clear_after_ms;
        let st = &mut self.state[idx(kind)];
        if !st.active {
            if bad {
                st.active = true;
                st.good_since = None;
                out.push(alert(kind, true, now, value, &self.cfg));
            }
            return;
        }
        if good {
            let since = *st.good_since.get_or_insert(now);
            if immediate || now.saturating_sub(since) >= clear_after {
                st.active = false;
                st.good_since = None;
                out.push(alert(kind, false, now, value, &self.cfg));
            }
        } else {
            st.good_since = None;
        }
    }
}

fn alert(kind: AlertKind, raised: bool, t_ms: u64, value: f64, cfg: &MonitorConfig) -> Alert {
    let (title, cause, fix) = if raised { texts(kind, cfg.encoder_is_cpu) } else { cleared_text(kind) };
    Alert { kind, raised, t_ms, title: title.into(), cause: cause.into(), fix: fix.into(), value }
}

fn texts(kind: AlertKind, cpu: bool) -> (&'static str, &'static str, &'static str) {
    match kind {
        AlertKind::Network => (
            "Your internet can't keep up",
            "Frames are being dropped before they reach the platform: your upload is too slow or unsteady for this bitrate. Wi-Fi is the usual cause.",
            "Lower the stream bitrate, plug in an Ethernet cable, or let OBS pick the closest server.",
        ),
        AlertKind::Encoding if cpu => (
            "Your PC can't encode fast enough",
            "The processor encoder (x264) is overloaded, so OBS skips frames.",
            "Switch to your graphics card's encoder (NVENC / AMF / QuickSync), or use a faster preset or lower resolution.",
        ),
        AlertKind::Encoding => (
            "Your PC can't encode fast enough",
            "Your graphics card's encoder is overloaded (often because the game uses the whole GPU), so OBS skips frames.",
            "Cap your game's frame rate, lower the stream resolution or fps, or use a faster encoder preset.",
        ),
        AlertKind::Rendering => (
            "Your game is starving OBS",
            "Your graphics card is at its limit, so OBS can't draw stream frames in time.",
            "Cap your game's FPS (e.g. to your monitor's refresh rate), run OBS as administrator, and remove sources you don't use.",
        ),
        AlertKind::LowBitrate => (
            "Your stream quality dropped",
            "Your stream is sending much less than its set bitrate, usually because the connection got worse and OBS lowered it.",
            "Check your connection (pause big downloads, use Ethernet). If it keeps happening, set a lower fixed bitrate.",
        ),
        AlertKind::Reconnecting => (
            "Your stream disconnected",
            "OBS lost the connection to the platform and is reconnecting. Viewers see a frozen or ended stream.",
            "Check your internet connection. If it happens often, lower your bitrate and use Ethernet.",
        ),
    }
}

fn cleared_text(kind: AlertKind) -> (&'static str, &'static str, &'static str) {
    match kind {
        AlertKind::Network => ("Your connection recovered", "Frames are getting through again.", ""),
        AlertKind::Encoding => ("Encoding caught up", "OBS is encoding every frame again.", ""),
        AlertKind::Rendering => ("OBS is rendering smoothly again", "OBS is drawing every frame again.", ""),
        AlertKind::LowBitrate => ("Stream quality is back", "Your stream is sending its full bitrate again.", ""),
        AlertKind::Reconnecting => ("Stream reconnected", "OBS is connected to the platform again.", ""),
    }
}

// ---------------------------------------------------------------- post-stream report

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReportConfig {
    pub configured_bitrate_kbps: Option<u32>,
    pub target_fps: Option<f32>,
    pub encoder_is_cpu: bool,
    /// Bucket size for "worst moments".
    pub bucket_ms: u64,
}

impl Default for ReportConfig {
    fn default() -> Self {
        ReportConfig { configured_bitrate_kbps: None, target_fps: None, encoder_is_cpu: false, bucket_ms: 10_000 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct LossTotals {
    pub lost: u64,
    pub total: u64,
    pub percent: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum LossKind {
    Network,
    Encoding,
    Rendering,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WorstMoment {
    /// Offset from stream start.
    pub offset_ms: u64,
    pub kind: LossKind,
    pub loss_percent: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamReport {
    pub duration_s: f64,
    pub network: Option<LossTotals>,
    pub encoding: LossTotals,
    pub rendering: LossTotals,
    pub avg_bitrate_kbps: Option<f64>,
    pub min_bitrate_kbps: Option<f64>,
    /// Coefficient of variation of per-bucket bitrate (0 = perfectly steady).
    pub bitrate_variation: Option<f64>,
    pub reconnect_events: u32,
    pub avg_fps: Option<f64>,
    pub peak_cpu_percent: f64,
    pub avg_cpu_percent: f64,
    /// Up to 5 worst 10-second moments (≥1% loss), worst first.
    pub worst_moments: Vec<WorstMoment>,
    pub findings: Vec<Finding>,
    pub score: Score,
}

pub mod ids {
    pub const NETWORK_DROPS: &str = "stream.network_drops";
    pub const ENCODING_LAG: &str = "stream.encoding_lag";
    pub const RENDERING_LAG: &str = "stream.rendering_lag";
    pub const BITRATE_UNSTABLE: &str = "stream.bitrate_unstable";
    pub const BITRATE_LOW: &str = "stream.bitrate_below_setting";
    pub const RECONNECTS: &str = "stream.reconnects";
    pub const LOW_FPS: &str = "stream.low_fps";
    pub const CPU_HIGH: &str = "stream.cpu_maxed";
}

#[derive(Default, Clone, Copy)]
struct Acc {
    lost: u64,
    total: u64,
}

impl Acc {
    fn add(&mut self, lost0: u64, lost1: u64, tot0: u64, tot1: u64) {
        self.lost += lost1.saturating_sub(lost0);
        self.total += tot1.saturating_sub(tot0);
    }
    fn totals(self) -> LossTotals {
        LossTotals {
            lost: self.lost,
            total: self.total,
            percent: if self.total > 0 { (self.lost as f64 / self.total as f64 * 10_000.0).round() / 100.0 } else { 0.0 },
        }
    }
}

/// Summarise a whole stream from its samples (in time order).
pub fn post_stream_report(samples: &[StatsSample], cfg: &ReportConfig) -> StreamReport {
    let mut render = Acc::default();
    let mut encode = Acc::default();
    let mut network = Acc::default();
    let mut has_network = false;
    let mut bytes = 0u64;
    let mut reconnects = 0u32;

    let start = samples.first().map(|s| s.t_ms).unwrap_or(0);
    let bucket_ms = cfg.bucket_ms.max(1000);
    // Per bucket: (render, encode, network, bytes, ms)
    let mut buckets: Vec<(Acc, Acc, Acc, u64, u64)> = Vec::new();

    for pair in samples.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if b.reconnecting && !a.reconnecting {
            reconnects += 1;
        }
        if counters_reset(a, b) || b.t_ms < a.t_ms {
            continue; // restart: next pair starts a new segment
        }
        let bi = ((b.t_ms - start) / bucket_ms) as usize;
        if buckets.len() <= bi {
            buckets.resize(bi + 1, (Acc::default(), Acc::default(), Acc::default(), 0, 0));
        }
        let bk = &mut buckets[bi];
        render.add(a.render_skipped, b.render_skipped, a.render_total, b.render_total);
        bk.0.add(a.render_skipped, b.render_skipped, a.render_total, b.render_total);
        encode.add(a.output_skipped, b.output_skipped, a.output_total, b.output_total);
        bk.1.add(a.output_skipped, b.output_skipped, a.output_total, b.output_total);
        if let (Some(d0), Some(d1), Some(t0), Some(t1)) = (a.network_dropped, b.network_dropped, a.network_total, b.network_total) {
            has_network = true;
            network.add(d0, d1, t0, t1);
            bk.2.add(d0, d1, t0, t1);
        }
        let db = b.bytes_sent.saturating_sub(a.bytes_sent);
        bytes += db;
        bk.3 += db;
        bk.4 += b.t_ms - a.t_ms;
    }
    if samples.first().is_some_and(|s| s.reconnecting) {
        reconnects += 1;
    }

    let duration_ms = samples.last().map(|s| s.t_ms.saturating_sub(start)).unwrap_or(0);
    let active_ms: u64 = buckets.iter().map(|b| b.4).sum();
    let avg_bitrate = (active_ms > 0).then(|| bytes as f64 * 8.0 / active_ms as f64);

    // Bitrate per full-ish bucket (ignore buckets under half length, e.g. the tail).
    let rates: Vec<f64> = buckets
        .iter()
        .filter(|b| b.4 >= bucket_ms / 2)
        .map(|b| b.3 as f64 * 8.0 / b.4 as f64)
        .collect();
    let min_bitrate = rates.iter().copied().reduce(f64::min);
    let variation = if rates.len() >= 3 {
        let mean = rates.iter().sum::<f64>() / rates.len() as f64;
        let var = rates.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / rates.len() as f64;
        (mean > 0.0).then(|| var.sqrt() / mean)
    } else {
        None
    };

    // Worst moments
    let mut worst = Vec::new();
    for (i, b) in buckets.iter().enumerate() {
        for (acc, kind) in [(b.2, LossKind::Network), (b.1, LossKind::Encoding), (b.0, LossKind::Rendering)] {
            if acc.total > 0 {
                let pct = acc.lost as f64 / acc.total as f64 * 100.0;
                if pct >= 1.0 {
                    worst.push(WorstMoment { offset_ms: i as u64 * bucket_ms, kind, loss_percent: (pct * 100.0).round() / 100.0 });
                }
            }
        }
    }
    worst.sort_by(|a, b| b.loss_percent.partial_cmp(&a.loss_percent).unwrap().then(a.offset_ms.cmp(&b.offset_ms)));
    worst.truncate(5);

    let n = samples.len().max(1) as f64;
    let avg_fps = (!samples.is_empty()).then(|| samples.iter().map(|s| s.active_fps as f64).sum::<f64>() / n);
    let avg_cpu = samples.iter().map(|s| s.cpu_percent as f64).sum::<f64>() / n;
    let peak_cpu = samples.iter().map(|s| s.cpu_percent as f64).fold(0.0, f64::max);

    let network_totals = has_network.then(|| network.totals());
    let enc = encode.totals();
    let ren = render.totals();

    // Findings
    let mut findings = Vec::new();
    let loss_sev = |pct: f64| {
        if pct >= 2.0 {
            Some(Severity::Critical)
        } else if pct >= 0.5 {
            Some(Severity::Major)
        } else if pct >= 0.1 {
            Some(Severity::Minor)
        } else {
            None
        }
    };
    if let Some(nt) = network_totals {
        if let Some(sev) = loss_sev(nt.percent) {
            let (t, c, f) = texts(AlertKind::Network, cfg.encoder_is_cpu);
            findings.push(Finding::new(ids::NETWORK_DROPS, Area::Stream, sev, t, c, f, FixKind::Manual).with_evidence("lost_percent", nt.percent));
        }
    }
    if let Some(sev) = loss_sev(enc.percent) {
        let (t, c, f) = texts(AlertKind::Encoding, cfg.encoder_is_cpu);
        findings.push(Finding::new(ids::ENCODING_LAG, Area::Stream, sev, t, c, f, FixKind::Guided).with_evidence("lost_percent", enc.percent));
    }
    if let Some(sev) = loss_sev(ren.percent) {
        let (t, c, f) = texts(AlertKind::Rendering, cfg.encoder_is_cpu);
        findings.push(Finding::new(ids::RENDERING_LAG, Area::Stream, sev, t, c, f, FixKind::Guided).with_evidence("lost_percent", ren.percent));
    }
    if reconnects > 0 {
        let (t, c, f) = texts(AlertKind::Reconnecting, cfg.encoder_is_cpu);
        findings.push(
            Finding::new(ids::RECONNECTS, Area::Stream, if reconnects >= 3 { Severity::Critical } else { Severity::Major }, t, c, f, FixKind::Manual)
                .with_evidence("reconnects", reconnects as f64),
        );
    }
    if let Some(v) = variation {
        if v > 0.25 {
            findings.push(
                Finding::new(
                    ids::BITRATE_UNSTABLE,
                    Area::Stream,
                    if v > 0.4 { Severity::Major } else { Severity::Minor },
                    "Your stream bitrate jumped around",
                    "Your sending speed kept changing during the stream, which shows up as quality going up and down for viewers.",
                    "Use CBR, an Ethernet cable, and a bitrate comfortably below your upload speed.",
                    FixKind::Guided,
                )
                .with_evidence("variation", (v * 100.0).round() / 100.0),
            );
        }
    }
    if let (Some(cfg_kbps), Some(avg)) = (cfg.configured_bitrate_kbps, avg_bitrate) {
        if avg < cfg_kbps as f64 * 0.8 {
            findings.push(
                Finding::new(
                    ids::BITRATE_LOW,
                    Area::Stream,
                    Severity::Minor,
                    "Your stream sent less than its set bitrate",
                    "On average the stream carried noticeably less than the bitrate set in OBS, so it looked worse than it could.",
                    "Check that your upload can carry the bitrate; if not, set a lower fixed bitrate so quality stays steady.",
                    FixKind::Guided,
                )
                .with_evidence("avg_kbps", avg.round())
                .with_evidence("configured_kbps", cfg_kbps as f64),
            );
        }
    }
    if let (Some(target), Some(fps)) = (cfg.target_fps, avg_fps) {
        if fps < target as f64 * 0.95 {
            findings.push(
                Finding::new(
                    ids::LOW_FPS,
                    Area::Stream,
                    Severity::Minor,
                    "OBS ran below your frame rate",
                    "OBS averaged fewer frames per second than your stream is set to, so motion looked less smooth.",
                    "Cap your game's FPS and close heavy background apps so OBS has room.",
                    FixKind::Guided,
                )
                .with_evidence("avg_fps", (fps * 10.0).round() / 10.0),
            );
        }
    }
    if cfg.encoder_is_cpu && peak_cpu >= 90.0 && avg_cpu >= 75.0 {
        findings.push(
            Finding::new(
                ids::CPU_HIGH,
                Area::Stream,
                Severity::Minor,
                "Your processor was maxed out",
                "With the processor encoder (x264), a CPU near 100% is the main cause of encoding lag and game stutter.",
                "Switch to your graphics card's encoder, or use a faster x264 preset.",
                FixKind::Guided,
            )
            .with_evidence("peak_cpu_percent", peak_cpu.round()),
        );
    }
    rank(&mut findings);
    let score = score_area(Area::Stream, &findings);

    StreamReport {
        duration_s: duration_ms as f64 / 1000.0,
        network: network_totals,
        encoding: enc,
        rendering: ren,
        avg_bitrate_kbps: avg_bitrate.map(|x| x.round()),
        min_bitrate_kbps: min_bitrate.map(|x| x.round()),
        bitrate_variation: variation.map(|v| (v * 1000.0).round() / 1000.0),
        reconnect_events: reconnects,
        avg_fps: avg_fps.map(|x| (x * 10.0).round() / 10.0),
        peak_cpu_percent: peak_cpu,
        avg_cpu_percent: (avg_cpu * 10.0).round() / 10.0,
        worst_moments: worst,
        findings,
        score,
    }
}
