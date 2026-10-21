//! Model pricing and per-request cost.

use serde::{Deserialize, Serialize};

use crate::model::Usage;

const EMBEDDED: &str = include_str!("../assets/pricing.toml");

/// Fallback when a model is unknown: Opus-tier prices, flagged as estimated.
const FALLBACK: ModelPrice = ModelPrice {
    matcher: String::new(),
    label: String::new(),
    input: 5.0,
    output: 25.0,
    cache_read: 0.5,
    cache_write_5m: None,
    cache_write_1h: None,
    context: 200_000,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPrice {
    #[serde(rename = "match")]
    pub matcher: String,
    #[serde(default)]
    pub label: String,
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_5m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h: Option<f64>,
    #[serde(default = "default_context")]
    pub context: u64,
}

fn default_context() -> u64 {
    200_000
}

impl ModelPrice {
    pub fn cost(&self, u: &Usage) -> f64 {
        let w5 = self.cache_write_5m.unwrap_or(self.input * 1.25);
        let w1 = self.cache_write_1h.unwrap_or(self.input * 2.0);
        (u.input as f64 * self.input
            + u.output as f64 * self.output
            + u.cache_read as f64 * self.cache_read
            + u.cache_write_5m as f64 * w5
            + u.cache_write_1h as f64 * w1)
            / 1_000_000.0
    }
}

#[derive(Debug, Deserialize)]
struct PricingFile {
    model: Vec<ModelPrice>,
}

#[derive(Debug, Clone)]
pub struct Pricing {
    models: Vec<ModelPrice>,
}

impl Default for Pricing {
    fn default() -> Self {
        let file: PricingFile = toml::from_str(EMBEDDED).expect("embedded pricing.toml is valid");
        Pricing { models: file.model }
    }
}

impl Pricing {
    /// Embedded table plus user overrides; an override with the same `match` replaces the entry.
    pub fn with_overrides(overrides: &[ModelPrice]) -> Self {
        let mut p = Pricing::default();
        for o in overrides {
            if let Some(existing) = p.models.iter_mut().find(|m| m.matcher == o.matcher) {
                *existing = o.clone();
            } else {
                p.models.push(o.clone());
            }
        }
        p
    }

    /// Longest-prefix match on the normalised id.
    pub fn lookup(&self, model: &str) -> Option<&ModelPrice> {
        let id = normalize_model(model);
        self.models
            .iter()
            .filter(|m| id.starts_with(&m.matcher))
            .max_by_key(|m| m.matcher.len())
    }

    /// Returns (cost, estimated).
    pub fn cost(&self, model: &str, usage: &Usage) -> (f64, bool) {
        match self.lookup(model) {
            Some(p) => (p.cost(usage), false),
            None if model.is_empty() || model == "<synthetic>" => (0.0, false),
            None => (FALLBACK.cost(usage), true),
        }
    }

    pub fn context_window(&self, model: &str) -> u64 {
        self.lookup(model).map_or(FALLBACK.context, |p| p.context)
    }

    pub fn label(&self, model: &str) -> String {
        match self.lookup(model) {
            Some(p) if !p.label.is_empty() => p.label.clone(),
            _ => pretty_model(model),
        }
    }
}

/// Strip provider prefixes, date suffixes and context markers:
/// `us.anthropic.claude-opus-4-5-20251101-v1:0[1m]` becomes `claude-opus-4-5`.
pub fn normalize_model(model: &str) -> String {
    let mut s = model.trim().to_ascii_lowercase();
    if let Some(i) = s.find('[') {
        s.truncate(i);
    }
    if let Some(i) = s.find("claude-") {
        s = s[i..].to_string();
    }
    if let Some(i) = s.find('@') {
        s.truncate(i);
    }
    // Drop a trailing -YYYYMMDD and anything after it.
    let parts: Vec<&str> = s.split('-').collect();
    if let Some(pos) = parts.iter().position(|p| p.len() == 8 && p.chars().all(|c| c.is_ascii_digit())) {
        s = parts[..pos].join("-");
    }
    s
}

/// `claude-opus-5-5` becomes `Opus 5.5` for unknown models.
pub fn pretty_model(model: &str) -> String {
    let id = normalize_model(model);
    let rest = id.strip_prefix("claude-").unwrap_or(&id);
    let mut parts = rest.split('-');
    let Some(family) = parts.next() else {
        return model.to_string();
    };
    let ver: Vec<&str> = parts.collect();
    let mut name = String::new();
    let mut chars = family.chars();
    if let Some(c) = chars.next() {
        name.extend(c.to_uppercase());
        name.push_str(chars.as_str());
    }
    if !ver.is_empty() {
        name.push(' ');
        name.push_str(&ver.join("."));
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_provider_and_dates() {
        assert_eq!(normalize_model("claude-opus-5-5"), "claude-opus-5-5");
        assert_eq!(normalize_model("claude-haiku-4-5-20251001"), "claude-haiku-4-5");
        assert_eq!(normalize_model("us.anthropic.claude-opus-4-5-20251101-v1:0"), "claude-opus-4-5");
        assert_eq!(normalize_model("claude-opus-4-5@20251101"), "claude-opus-4-5");
        assert_eq!(normalize_model("claude-fable-5-1[1m]"), "claude-fable-5-1");
    }

    #[test]
    fn longest_prefix_wins() {
        let p = Pricing::default();
        assert_eq!(p.lookup("claude-fable-5-1").unwrap().cache_read, 0.25);
        assert_eq!(p.lookup("claude-fable-5").unwrap().cache_read, 1.0);
        assert_eq!(p.lookup("claude-opus-5-5").unwrap().input, 4.0);
        assert_eq!(p.lookup("claude-opus-5").unwrap().input, 5.0);
        assert_eq!(p.lookup("claude-opus-4-1-20250805").unwrap().input, 15.0);
        assert_eq!(p.context_window("claude-haiku-4-5-20251001"), 200_000);
    }

    #[test]
    fn cost_matches_hand_calculation() {
        // The assistant turn observed in this session's transcript.
        let u = Usage {
            input: 2,
            output: 1915,
            cache_read: 25235,
            cache_write_1h: 36262,
            ..Default::default()
        };
        let (c, est) = Pricing::default().cost("claude-fable-5-1", &u);
        let expected = (2.0 * 10.0 + 1915.0 * 50.0 + 25235.0 * 0.25 + 36262.0 * 20.0) / 1e6;
        assert!(!est);
        assert!((c - expected).abs() < 1e-9, "{c} vs {expected}");
    }

    #[test]
    fn unknown_model_is_estimated() {
        let (_, est) = Pricing::default().cost("gpt-9", &Usage { input: 1, ..Default::default() });
        assert!(est);
        let (c, est) = Pricing::default().cost("<synthetic>", &Usage::default());
        assert_eq!((c, est), (0.0, false));
    }

    #[test]
    fn overrides_replace_entries() {
        let p = Pricing::with_overrides(&[ModelPrice {
            matcher: "claude-opus-5-5".into(),
            label: "Opus 5.5".into(),
            input: 1.0,
            output: 2.0,
            cache_read: 0.1,
            cache_write_5m: None,
            cache_write_1h: None,
            context: 1_000_000,
        }]);
        assert_eq!(p.lookup("claude-opus-5-5").unwrap().input, 1.0);
    }

    #[test]
    fn pretty_names() {
        assert_eq!(pretty_model("claude-zeta-9-1"), "Zeta 9.1");
        assert_eq!(Pricing::default().label("claude-sonnet-5-5"), "Sonnet 5.5");
    }
}
