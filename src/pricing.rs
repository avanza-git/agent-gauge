//! USD cost estimation. Prices are USD per million tokens, ported verbatim
//! from the reference widget's `pricing.py` (Anthropic public pricing, 2026).
//!
//! Cache rates: cache_read is tabled per model, cache_creation is the
//! 5-minute-TTL write rate, and 1-hour-TTL writes bill at 2x input.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::stats::TokenBucket;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_creation: f64,
}

#[rustfmt::skip]
const MODEL_PRICING: &[(&str, Rates)] = &[
    ("claude-opus-4-8", Rates { input: 5.0, output: 25.0, cache_read: 0.5, cache_creation: 6.25 }),
    ("claude-opus-5", Rates { input: 5.0, output: 25.0, cache_read: 0.5, cache_creation: 6.25 }),
    ("claude-opus-5-5", Rates { input: 4.0, output: 20.0, cache_read: 0.2, cache_creation: 5.0 }),
    ("claude-opus-4-7", Rates { input: 5.0, output: 25.0, cache_read: 0.5, cache_creation: 6.25 }),
    ("claude-opus-4-6", Rates { input: 5.0, output: 25.0, cache_read: 0.5, cache_creation: 6.25 }),
    ("claude-sonnet-5-5", Rates { input: 2.0, output: 10.0, cache_read: 0.1, cache_creation: 2.5 }),
    ("claude-sonnet-5", Rates { input: 2.0, output: 10.0, cache_read: 0.2, cache_creation: 2.5 }),
    ("claude-sonnet-4-6", Rates { input: 3.0, output: 15.0, cache_read: 0.3, cache_creation: 3.75 }),
    ("claude-fable-5", Rates { input: 10.0, output: 50.0, cache_read: 1.0, cache_creation: 12.5 }),
    ("claude-fable-5-1", Rates { input: 10.0, output: 50.0, cache_read: 0.25, cache_creation: 12.5 }),
    ("claude-mythos-5-1", Rates { input: 10.0, output: 50.0, cache_read: 0.25, cache_creation: 12.5 }),
    ("claude-mythos-5", Rates { input: 10.0, output: 50.0, cache_read: 1.0, cache_creation: 12.5 }),
    ("claude-opus-4-5", Rates { input: 5.0, output: 25.0, cache_read: 0.5, cache_creation: 6.25 }),
    ("claude-opus-4-1", Rates { input: 15.0, output: 75.0, cache_read: 1.5, cache_creation: 18.75 }),
    ("claude-opus-4", Rates { input: 15.0, output: 75.0, cache_read: 1.5, cache_creation: 18.75 }),
    ("claude-opus-4-0", Rates { input: 15.0, output: 75.0, cache_read: 1.5, cache_creation: 18.75 }),
    ("claude-sonnet-4-5", Rates { input: 3.0, output: 15.0, cache_read: 0.3, cache_creation: 3.75 }),
    ("claude-sonnet-4", Rates { input: 3.0, output: 15.0, cache_read: 0.3, cache_creation: 3.75 }),
    ("claude-sonnet-4-0", Rates { input: 3.0, output: 15.0, cache_read: 0.3, cache_creation: 3.75 }),
    ("claude-haiku-5-5", Rates { input: 0.1, output: 0.5, cache_read: 0.01, cache_creation: 0.125 }),
    ("claude-haiku-5-5[>100k]", Rates { input: 0.5, output: 2.5, cache_read: 0.05, cache_creation: 0.625 }),
    ("claude-haiku-4-5", Rates { input: 1.0, output: 5.0, cache_read: 0.1, cache_creation: 1.25 }),
    ("claude-haiku-4-5-20251001", Rates { input: 1.0, output: 5.0, cache_read: 0.1, cache_creation: 1.25 }),
    ("<synthetic>", Rates { input: 0.0, output: 0.0, cache_read: 0.0, cache_creation: 0.0 }),
    ("unknown", Rates { input: 0.0, output: 0.0, cache_read: 0.0, cache_creation: 0.0 }),
];

const FALLBACK_MODEL: &str = "claude-sonnet-4-6";

/// Family token → representative model for unknown point releases.
const FAMILY_FALLBACK: &[(&str, &str)] = &[
    ("opus", "claude-opus-5"),
    ("fable", "claude-fable-5"),
    ("mythos", "claude-mythos-5"),
    ("sonnet", "claude-sonnet-5"),
    ("haiku", "claude-haiku-4-5-20251001"),
];

/// Models billed at a second rate card when one request's prompt exceeds a threshold.
const LONG_PROMPT_TIERS: &[(&str, u64, &str)] =
    &[("claude-haiku-5-5", 100_000, "claude-haiku-5-5[>100k]")];

const PER_MILLION: f64 = 1_000_000.0;
const CACHE_WRITE_1H_MULTIPLIER: f64 = 2.0;

fn lookup(model: &str) -> Option<Rates> {
    MODEL_PRICING
        .iter()
        .find(|(name, _)| *name == model)
        .map(|(_, r)| *r)
}

