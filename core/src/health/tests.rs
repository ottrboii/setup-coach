use super::*;

#[derive(Clone, Copy)]
struct Cond {
    render_loss: f64,
    enc_loss: f64,
    net_loss: f64,
    kbps: f64,
    congestion: f32,
    reconnecting: bool,
    cpu: f32,
    fps: f32,
}

impl Default for Cond {
    fn default() -> Self {
        Cond { render_loss: 0.0, enc_loss: 0.0, net_loss: 0.0, kbps: 6000.0, congestion: 0.0, reconnecting: false, cpu: 30.0, fps: 60.0 }
    }
}

/// Simulate a stream at 60 fps sampled every second.
fn simulate(seconds: u64, cond: impl Fn(u64) -> Cond, with_network: bool) -> Vec<StatsSample> {
    let mut out = Vec::new();
    let mut s = StatsSample { network_dropped: with_network.then_some(0), network_total: with_network.then_some(0), ..Default::default() };
    let (mut fr, mut fo, mut fnw, mut bytes) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    out.push(s.clone());
    for sec in 1..=seconds {
        let c = cond(sec);
        s.t_ms = sec * 1000;
        s.render_total += 60;
        fr += 60.0 * c.render_loss;
        s.render_skipped = fr.round() as u64;
        s.output_total += 60;
        fo += 60.0 * c.enc_loss;
        s.output_skipped = fo.round() as u64;
        if with_network {
            s.network_total = Some(s.network_total.unwrap() + 60);
            fnw += 60.0 * c.net_loss;
            s.network_dropped = Some(fnw.round() as u64);
        }
        bytes += c.kbps * 1000.0 / 8.0;
        s.bytes_sent = bytes as u64;
        s.congestion = c.congestion;
        s.reconnecting = c.reconnecting;
        s.cpu_percent = c.cpu;
        s.active_fps = c.fps;
        out.push(s.clone());
    }
    out
}

fn run(cfg: MonitorConfig, samples: &[StatsSample]) -> (HealthMonitor, Vec<Alert>) {
    let mut m = HealthMonitor::new(cfg);
    let mut alerts = Vec::new();
    for s in samples {
        alerts.extend(m.ingest(s.clone()));
    }
    (m, alerts)
}

#[test]
fn healthy_stream_raises_nothing() {
    let samples = simulate(300, |_| Cond::default(), true);
    let (m, alerts) = run(MonitorConfig { configured_bitrate_kbps: Some(6000), ..Default::default() }, &samples);
    assert!(alerts.is_empty(), "{alerts:?}");
    assert!(m.active_alerts().is_empty());
    let w = m.window_stats();
    assert!((w.bitrate_kbps.unwrap() - 6000.0).abs() < 5.0);
}

#[test]
fn attributes_network_loss() {
    let samples = simulate(120, |t| if (30..90).contains(&t) { Cond { net_loss: 0.05, ..Default::default() } } else { Cond::default() }, true);
    let (_, alerts) = run(MonitorConfig::default(), &samples);
    let raised: Vec<_> = alerts.iter().filter(|a| a.raised).collect();
    assert_eq!(raised.len(), 1);
    assert_eq!(raised[0].kind, AlertKind::Network);
    assert_eq!(raised[0].title, "Your internet can't keep up");
    assert!(raised[0].t_ms > 30_000 && raised[0].t_ms < 40_000);
}

#[test]
fn congestion_alone_counts_as_network_problem_without_counters() {
    let samples = simulate(60, |t| if t > 10 { Cond { congestion: 0.6, ..Default::default() } } else { Cond::default() }, false);
    let (m, alerts) = run(MonitorConfig::default(), &samples);
    assert!(alerts.iter().any(|a| a.kind == AlertKind::Network && a.raised));
    assert_eq!(m.active_alerts(), vec![AlertKind::Network]);
}

#[test]
fn attributes_encoding_and_rendering_with_cpu_specific_advice() {
    let samples = simulate(60, |_| Cond { enc_loss: 0.03, ..Default::default() }, true);
    let (_, alerts) = run(MonitorConfig { encoder_is_cpu: true, ..Default::default() }, &samples);
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].kind, AlertKind::Encoding);
    assert!(alerts[0].cause.contains("x264"));
    let (_, alerts) = run(MonitorConfig::default(), &samples);
    assert!(alerts[0].cause.contains("graphics card"));

    let samples = simulate(60, |_| Cond { render_loss: 0.03, ..Default::default() }, true);
    let (_, alerts) = run(MonitorConfig::default(), &samples);
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].kind, AlertKind::Rendering);
    assert_eq!(alerts[0].title, "Your game is starving OBS");
}

