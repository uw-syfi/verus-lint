//! SQL rule runner: header parsing, parameter substitution, finding collection.
//!
//! A rule is one `.sql` file: a `-- key: value` comment header and one SELECT.
//! The query must return `entity` and `message`; `file`, `line`, `metric` and
//! `severity` are optional; other columns are kept as properties.

use crate::sdk::{Function, Ratchet, RuleMeta, Severity};
use anyhow::{Context, Result, anyhow, bail};
use duckdb::Connection;
use duckdb::types::Value;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone)]
/// A parsed SQL rule file: header fields and the query text.
pub struct Rule {
    /// Rule id (`namespace/name`).
    pub id: String,
    /// One-line description.
    pub summary: String,
    /// Default severity text (`note`, `warning` or `error`).
    pub severity: String,
    /// Schema version range the rule was written against.
    pub schema: Option<String>,
    /// Parameter names with default values.
    pub params: Vec<(String, String)>,
    /// The `needs` header (`dynamic`), if any.
    pub needs: Option<String>,
    /// The `ratchet` header text, if any.
    pub ratchet: Option<String>,
    /// The `SELECT` statement.
    pub sql: String,
}

/// One result of a rule. `rule` and an empty `severity` are filled in by the runner.
#[derive(Debug, Clone, Default)]
pub struct Finding {
    /// Id of the rule that produced the finding.
    pub rule: String,
    /// Severity text; empty until the runner fills in the rule's default.
    pub severity: String,
    /// Stable key the baseline uses (a function path, never a line number).
    pub entity: String,
    /// One-line message.
    pub message: String,
    /// Source file, relative to the workspace.
    pub file: Option<String>,
    /// Source line.
    pub line: Option<i64>,
    /// Number compared by metric ratchets.
    pub metric: Option<f64>,
    /// Extra properties, kept in JSON and SARIF output.
    pub props: BTreeMap<String, String>,
}

impl Rule {
    /// The rule's static description.
    ///
    /// # Errors
    /// Fails on an invalid severity, ratchet or `needs` header.
    pub fn meta(&self) -> Result<RuleMeta> {
        let mut m = RuleMeta::new(&self.id, &self.summary)
            .severity(Severity::parse(&self.severity).with_context(|| self.id.clone())?);
        m.params.clone_from(&self.params);
        m.schema.clone_from(&self.schema);
        match self.needs.as_deref() {
            None => {}
            Some("dynamic") => m.needs_dynamic = true,
            Some(o) => bail!("rule {}: unknown needs `{o}`", self.id),
        }
        if let Some(r) = &self.ratchet {
            m.ratchet = Ratchet::parse(r).with_context(|| format!("rule {}", self.id))?;
        }
        Ok(m)
    }
}

impl Finding {
    /// A finding for `entity` (the stable key the baseline uses; never a line number).
    #[must_use]
    pub fn new(entity: &str, message: impl Into<String>) -> Self {
        Self {
            entity: entity.to_string(),
            message: message.into(),
            ..Self::default()
        }
    }

    /// A finding about a function: entity is its path, location is its definition.
    #[must_use]
    pub fn at(f: &Function, message: impl Into<String>) -> Self {
        Self::new(&f.path, message).location(&f.file, f.line)
    }

    /// Set the source location.
    #[must_use]
    pub fn location(mut self, file: &str, line: u32) -> Self {
        self.file = Some(file.to_string());
        self.line = Some(i64::from(line));
        self
    }

    /// Set the number compared by metric ratchets.
    #[must_use]
    pub const fn metric(mut self, m: f64) -> Self {
        self.metric = Some(m);
        self
    }

    /// Override the rule's severity for this finding.
    #[must_use]
    pub fn severity(mut self, s: Severity) -> Self {
        self.severity = s.as_str().to_string();
        self
    }

    /// Attach a property (kept in JSON and SARIF output).
    #[must_use]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "builder argument: any displayable value"
    )]
    pub fn prop(mut self, key: &str, value: impl ToString) -> Self {
        self.props.insert(key.to_string(), value.to_string());
        self
    }
}

/// The example rules shipped in `examples/rules` (for tests and documentation; the tool
/// itself loads no rules unless a directory is given).
///
/// # Errors
/// Fails when the operation's I/O, parsing or database step fails; the error says which.
pub fn examples() -> Result<Vec<Rule>> {
    load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/rules"))
}

/// Load all `*.sql` files of a directory, sorted by name.
///
/// # Errors
/// Fails when the operation's I/O, parsing or database step fails; the error says which.
pub fn load_dir(dir: &Path) -> Result<Vec<Rule>> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "sql"))
        .collect();
    files.sort();
    files
        .iter()
        .map(|p| parse_rule(&std::fs::read_to_string(p)?).with_context(|| p.display().to_string()))
        .collect()
}