/// Strip a trailing `-YYYYMMDD` snapshot date.
pub fn strip_date_suffix(model: &str) -> &str {
    let b = model.as_bytes();
    if b.len() > 9 {
        let tail = &b[b.len() - 9..];
        if tail[0] == b'-' && tail[1..].iter().all(u8::is_ascii_digit) {
            return &model[..model.len() - 9];
        }
    }
    model
}

/// Pricing key for ONE request: tiered models with a long prompt get their own key.
pub fn billing_model(model: &str, prompt_tokens: u64) -> String {
    let base = strip_date_suffix(model);
    for (name, threshold, key) in LONG_PROMPT_TIERS {
        if *name == base && prompt_tokens > *threshold {
            return (*key).to_string();
        }
    }
    model.to_string()
}

/// Rate card for *model* with the same fallback chain as the reference:
/// exact → date-stripped → family → generic Sonnet.
pub fn get_pricing(model: &str) -> Rates {
    if let Some(r) = lookup(model).or_else(|| lookup(strip_date_suffix(model))) {
        return r;
    }
    let lowered = model.to_ascii_lowercase();
    let fallback = FAMILY_FALLBACK
        .iter()
        .find(|(fam, _)| lowered.contains(fam))
        .map(|(_, m)| *m)
        .unwrap_or(FALLBACK_MODEL);
    lookup(fallback).expect("fallback model is tabled")
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Cost {
    pub total: f64,
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_creation: f64,
    /// What the cache-read tokens would have cost at the full input rate, minus what they did.
    pub cache_savings: f64,
}

impl std::ops::AddAssign for Cost {
    fn add_assign(&mut self, o: Self) {
        self.total += o.total;
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_creation += o.cache_creation;
        self.cache_savings += o.cache_savings;
    }
}

pub fn calculate_cost(model: &str, b: &TokenBucket) -> Cost {
    let p = get_pricing(model);
    let cache_1h = b.cache_creation_1h.min(b.cache_creation) as f64;
    let input = b.input as f64 * p.input / PER_MILLION;
    let output = b.output as f64 * p.output / PER_MILLION;
    let cache_read = b.cache_read as f64 * p.cache_read / PER_MILLION;
    let cache_creation = ((b.cache_creation as f64 - cache_1h) * p.cache_creation
        + cache_1h * p.input * CACHE_WRITE_1H_MULTIPLIER)
        / PER_MILLION;
    let cache_savings = b.cache_read as f64 * p.input / PER_MILLION - cache_read;
    Cost {
        total: input + output + cache_read + cache_creation,
        input,
        output,
        cache_read,
        cache_creation,
        cache_savings,
    }
}

/// Total cost plus the per-model breakdown.
pub fn calculate_stats_cost(
    by_model: &BTreeMap<String, TokenBucket>,
) -> (Cost, BTreeMap<String, Cost>) {
    let mut total = Cost::default();
    let mut per = BTreeMap::new();
    for (model, bucket) in by_model {
        let c = calculate_cost(model, bucket);
        total += c;
        per.insert(model.clone(), c);
    }
    (total, per)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bucket(input: u64, output: u64, cr: u64, cc: u64, cc1h: u64) -> TokenBucket {
        TokenBucket {
            input,
            output,
            cache_read: cr,
            cache_creation: cc,
            cache_creation_1h: cc1h,
        }
    }

    #[test]
    fn known_model_cost() {
        let c = calculate_cost("claude-sonnet-4-6", &bucket(1_000_000, 1_000_000, 0, 0, 0));
        assert!((c.total - 18.0).abs() < 1e-9);
    }

    #[test]
    fn date_suffix_and_family_fallback() {
        assert_eq!(
            strip_date_suffix("claude-sonnet-4-5-20250929"),
            "claude-sonnet-4-5"
        );
        assert_eq!(get_pricing("claude-sonnet-4-5-20250929").input, 3.0);
        // Unknown opus point release → opus-5 tier.
        assert_eq!(get_pricing("claude-opus-9-9").input, 5.0);
        // Totally unknown → sonnet-4-6.
        assert_eq!(get_pricing("gpt-something").input, 3.0);
    }

    #[test]
    fn cache_1h_billed_at_2x_input() {
        let c = calculate_cost("claude-sonnet-4-6", &bucket(0, 0, 0, 1_000_000, 1_000_000));
        assert!((c.cache_creation - 6.0).abs() < 1e-9);
        let c = calculate_cost("claude-sonnet-4-6", &bucket(0, 0, 0, 1_000_000, 0));
        assert!((c.cache_creation - 3.75).abs() < 1e-9);
    }

    #[test]
    fn cache_savings() {
        let c = calculate_cost("claude-sonnet-4-6", &bucket(0, 0, 1_000_000, 0, 0));
        assert!((c.cache_savings - 2.7).abs() < 1e-9);
    }

    #[test]
    fn long_prompt_tier() {
        assert_eq!(
            billing_model("claude-haiku-5-5", 100_001),
            "claude-haiku-5-5[>100k]"
        );
        assert_eq!(
            billing_model("claude-haiku-5-5", 100_000),
            "claude-haiku-5-5"
        );
        assert_eq!(billing_model("claude-opus-5-5", 500_000), "claude-opus-5-5");
    }
}
