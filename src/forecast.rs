//! Limit-exhaustion forecast: fit a usage rate to recent samples of the
//! current window and project when utilisation reaches 100%.

use serde::{Deserialize, Serialize};

use crate::history::Sample;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Forecast {
    /// Utilisation gained per hour (0.12 = +12%/h).
    pub rate_per_hour: f64,
    /// Unix seconds when the window hits 100%; 0 = not before it resets.
    pub exhausts_at: i64,
}

impl Forecast {
    /// True when the limit is projected to run out before the window resets.
    pub fn exhausts(&self) -> bool {
        self.exhausts_at > 0
    }

    /// Projected utilisation at *reset* (for "on pace, ~74% at reset").
    pub fn at_reset(&self, util: f64, now: i64, reset: i64) -> f64 {
        let hours = (reset - now).max(0) as f64 / 3600.0;
        (util + self.rate_per_hour * hours).min(1.0)
    }
}

pub struct Params {
    /// Only samples newer than `now - lookback` are fitted.
    pub lookback: f64,
    /// The fitted points must span at least this many seconds.
    pub min_span: f64,
}

pub const SESSION: Params = Params {
    lookback: 45.0 * 60.0,
    min_span: 10.0 * 60.0,
};
pub const WEEKLY: Params = Params {
    lookback: 24.0 * 3600.0,
    min_span: 2.0 * 3600.0,
};

/// Reset epochs from the API jitter by a few seconds between calls.
const RESET_TOLERANCE: i64 = 120;

/// True when a sample's reset epoch *r* belongs to the window ending at *reset*.
pub fn same_window(r: i64, reset: i64) -> bool {
    r > 0 && reset > 0 && (r - reset).abs() <= RESET_TOLERANCE
}

/// The `(ts, util)` points of *samples* that belong to the window ending at
/// *reset*. `pick` extracts `(util, reset)` from a sample; samples without a
/// reset epoch (legacy lines) are skipped.
pub fn window_points(
    samples: &[Sample],
    pick: impl Fn(&Sample) -> Option<(f64, i64)>,
    reset: i64,
) -> Vec<(f64, f64)> {
    samples
        .iter()
        .filter_map(|s| {
            let (util, r) = pick(s)?;
            (same_window(r, reset) && util.is_finite()).then_some((s.ts, util))
        })
        .collect()
}

/// Least-squares projection over *points* plus the current reading.
pub fn project(
    points: &[(f64, f64)],
    now: f64,
    util: f64,
    reset: i64,
    p: &Params,
) -> Option<Forecast> {
    if !util.is_finite() || util >= 1.0 || reset as f64 <= now {
        return None;
    }
    let mut pts: Vec<(f64, f64)> = points
        .iter()
        .copied()
        .filter(|(ts, u)| *ts >= now - p.lookback && *ts < now && u.is_finite())
        .collect();
    pts.push((now, util));
    let first = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    if pts.len() < 2 || now - first < p.min_span {
        return None;
    }
    let n = pts.len() as f64;
    let mean_t = pts.iter().map(|p| p.0).sum::<f64>() / n;
    let mean_u = pts.iter().map(|p| p.1).sum::<f64>() / n;
    let (mut num, mut den) = (0.0, 0.0);
    for (t, u) in &pts {
        num += (t - mean_t) * (u - mean_u);
        den += (t - mean_t) * (t - mean_t);
    }
    if den <= 0.0 {
        return None;
    }
    let slope = num / den; // per second
    if !slope.is_finite() || slope <= 0.0 {
        return None;
    }
    let eta = now + (1.0 - util) / slope;
    Some(Forecast {
        rate_per_hour: slope * 3600.0,
        exhausts_at: if eta < reset as f64 { eta as i64 } else { 0 },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 1_000_000.0;

    #[test]
    fn linear_rise_projects_eta() {
        // +10% per 10 min = 60%/h; at 50% → full in 50 min.
        let pts = [(NOW - 1200.0, 0.30), (NOW - 600.0, 0.40)];
        let f = project(&pts, NOW, 0.50, (NOW + 7200.0) as i64, &SESSION).unwrap();
        assert!((f.rate_per_hour - 0.6).abs() < 1e-9);
        assert_eq!(f.exhausts_at, (NOW + 3000.0) as i64);
        assert!(f.exhausts());
    }

    #[test]
    fn eta_after_reset_is_zero() {
        // Full in 50 min, but the window resets in 30 min.
        let pts = [(NOW - 1200.0, 0.30), (NOW - 600.0, 0.40)];
        let f = project(&pts, NOW, 0.50, (NOW + 1800.0) as i64, &SESSION).unwrap();
        assert_eq!(f.exhausts_at, 0);
        assert!(!f.exhausts());
        // Slow rise: 1%/10 min → 6%/h, 50% left needs ~8h; reset in 2h.
        let slow = [(NOW - 1200.0, 0.48), (NOW - 600.0, 0.49)];
        let f = project(&slow, NOW, 0.50, (NOW + 7200.0) as i64, &SESSION).unwrap();
        assert_eq!(f.exhausts_at, 0);
        assert!((f.at_reset(0.50, NOW as i64, (NOW + 7200.0) as i64) - 0.62).abs() < 1e-6);
    }

    #[test]
    fn needs_enough_span_and_points() {
        let reset = (NOW + 7200.0) as i64;
        assert!(project(&[], NOW, 0.5, reset, &SESSION).is_none());
        assert!(project(&[(NOW - 300.0, 0.4)], NOW, 0.5, reset, &SESSION).is_none());
        // Outside the lookback window.
        assert!(project(&[(NOW - 3600.0, 0.1)], NOW, 0.5, reset, &SESSION).is_none());
    }

    #[test]
    fn flat_falling_or_full_gives_none() {
        let reset = (NOW + 7200.0) as i64;
        let flat = [(NOW - 1200.0, 0.5), (NOW - 600.0, 0.5)];
        assert!(project(&flat, NOW, 0.5, reset, &SESSION).is_none());
        let falling = [(NOW - 1200.0, 0.7), (NOW - 600.0, 0.6)];
        assert!(project(&falling, NOW, 0.5, reset, &SESSION).is_none());
        let rising = [(NOW - 1200.0, 0.8), (NOW - 600.0, 0.9)];
        assert!(project(&rising, NOW, 1.0, reset, &SESSION).is_none());
        assert!(project(&rising, NOW, 0.95, NOW as i64 - 1, &SESSION).is_none());
    }

    #[test]
    fn window_points_drops_other_windows() {
        let reset = 50_000;
        let mk = |ts: f64, session: f64, sr: i64| Sample {
            ts,
            session,
            session_reset: sr,
            ..Default::default()
        };
        let samples = vec![
            mk(1.0, 0.9, 30_000),     // previous window
            mk(2.0, 0.1, 0),          // legacy, no reset
            mk(3.0, 0.2, reset - 30), // jitter
            mk(4.0, 0.3, reset),
        ];
        let pts = window_points(&samples, |s| Some((s.session, s.session_reset)), reset);
        assert_eq!(pts, vec![(3.0, 0.2), (4.0, 0.3)]);
        assert!(window_points(&samples, |s| Some((s.session, s.session_reset)), 0).is_empty());
    }
}
