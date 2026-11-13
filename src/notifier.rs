//! Desktop notifications when utilisation crosses a threshold (75% / 90%).

use std::collections::HashMap;

/// Detects upward threshold crossings per scope. The first observation only
/// sets a baseline; a drop (window reset) re-arms the thresholds.
#[derive(Debug, Default)]
pub struct CrossingDetector {
    thresholds: Vec<f64>,
    last: HashMap<&'static str, f64>,
}

impl CrossingDetector {
    pub fn new(thresholds: &[f64]) -> Self {
        let mut t: Vec<f64> = thresholds
            .iter()
            .copied()
            .filter(|t| *t > 0.0 && *t <= 1.0)
            .collect();
        t.sort_by(f64::total_cmp);
        Self {
            thresholds: t,
            last: HashMap::new(),
        }
    }

    /// Thresholds crossed (prev < t <= cur) since the previous call for *scope*.
    pub fn check(&mut self, scope: &'static str, util: f64) -> Vec<f64> {
        match self.last.insert(scope, util) {
            None => Vec::new(),
            Some(prev) => self
                .thresholds
                .iter()
                .copied()
                .filter(|t| prev < *t && *t <= util)
                .collect(),
        }
    }
}

/// Fire-and-forget notification on a background thread (never blocks the UI).
pub fn send(title: String, body: String) {
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new()
            .appname("AgentGauge")
            .summary(&title)
            .body(&body)
            .icon("dialog-warning")
            .show();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossing_logic() {
        let mut d = CrossingDetector::new(&[0.9, 0.75, 1.5]);
        assert!(d.check("s", 0.8).is_empty()); // baseline only
        assert!(d.check("s", 0.85).is_empty());
        assert_eq!(d.check("s", 0.95), vec![0.9]);
        assert!(d.check("s", 0.97).is_empty());
        assert!(d.check("s", 0.1).is_empty()); // reset re-arms
        assert_eq!(d.check("s", 0.92), vec![0.75, 0.9]);
    }
}
