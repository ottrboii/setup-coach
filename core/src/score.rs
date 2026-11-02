//! The friendly 0-100 score shown on the report card.

use crate::finding::{Area, Finding};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub area: Area,
    /// 0-100, higher is better.
    pub value: u8,
    /// Number of findings that cost points.
    pub issues: usize,
}

/// Score one area: start at 100, subtract severity weights with diminishing
/// returns so a long list of minor issues can't drive the score to zero.
pub fn score_area(area: Area, findings: &[Finding]) -> Score {
    let mut weights: Vec<f64> = findings
        .iter()
        .filter(|f| f.area == area)
        .map(|f| f.severity.weight())
        .filter(|w| *w > 0.0)
        .collect();
    weights.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let mut penalty = 0.0;
    for (i, w) in weights.iter().enumerate() {
        penalty += w * 0.85f64.powi(i as i32);
    }
    let value = (100.0 - penalty).clamp(5.0, 100.0).round() as u8;
    Score { area, value, issues: weights.len() }
}
