//! Microphone analysis: measure a guided ~60 s test recording, turn the
//! measurements into plain-language findings, and compute a static processing
//! chain (gain → high-pass → EQ → expander → compressor → de-esser → limiter).
//!
//! No AI processing anywhere: every number comes from plain signal measurement.

pub mod analyze;
pub mod chain;
pub mod dsp;
pub mod findings;
pub mod process;
pub mod synth;
pub mod targets;
pub mod tune;
pub mod vibration;
pub mod voice;

use serde::{Deserialize, Serialize};

pub use analyze::{AudioMetrics, analyze};
pub use chain::{ProcessingChain, compute_chain};
pub use findings::{MicContext, mic_findings};

/// The steps of the guided mic test. The app shows a prompt for each step and
/// records the time span it covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentKind {
    /// Stay silent (noise floor, hum).
    Silence,
    /// Read the on-screen text (plosives, sibilants, tone, level).
    Read,
    /// Laugh / shout as loud as you get on stream (headroom).
    Loud,
    /// Type on your keyboard while silent.
    Typing,
    /// Tap the desk once or twice.
    DeskTap,
    /// Talk while gently wiggling the mic cable.
    CableWiggle,
    /// Talk while leaning back and turning your head like on stream.
    Moving,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub kind: SegmentKind,
    pub start_s: f64,
    pub end_s: f64,
}

/// Raw capture, de-interleaved, samples in [-1, 1].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioBuffer {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
}

impl AudioBuffer {
    pub fn mono(sample_rate: u32, samples: Vec<f32>) -> Self {
        AudioBuffer { sample_rate, channels: vec![samples] }
    }

    pub fn len(&self) -> usize {
        self.channels.first().map(|c| c.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn duration_s(&self) -> f64 {
        self.len() as f64 / self.sample_rate as f64
    }
}

/// A recorded guided test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicTest {
    pub audio: AudioBuffer,
    pub segments: Vec<Segment>,
}

impl MicTest {
    /// Sample range for a time span, clamped to the buffer.
    pub fn range(&self, seg: &Segment) -> std::ops::Range<usize> {
        let sr = self.audio.sample_rate as f64;
        let len = self.audio.len();
        let a = ((seg.start_s * sr).max(0.0) as usize).min(len);
        let b = ((seg.end_s * sr).max(0.0) as usize).min(len);
        a..b.max(a)
    }

    pub fn has(&self, kind: SegmentKind) -> bool {
        self.segments.iter().any(|s| s.kind == kind)
    }
}

/// Everything the app shows after a mic scan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicReport {
    pub metrics: AudioMetrics,
    pub findings: Vec<crate::Finding>,
    pub chain: ProcessingChain,
    pub score: crate::score::Score,
    pub targets: Vec<targets::TargetCheck>,
    /// The setup profile the pass bars came from (`MicContext::profile`).
    #[serde(default)]
    pub profile: crate::SetupProfile,
}

/// One call for the app: analyse → findings (with constraints applied) →
/// processing chain → score → target checks. The pass bars follow
/// `ctx.profile`; so do the chain's documented dynamics fields
/// (`SetupProfile::tuning_for`). Voice tuning is a separate step (`tune::compose`).
pub fn scan(test: &MicTest, ctx: &MicContext, constraints: &[crate::Constraint]) -> MicReport {
    let metrics = analyze(test);
    let mut findings = mic_findings(&metrics, ctx);
    findings::apply_constraints(&mut findings, constraints);
    ctx.profile.filter_findings(&mut findings);
    crate::finding::rank(&mut findings);
    let mut chain = compute_chain(&metrics, ctx, constraints);
    if !metrics.no_signal {
        process::calibrate_makeup(test, &mut chain);
    }
    let score = crate::score::score_area(crate::Area::Mic, &findings);
    let targets = targets::check_targets_for(&metrics, ctx.profile);
    MicReport { metrics, findings, chain, score, targets, profile: ctx.profile }
}
