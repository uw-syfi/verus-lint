//! Baseline file: the findings a project accepts today, compared by set or by metric.
//!
//! The file is written by `check --update-baseline` and committed. A rule with a set ratchet
//! stores the entities it reports; a rule with a metric ratchet stores each entity's metric.
//! Findings the baseline covers do not fail a gated rule; entities that disappear are
//! reported as fixed.

use crate::rules::Finding;
use crate::sdk::Ratchet;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Version of the baseline file format.
pub const BASELINE_SCHEMA: u32 = 1;

/// Baseline entries of one rule.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RuleBaseline {
    /// Accepted entities (set ratchet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<BTreeSet<String>>,
    /// Accepted metric per entity (metric ratchet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<BTreeMap<String, f64>>,
}

/// The whole file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Baseline {
    /// File format version.
    pub schema: u32,
    /// Verus commit of the extraction the baseline was taken from.
    pub verus_commit: String,
    /// Entries per rule id.
    pub rules: BTreeMap<String, RuleBaseline>,
}

impl Default for Baseline {
    fn default() -> Self {
        Self {
            schema: BASELINE_SCHEMA,
            verus_commit: String::new(),
            rules: BTreeMap::new(),
        }
    }
}

impl Baseline {
    /// Read a baseline file; a missing file is an empty baseline.
    ///
    /// # Errors
    /// Fails when the file exists but is unreadable, malformed or of another format version.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let b: Self =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if b.schema != BASELINE_SCHEMA {
            anyhow::bail!(
                "{}: baseline schema {} is not supported (expected {BASELINE_SCHEMA})",
                path.display(),
                b.schema
            );
        }
        Ok(b)
    }

    /// Write the file with stable ordering and a trailing newline.
    ///
    /// # Errors
    /// Fails on I/O errors.
    pub fn save(&self, path: &Path) -> Result<()> {
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
    }

    /// Whether the baseline accepts this finding of a rule with this ratchet.
    #[must_use]
    pub fn covers(&self, rule: &str, ratchet: Ratchet, f: &Finding) -> bool {
        let Some(b) = self.rules.get(rule) else {
            return false;
        };
        match ratchet {
            Ratchet::None => false,
            Ratchet::Set => b.set.as_ref().is_some_and(|s| s.contains(&f.entity)),
            Ratchet::Metric { .. } => {
                let (Some(m), Some(now)) = (b.metric.as_ref(), f.metric) else {
                    return false;
                };
                m.get(&f.entity)
                    .is_some_and(|&base| now <= ratchet.allowed(base))
            }
        }
    }

    /// Entities the baseline holds for a rule that `now` no longer reports.
    #[must_use]
    pub fn fixed(&self, rule: &str, now: &[Finding]) -> Vec<String> {
        let present: BTreeSet<&str> = now.iter().map(|f| f.entity.as_str()).collect();
        let Some(b) = self.rules.get(rule) else {
            return Vec::new();
        };
        let keys: Vec<&String> = match (&b.set, &b.metric) {
            (Some(s), _) => s.iter().collect(),
            (None, Some(m)) => m.keys().collect(),
            (None, None) => Vec::new(),
        };
        keys.into_iter()
            .filter(|k| !present.contains(k.as_str()))
            .cloned()
            .collect()
    }

    /// Replace one rule's entry with the current findings.
    pub fn record(&mut self, rule: &str, ratchet: Ratchet, now: &[Finding]) {
        let entry = match ratchet {
            Ratchet::None => {
                self.rules.remove(rule);
                return;
            }
            Ratchet::Set => RuleBaseline {
                set: Some(now.iter().map(|f| f.entity.clone()).collect()),
                metric: None,
            },
            Ratchet::Metric { .. } => {
                let mut m: BTreeMap<String, f64> = BTreeMap::new();
                for f in now {
                    let e = m.entry(f.entity.clone()).or_insert(f64::MIN);
                    *e = e.max(f.metric.unwrap_or(0.0));
                }
                RuleBaseline {
                    set: None,
                    metric: Some(m),
                }
            }
        };
        self.rules.insert(rule.to_string(), entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(entity: &str, metric: Option<f64>) -> Finding {
        Finding {
            entity: entity.into(),
            metric,
            ..Finding::default()
        }
    }

    #[test]
    fn set_ratchet() {
        let mut b = Baseline::default();
        b.record("r", Ratchet::Set, &[f("a", None), f("b", None)]);
        assert!(b.covers("r", Ratchet::Set, &f("a", None)));
        assert!(!b.covers("r", Ratchet::Set, &f("c", None)));
        assert!(!b.covers("other", Ratchet::Set, &f("a", None)));
        assert_eq!(b.fixed("r", &[f("b", None)]), ["a"]);
    }

    #[test]
    fn metric_ratchet() {
        let r = Ratchet::Metric {
            ratio: 1.2,
            abs: 5.0,
        };
        let mut b = Baseline::default();
        b.record("m", r, &[f("a", Some(100.0)), f("a", Some(90.0))]);
        assert!(b.covers("m", r, &f("a", Some(120.0))));
        assert!(!b.covers("m", r, &f("a", Some(121.0))));
        assert!(!b.covers("m", r, &f("new", Some(1.0))));
        assert_eq!(b.fixed("m", &[]), ["a"]);
    }

    #[test]
    fn round_trip_is_stable() {
        let mut b = Baseline {
            verus_commit: "abc".into(),
            ..Baseline::default()
        };
        b.record("z", Ratchet::Set, &[f("y", None), f("x", None)]);
        let dir = std::env::temp_dir().join(format!("vl-baseline-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("b.json");
        b.save(&p).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.find("\"x\"").unwrap() < text.find("\"y\"").unwrap());
        assert_eq!(Baseline::load(&p).unwrap(), b);
        assert_eq!(
            Baseline::load(&dir.join("missing.json")).unwrap(),
            Baseline::default()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