#[test]
fn hysteresis_does_not_flap() {
    // Loss toggles every 5 s: once raised, the alert should stay raised.
    let samples = simulate(200, |t| if (t / 5) % 2 == 0 { Cond { enc_loss: 0.06, ..Default::default() } } else { Cond::default() }, true);
    let (m, alerts) = run(MonitorConfig::default(), &samples);
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert!(alerts[0].raised);
    assert_eq!(m.active_alerts(), vec![AlertKind::Encoding]);
}

#[test]
fn alert_clears_after_sustained_recovery() {
    let samples = simulate(200, |t| if t < 40 { Cond { enc_loss: 0.05, ..Default::default() } } else { Cond::default() }, true);
    let (m, alerts) = run(MonitorConfig::default(), &samples);
    assert_eq!(alerts.len(), 2);
    assert!(alerts[0].raised);
    assert!(!alerts[1].raised);
    assert_eq!(alerts[1].title, "Encoding caught up");
    // Window (30 s) must drain the loss, then clear_after (30 s) must pass.
    assert!(alerts[1].t_ms >= 40_000 + 30_000, "{}", alerts[1].t_ms);
    assert!(m.active_alerts().is_empty());
}

#[test]
fn single_bad_second_below_threshold_is_ignored() {
    // 30 frames lost once over a 30 s window of 1800 frames ≈ 1.7%... use 12 frames → 0.7%.
    let samples = simulate(120, |t| if t == 50 { Cond { enc_loss: 0.2, ..Default::default() } } else { Cond::default() }, true);
    let (_, alerts) = run(MonitorConfig::default(), &samples);
    assert!(alerts.is_empty(), "{alerts:?}");
}

#[test]
fn low_bitrate_alert() {
    let samples = simulate(120, |t| if t > 40 { Cond { kbps: 3000.0, ..Default::default() } } else { Cond::default() }, true);
    let (m, alerts) = run(MonitorConfig { configured_bitrate_kbps: Some(6000), ..Default::default() }, &samples);
    let a = alerts.iter().find(|a| a.kind == AlertKind::LowBitrate).unwrap();
    assert!(a.raised);
    assert!(m.active_alerts().contains(&AlertKind::LowBitrate));
    // No configured bitrate → no bitrate alerts.
    let (_, alerts) = run(MonitorConfig::default(), &samples);
    assert!(alerts.iter().all(|a| a.kind != AlertKind::LowBitrate));
}

#[test]
fn reconnect_is_immediate_both_ways() {
    let samples = simulate(60, |t| if (20..25).contains(&t) { Cond { reconnecting: true, kbps: 0.0, ..Default::default() } } else { Cond::default() }, true);
    let (_, alerts) = run(MonitorConfig::default(), &samples);
    let rc: Vec<_> = alerts.iter().filter(|a| a.kind == AlertKind::Reconnecting).collect();
    assert_eq!(rc.len(), 2);
    assert_eq!((rc[0].raised, rc[0].t_ms), (true, 20_000));
    assert_eq!((rc[1].raised, rc[1].t_ms), (false, 25_000));
}

#[test]
fn counter_reset_starts_fresh_window() {
    let mut samples = simulate(40, |_| Cond { enc_loss: 0.05, ..Default::default() }, true);
    // OBS restarted: counters go back to zero.
    let mut fresh = simulate(40, |_| Cond::default(), true);
    for s in fresh.iter_mut() {
        s.t_ms += 41_000;
    }
    samples.extend(fresh);
    let (m, _) = run(MonitorConfig::default(), &samples);
    let w = m.window_stats();
    assert_eq!(w.encoding_loss, Some(0.0));
}

#[test]
fn window_needs_enough_frames() {
    let samples = simulate(0, |_| Cond::default(), true);
    let (m, alerts) = run(MonitorConfig::default(), &samples);
    assert!(alerts.is_empty());
    assert_eq!(m.window_stats().encoding_loss, None);
}

// ---------------------------------------------------------------- report

#[test]
fn report_totals_and_clean_score() {
    let samples = simulate(600, |_| Cond::default(), true);
    let r = post_stream_report(&samples, &ReportConfig { configured_bitrate_kbps: Some(6000), target_fps: Some(60.0), ..Default::default() });
    assert_eq!(r.duration_s, 600.0);
    assert_eq!(r.encoding.total, 36_000);
    assert_eq!(r.encoding.lost, 0);
    assert_eq!(r.network.unwrap().total, 36_000);
    assert!((r.avg_bitrate_kbps.unwrap() - 6000.0).abs() <= 1.0);
    assert!(r.findings.is_empty(), "{:?}", r.findings);
    assert_eq!(r.score.value, 100);
    assert!(r.worst_moments.is_empty());
}