/// Parse a SQL rule file (comment header plus query).
///
/// # Errors
/// Fails when the header is missing a required field.
pub fn parse_rule(text: &str) -> Result<Rule> {
    let mut hdr: BTreeMap<String, String> = BTreeMap::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("--") else {
            break;
        };
        if let Some((k, v)) = rest.split_once(':') {
            let k = k.trim();
            if matches!(
                k,
                "id" | "summary" | "severity" | "schema" | "params" | "needs" | "ratchet"
            ) {
                hdr.insert(k.to_string(), v.trim().to_string());
            }
        }
    }
    let id = hdr
        .remove("id")
        .ok_or_else(|| anyhow!("rule header lacks `id`"))?;
    let summary = hdr
        .remove("summary")
        .ok_or_else(|| anyhow!("rule {id} lacks `summary`"))?;
    let severity = hdr.remove("severity").unwrap_or_else(|| "warning".into());
    if !matches!(severity.as_str(), "note" | "warning" | "error") {
        bail!("rule {id}: bad severity {severity}");
    }
    let mut params = Vec::new();
    if let Some(p) = hdr.remove("params") {
        for kv in p.split(',') {
            let (k, v) = kv
                .split_once('=')
                .ok_or_else(|| anyhow!("rule {id}: bad param `{kv}`"))?;
            params.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Ok(Rule {
        id,
        summary,
        severity,
        schema: hdr.remove("schema"),
        params,
        needs: hdr.remove("needs"),
        ratchet: hdr.remove("ratchet"),
        sql: text.to_string(),
    })
}

/// Replace `param('name')` with the literal value (override, else header default).
///
/// # Errors
/// Fails when the operation's I/O, parsing or database step fails; the error says which.
pub fn substitute(rule: &Rule, overrides: &BTreeMap<String, String>) -> Result<String> {
    let mut sql = rule.sql.clone();
    let mut out = String::new();
    while let Some(i) = sql.find("param('") {
        out.push_str(&sql[..i]);
        let rest = &sql[i + 7..];
        let j = rest
            .find("')")
            .ok_or_else(|| anyhow!("rule {}: unterminated param()", rule.id))?;
        let name = &rest[..j];
        let v = overrides
            .get(name)
            .or_else(|| rule.params.iter().find(|(k, _)| k == name).map(|(_, v)| v))
            .ok_or_else(|| anyhow!("rule {}: param '{name}' has no default", rule.id))?;
        if v.parse::<f64>().is_err() {
            bail!(
                "rule {}: param '{name}' must be numeric, got `{v}`",
                rule.id
            );
        }
        out.push_str(v);
        sql = rest[j + 2..].to_string();
    }
    out.push_str(&sql);
    Ok(out)
}

/// Render a database value as text for messages.
#[must_use]
pub fn cell_text(v: &Value) -> String {
    text(v)
}

fn text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Boolean(b) => b.to_string(),
        Value::TinyInt(x) => x.to_string(),
        Value::SmallInt(x) => x.to_string(),
        Value::Int(x) => x.to_string(),
        Value::BigInt(x) => x.to_string(),
        Value::HugeInt(x) => x.to_string(),
        Value::UTinyInt(x) => x.to_string(),
        Value::USmallInt(x) => x.to_string(),
        Value::UInt(x) => x.to_string(),
        Value::UBigInt(x) => x.to_string(),
        Value::Float(x) => x.to_string(),
        Value::Double(x) => x.to_string(),
        Value::Text(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Null => None,
        Value::Text(s) => s.parse().ok(),
        o => text(o).parse().ok(),
    }
}

/// Run one rule and return its findings.
///
/// # Errors
/// Fails when the query is invalid or a parameter is missing.
#[allow(
    clippy::cast_possible_truncation,
    reason = "finding line numbers are small non-negative integers"
)]
pub fn run_rule(
    conn: &Connection,
    rule: &Rule,
    overrides: &BTreeMap<String, String>,
) -> Result<Vec<Finding>> {
    let sql = substitute(rule, overrides)?;
    let mut stmt = conn
        .prepare(&sql)
        .with_context(|| format!("rule {}: preparing SQL", rule.id))?;
    let mut rows = stmt
        .query([])
        .with_context(|| format!("rule {}: running SQL", rule.id))?;
    let names: Vec<String> = rows
        .as_ref()
        .map(duckdb::Statement::column_names)
        .unwrap_or_default();
    let col = |n: &str| names.iter().position(|x| x == n);
    let (Some(ce), Some(cm)) = (col("entity"), col("message")) else {
        bail!(
            "rule {}: result must have `entity` and `message` columns, got {names:?}",
            rule.id
        );
    };
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let get = |i: usize| r.get::<_, Value>(i);
        let mut f = Finding {
            rule: rule.id.clone(),
            severity: rule.severity.clone(),
            entity: text(&get(ce)?),
            message: text(&get(cm)?),
            ..Default::default()
        };
        for (i, n) in names.iter().enumerate() {
            let v = get(i)?;
            match n.as_str() {
                "entity" | "message" => {}
                "file" => f.file = Some(text(&v)).filter(|s| !s.is_empty()),
                "line" => f.line = num(&v).map(|x| x as i64),
                "metric" => f.metric = num(&v),
                "severity" => {
                    let s = text(&v);
                    if !s.is_empty() {
                        f.severity = s;
                    }
                }
                other => {
                    f.props.insert(other.to_string(), text(&v));
                }
            }
        }
        out.push(f);
    }
    Ok(out)
}

