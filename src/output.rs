//! Output formats: text, JSON and SARIF 2.1.0.

use crate::config::Level;
use crate::engine::{Outcome, Reported};
use anyhow::Result;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Output format selected with `--format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    /// One line per finding and a summary per rule.
    Text,
    /// Stable-order JSON for scripts.
    Json,
    /// SARIF 2.1.0 for code-scanning UIs.
    Sarif,
}

/// Facts about the run that go into JSON and SARIF output.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ReportMeta {
    /// verus-lint version.
    pub tool_version: String,
    /// Fact schema version of the database.
    pub schema_version: String,
    /// Verus commit that produced the facts.
    pub verus_commit: String,
    /// Source commit the facts were extracted from.
    pub source_commit: String,
}

#[derive(Serialize)]
struct JsonFinding<'a> {
    rule: &'a str,
    level: &'a str,
    severity: &'a str,
    entity: &'a str,
    message: &'a str,
    file: Option<&'a str>,
    line: Option<i64>,
    metric: Option<f64>,
    properties: &'a BTreeMap<String, String>,
    baselined: bool,
}

#[derive(Serialize)]
struct JsonRule<'a> {
    id: &'a str,
    level: &'a str,
    severity: &'a str,
    summary: &'a str,
    findings: usize,
    new: usize,
    baselined: usize,
    fixed: &'a [String],
}

fn counts(o: &Outcome, id: &str) -> (usize, usize) {
    let all = o.findings.iter().filter(|r| r.finding.rule == id);
    let base = all.clone().filter(|r| r.baselined).count();
    (all.count() - base, base)
}

fn json_finding(r: &Reported) -> JsonFinding<'_> {
    let f = &r.finding;
    JsonFinding {
        rule: &f.rule,
        level: r.level.as_str(),
        severity: &f.severity,
        entity: &f.entity,
        message: &f.message,
        file: f.file.as_deref(),
        line: f.line,
        metric: f.metric,
        properties: &f.props,
        baselined: r.baselined,
    }
}

#[derive(Serialize)]
struct Doc<'a> {
    meta: &'a ReportMeta,
    rules: Vec<JsonRule<'a>>,
    findings: Vec<JsonFinding<'a>>,
    skipped: Vec<Value>,
    gate_failures: usize,
}

/// Render the outcome as JSON.
///
/// # Errors
/// Fails only if serialization does.
pub fn json(o: &Outcome, meta: &ReportMeta) -> Result<String> {
    let rules: Vec<JsonRule> = o
        .runs
        .iter()
        .map(|r| {
            let (new, baselined) = counts(o, &r.meta.id);
            JsonRule {
                id: &r.meta.id,
                level: r.level.as_str(),
                severity: r.meta.severity.as_str(),
                summary: &r.meta.summary,
                findings: new + baselined,
                new,
                baselined,
                fixed: &r.fixed,
            }
        })
        .collect();
    let findings: Vec<JsonFinding> = o.findings.iter().map(json_finding).collect();
    let skipped: Vec<Value> = o
        .skipped
        .iter()
        .map(|(r, why)| json!({"rule": r, "reason": why}))
        .collect();
    let mut s = serde_json::to_string_pretty(&Doc {
        meta,
        rules,
        findings,
        skipped,
        gate_failures: o.gate_failures(),
    })?;
    s.push('\n');
    Ok(s)
}

const fn sarif_level(severity: &str) -> &'static str {
    match severity.as_bytes() {
        b"error" => "error",
        b"note" => "note",
        _ => "warning",
    }
}

/// Render the outcome as a SARIF 2.1.0 log with one run.
///
/// # Errors
/// Fails only if serialization does.
pub fn sarif(o: &Outcome, meta: &ReportMeta) -> Result<String> {
    let rules: Vec<Value> = o
        .runs
        .iter()
        .map(|r| {
            json!({
                "id": r.meta.id,
                "shortDescription": {"text": r.meta.summary},
                "defaultConfiguration": {"level": sarif_level(r.meta.severity.as_str())},
            })
        })
        .collect();
    let results: Vec<Value> = o
        .findings
        .iter()
        .map(|r| {
            let f = &r.finding;
            let mut res = json!({
                "ruleId": f.rule,
                "level": sarif_level(&f.severity),
                "message": {"text": f.message},
                "partialFingerprints": {"entity": f.entity},
                "baselineState": if r.baselined { "unchanged" } else { "new" },
            });
            if let Some(file) = &f.file {
                let region = f
                    .line
                    .filter(|l| *l > 0)
                    .map_or_else(|| json!({}), |l| json!({"startLine": l}));
                res["locations"] = json!([{"physicalLocation": {
                    "artifactLocation": {"uri": file},
                    "region": region,
                }}]);
            }
            let mut props = serde_json::Map::new();
            if let Some(m) = f.metric {
                props.insert("metric".into(), json!(m));
            }
            for (k, v) in &f.props {
                props.insert(k.clone(), json!(v));
            }
            if !props.is_empty() {
                res["properties"] = Value::Object(props);
            }
            res
        })
        .collect();
    let doc = json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {"driver": {
                "name": "verus-lint",
                "version": meta.tool_version,
                "rules": rules,
            }},
            "properties": {
                "verusCommit": meta.verus_commit,
                "sourceCommit": meta.source_commit,
            },
            "results": results,
        }],
    });
    let mut s = serde_json::to_string_pretty(&doc)?;
    s.push('\n');
    Ok(s)
}

/// Render the outcome as text: findings the baseline does not cover (at most `top` per rule,
/// 0 for all), then one summary line per rule.
#[must_use]
#[allow(
    clippy::many_single_char_names,
    reason = "short local names in a formatter"
)]
pub fn text(o: &Outcome, top: usize) -> String {
    let mut s = String::new();
    for run in &o.runs {
        let shown: Vec<&Reported> = o
            .findings
            .iter()
            .filter(|r| r.finding.rule == run.meta.id && !r.baselined)
            .collect();
        let n = if top == 0 { shown.len() } else { top };
        for r in shown.iter().take(n) {
            let f = &r.finding;
            let loc = match (&f.file, f.line) {
                (Some(file), Some(l)) => format!("{file}:{l}: "),
                (Some(file), None) => format!("{file}: "),
                _ => String::new(),
            };
            let _ = writeln!(
                s,
                "{loc}{} {}: {} [{}]",
                f.severity, f.rule, f.message, f.entity
            );
        }
        if shown.len() > n {
            let _ = writeln!(s, "... {} more {} findings", shown.len() - n, run.meta.id);
        }
    }
    s.push('\n');
    for run in &o.runs {
        let (new, baselined) = counts(o, &run.meta.id);
        let _ = writeln!(
            s,
            "{} [{}]: {} new, {} baselined, {} fixed: {}",
            run.meta.id,
            run.level.as_str(),
            new,
            baselined,
            run.fixed.len(),
            run.meta.summary
        );
        for e in &run.fixed {
            let _ = writeln!(s, "  fixed: {e}");
        }
    }
    for (id, why) in &o.skipped {
        let _ = writeln!(s, "{id}: skipped ({why})");
    }
    let failures = o.gate_failures();
    if failures > 0 {
        let _ = writeln!(
            s,
            "FAILED: {failures} gated finding(s) not covered by the baseline"
        );
    } else if o.runs.iter().any(|r| r.level == Level::Gate) {
        let _ = writeln!(s, "ok: no gated findings outside the baseline");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::RuleRun;
    use crate::rules::Finding;
    use crate::sdk::{Ratchet, RuleMeta};

    fn outcome() -> Outcome {
        let meta = RuleMeta::new("t/x", "summary").ratchet(Ratchet::Set);
        let f = |e: &str, base| Reported {
            finding: Finding {
                rule: "t/x".into(),
                severity: "warning".into(),
                ..Finding::new(e, "msg").location("src/a.rs", 7).metric(2.0)
            },
            level: Level::Gate,
            baselined: base,
        };
        Outcome {
            runs: vec![RuleRun {
                meta,
                level: Level::Gate,
                fixed: vec!["gone".into()],
            }],
            findings: vec![f("a", false), f("b", true)],
            skipped: vec![],
        }
    }

    #[test]
    fn text_shows_only_uncovered() {
        let t = text(&outcome(), 20);
        assert!(t.contains("src/a.rs:7: warning t/x: msg [a]"));
        assert!(!t.contains("[b]"));
        assert!(t.contains("t/x [gate]: 1 new, 1 baselined, 1 fixed: summary"));
        assert!(t.contains("FAILED: 1 gated"));
    }

    #[test]
    fn json_fields_and_sarif_shape() {
        let m = ReportMeta::default();
        let j: Value = serde_json::from_str(&json(&outcome(), &m).unwrap()).unwrap();
        assert_eq!(j["findings"][1]["baselined"], true);
        assert_eq!(j["findings"][0]["line"], 7);
        assert_eq!(j["gate_failures"], 1);
        let s: Value = serde_json::from_str(&sarif(&outcome(), &m).unwrap()).unwrap();
        assert_eq!(s["version"], "2.1.0");
        let r = &s["runs"][0]["results"][0];
        assert_eq!(r["partialFingerprints"]["entity"], "a");
        assert_eq!(
            r["locations"][0]["physicalLocation"]["region"]["startLine"],
            7
        );
        assert_eq!(s["runs"][0]["tool"]["driver"]["rules"][0]["id"], "t/x");
    }
}