#[test]
fn report_attributes_losses_and_worst_moments() {
    let samples = simulate(600, |t| match t {
        100..=120 => Cond { net_loss: 0.2, ..Default::default() },
        300..=305 => Cond { render_loss: 0.5, ..Default::default() },
        _ => Cond::default(),
    }, true);
    let r = post_stream_report(&samples, &ReportConfig::default());
    let net = r.network.unwrap();
    assert_eq!(net.lost, 252); // 21 s * 60 * 0.2
    assert!(net.percent > 0.5);
    assert_eq!(r.rendering.lost, 180);
    let ids: Vec<&str> = r.findings.iter().map(|f| f.id.as_str()).collect();
    assert!(ids.contains(&ids::NETWORK_DROPS));
    assert!(ids.contains(&ids::RENDERING_LAG));
    assert!(!ids.contains(&ids::ENCODING_LAG));
    assert!(!r.worst_moments.is_empty() && r.worst_moments.len() <= 5);
    let w = r.worst_moments[0];
    assert_eq!(w.kind, LossKind::Rendering);
    assert_eq!(w.offset_ms, 300_000);
    assert!(r.worst_moments.windows(2).all(|p| p[0].loss_percent >= p[1].loss_percent));
    assert!(r.score.value < 100);
}

#[test]
fn report_severity_scales_with_loss() {
    let sev = |loss: f64| {
        let samples = simulate(300, |_| Cond { enc_loss: loss, ..Default::default() }, true);
        let r = post_stream_report(&samples, &ReportConfig::default());
        r.findings.iter().find(|f| f.id == ids::ENCODING_LAG).map(|f| f.severity)
    };
    assert_eq!(sev(0.0), None);
    assert_eq!(sev(0.002), Some(Severity::Minor));
    assert_eq!(sev(0.01), Some(Severity::Major));
    assert_eq!(sev(0.05), Some(Severity::Critical));
}

#[test]
fn report_reconnects_bitrate_fps_cpu() {
    let samples = simulate(300, |t| {
        let mut c = Cond { cpu: 95.0, fps: 50.0, ..Default::default() };
        if (50..55).contains(&t) || (150..152).contains(&t) {
            c.reconnecting = true;
        }
        c.kbps = if t % 20 < 10 { 2000.0 } else { 6000.0 };
        c
    }, true);
    let r = post_stream_report(&samples, &ReportConfig { configured_bitrate_kbps: Some(6000), target_fps: Some(60.0), encoder_is_cpu: true, ..Default::default() });
    assert_eq!(r.reconnect_events, 2);
    let ids: Vec<&str> = r.findings.iter().map(|f| f.id.as_str()).collect();
    for id in [ids::RECONNECTS, ids::BITRATE_UNSTABLE, ids::BITRATE_LOW, ids::LOW_FPS, ids::CPU_HIGH] {
        assert!(ids.contains(&id), "missing {id}: {ids:?}");
    }
    assert!(r.bitrate_variation.unwrap() > 0.4);
    assert_eq!(r.min_bitrate_kbps, Some(2000.0));
    // Ranked most severe first.
    assert!(r.findings.windows(2).all(|p| p[0].severity >= p[1].severity));
}

#[test]
fn report_handles_counter_reset_and_missing_network() {
    let mut samples = simulate(100, |_| Cond { enc_loss: 0.01, ..Default::default() }, false);
    let mut second = simulate(100, |_| Cond::default(), false);
    for s in second.iter_mut() {
        s.t_ms += 101_000;
    }
    samples.extend(second);
    let r = post_stream_report(&samples, &ReportConfig::default());
    assert!(r.network.is_none());
    assert_eq!(r.encoding.total, 12_000);
    assert_eq!(r.encoding.lost, 60);
    assert_eq!(r.encoding.percent, 0.5);
}

#[test]
fn report_of_empty_input() {
    let r = post_stream_report(&[], &ReportConfig::default());
    assert_eq!(r.duration_s, 0.0);
    assert!(r.findings.is_empty());
    assert_eq!(r.score.value, 100);
}

#[test]
fn report_serializes() {
    let samples = simulate(30, |_| Cond::default(), true);
    let r = post_stream_report(&samples, &ReportConfig::default());
    let v = serde_json::to_value(&r).unwrap();
    assert!(v["score"]["value"].is_number());
}