/// One-line text form of a finding.
#[must_use]
pub fn format_finding(f: &Finding) -> String {
    let loc = match (&f.file, f.line) {
        (Some(file), Some(l)) => format!("{file}:{l}: "),
        (Some(file), None) => format!("{file}: "),
        _ => String::new(),
    };
    format!(
        "{loc}{} {}: {} [{}]",
        f.severity, f.rule, f.message, f.entity
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{CrateInfo, Db};
    use crate::vir::{ImplNames, parse_log};

    #[test]
    fn header_and_params() {
        let r = parse_rule(
            "-- id: t/x\n-- summary: s\n-- params: a = 3, b = 4.5\nSELECT param('a') AS e",
        )
        .unwrap();
        assert_eq!(r.id, "t/x");
        assert_eq!(r.severity, "warning");
        assert_eq!(
            substitute(&r, &BTreeMap::new())
                .unwrap()
                .lines()
                .last()
                .unwrap(),
            "SELECT 3 AS e"
        );
        let ov = BTreeMap::from([("a".to_string(), "9".to_string())]);
        assert!(substitute(&r, &ov).unwrap().contains("SELECT 9 AS e"));
    }

    #[test]
    fn rejects_bad_headers_and_params() {
        assert!(parse_rule("-- summary: s\nSELECT 1").is_err());
        assert!(parse_rule("-- id: a\n-- summary: s\n-- severity: loud\nSELECT 1").is_err());
        let r = parse_rule("-- id: a\n-- summary: s\nSELECT param('zz')").unwrap();
        assert!(substitute(&r, &BTreeMap::new()).is_err());
        let r = parse_rule("-- id: a\n-- summary: s\n-- params: n = 1\nSELECT param('n')").unwrap();
        let ov = BTreeMap::from([("n".to_string(), "1; DROP".to_string())]);
        assert!(substitute(&r, &ov).is_err());
    }

    #[test]
    fn examples_parse() {
        let rs = examples().unwrap();
        assert_eq!(
            rs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            [
                "verus/dead-proof-code",
                "verus/extraction-health",
                "verus/fanin-open-spec",
                "verus/fanin-reveal",
                "verus/hotspot-growth",
                "verus/quantifier-auto-trigger",
                "verus/rlimit-headroom",
                "verus/rlimit-hotspot",
                "verus/seed-instability",
                "verus/spinoff-candidate",
                "verus/trait-spec-default",
                "verus/trusted-inventory",
                "verus/unused-public-api"
            ]
        );
    }

    fn fixture_db() -> Db {
        let text = include_str!("../tests/fixtures/mini.vir");
        let facts = parse_log(text, "mini", &ImplNames::new()).unwrap();
        let mut db = Db::in_memory().unwrap();
        db.load_crate(
            &facts,
            &CrateInfo {
                manifest: "Cargo.toml",
                log_bytes: 0,
            },
        )
        .unwrap();
        db.resolve().unwrap();
        db
    }

    #[test]
    fn fanin_rules_on_fixture() {
        let db = fixture_db();
        let rules: Vec<_> = examples()
            .unwrap()
            .into_iter()
            .filter(|r| r.id.starts_with("verus/fanin"))
            .collect();
        let ov = BTreeMap::from([("min_fns".to_string(), "1".to_string())]);
        let open = run_rule(&db.conn, &rules[0], &ov).unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].entity, "mini::m::open_a");
        assert_eq!(open[0].metric, Some(1.0));
        assert_eq!(
            (open[0].file.as_deref(), open[0].line),
            (Some("src/m.rs"), Some(3))
        );
        let rev = run_rule(&db.conn, &rules[1], &ov).unwrap();
        assert_eq!(rev.len(), 1);
        assert_eq!(rev[0].entity, "mini::m::opaque_b");
        assert!(rev[0].message.contains("opaque"));
        // Default threshold hides both.
        assert!(
            run_rule(&db.conn, &rules[0], &BTreeMap::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn missing_columns_error() {
        let db = fixture_db();
        let r = parse_rule("-- id: a\n-- summary: s\nSELECT 1 AS x").unwrap();
        assert!(run_rule(&db.conn, &r, &BTreeMap::new()).is_err());
    }
}
