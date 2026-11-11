//! Estimated energy of token usage, in watt-hours.
//!
//! This is an ESTIMATE built on assumed rates, never a measurement.
//! Anthropic publishes no per-token energy figure; the rates below are
//! clankmon's own order-of-magnitude guesses. Derivation: decode costs
//! about 0.5 mWh per output token for a Sonnet-class model (from the Epoch AI
//! anchor in [`SOURCE`]); prefill is 0.2x that; a cache write is prefill
//! +25%; a cache read is 0.02x (KV load, no recompute). Other tiers scale by
//! x0.3 (Haiku), x1 (Sonnet), x2 (Opus), x3 (Fable, Mythos). Datacenter
//! overhead (PUE) is implicit in the anchor.

use crate::model::Usage;
use crate::pricing::normalize_model;

pub const SOURCE: &str = "clankmon v1 assumption; anchor: Epoch AI 2025, ~0.3 Wh/chat query";

/// Watt-hours per million tokens, by token class.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnergyRate {
    pub matcher: &'static str,
    pub input: f64,
    pub cache_write: f64,
    pub cache_read: f64,
    pub output: f64,
}

impl EnergyRate {
    /// Thinking tokens are a subset of `output`, so they are not added again.
    /// Both cache TTLs share one rate.
    pub fn wh(&self, u: &Usage) -> f64 {
        (self.input * u.input as f64
            + self.cache_write * u.cache_write() as f64
            + self.cache_read * u.cache_read as f64
            + self.output * u.output as f64)
            / 1e6
    }
}

const fn row(matcher: &'static str, input: f64, cache_write: f64, cache_read: f64, output: f64) -> EnergyRate {
    EnergyRate { matcher, input, cache_write, cache_read, output }
}

const TABLE: &[EnergyRate] = &[
    row("claude-haiku", 30.0, 38.0, 3.0, 150.0),
    row("claude-sonnet", 100.0, 125.0, 10.0, 500.0),
    row("claude-opus", 200.0, 250.0, 20.0, 1000.0),
    row("claude-fable", 300.0, 375.0, 30.0, 1500.0),
    row("claude-mythos", 300.0, 375.0, 30.0, 1500.0),
];

/// Unknown models are assumed to be Opus-class, like the price fallback.
const FALLBACK: EnergyRate = row("", 200.0, 250.0, 20.0, 1000.0);

/// The rate row for a known model family (longest matching prefix).
pub fn rate(model: &str) -> Option<&'static EnergyRate> {
    let id = normalize_model(model);
    TABLE.iter().filter(|r| id.starts_with(r.matcher)).max_by_key(|r| r.matcher.len())
}

pub fn estimate_wh(model: &str, u: &Usage) -> f64 {
    if model.is_empty() || model == "<synthetic>" {
        return 0.0;
    }
    rate(model).unwrap_or(&FALLBACK).wh(u)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(n: u64) -> Usage {
        Usage { output: n, ..Default::default() }
    }

    #[test]
    fn families_match_by_longest_prefix() {
        assert_eq!(rate("claude-opus-5-5-20260101").unwrap().matcher, "claude-opus");
        assert_eq!(rate("claude-sonnet-5-5").unwrap().matcher, "claude-sonnet");
        assert_eq!(rate("claude-haiku-5").unwrap().matcher, "claude-haiku");
        assert_eq!(rate("claude-fable-5-1").unwrap().matcher, "claude-fable");
        assert_eq!(rate("claude-mythos-5").unwrap().matcher, "claude-mythos");
        assert!(rate("gpt-9").is_none());
    }

    #[test]
    fn hand_calculation_for_fable() {
        let u = Usage { input: 2, output: 1915, cache_read: 25235, cache_write_1h: 36262, ..Default::default() };
        let want = (2.0 * 300.0 + 36262.0 * 375.0 + 25235.0 * 30.0 + 1915.0 * 1500.0) / 1e6;
        assert!((estimate_wh("claude-fable-5-1", &u) - want).abs() < 1e-12);
    }

    #[test]
    fn empty_and_synthetic_are_zero() {
        assert_eq!(estimate_wh("", &out(1000)), 0.0);
        assert_eq!(estimate_wh("<synthetic>", &out(1000)), 0.0);
    }

    #[test]
    fn unknown_models_use_the_opus_fallback() {
        let u = Usage { input: 10, output: 500, cache_read: 900, ..Default::default() };
        assert_eq!(estimate_wh("gpt-9", &u), estimate_wh("claude-opus-5-5", &u));
        assert!(estimate_wh("gpt-9", &u) > 0.0);
    }

    #[test]
    fn thinking_is_a_subset_of_output() {
        let a = Usage { output: 100, ..Default::default() };
        let b = Usage { output: 100, thinking: 80, ..Default::default() };
        assert_eq!(estimate_wh("claude-sonnet-5-5", &a), estimate_wh("claude-sonnet-5-5", &b));
    }

    #[test]
    fn cache_ttls_share_one_rate() {
        let a = Usage { cache_write_5m: 1000, ..Default::default() };
        let b = Usage { cache_write_1h: 1000, ..Default::default() };
        let w = estimate_wh("claude-sonnet-5-5", &a);
        assert!(w > 0.0);
        assert_eq!(w, estimate_wh("claude-sonnet-5-5", &b));
    }

    #[test]
    fn cache_reads_are_cheap_and_output_is_dear() {
        let m = "claude-sonnet-5-5";
        let read = estimate_wh(m, &Usage { cache_read: 1000, ..Default::default() });
        let input = estimate_wh(m, &Usage { input: 1000, ..Default::default() });
        assert!(read < input && input < estimate_wh(m, &out(1000)));
    }
}
