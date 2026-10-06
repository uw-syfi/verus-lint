#![allow(
    clippy::unwrap_used,
    reason = "integration test helpers; a panic is the failure report"
)]
//! Keeps `docs/schema.md` in step with the database schema, and runs its example queries.

use std::collections::BTreeMap;
use std::path::Path;
use verus_lint::db::Db;
use verus_lint::extract::load_crate_logs;

const DOC: &str = include_str!("../docs/schema.md");

/// Table or view name to its columns (name, type) in order, as `DuckDB` reports them.
fn schema(db: &Db) -> BTreeMap<String, Vec<(String, String)>> {
    let rows = db
        .query_rows(
            "SELECT table_name, column_name, data_type FROM information_schema.columns \
             WHERE table_schema = 'main' ORDER BY table_name, ordinal_position",
        )
        .unwrap();
    let mut out: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for r in rows.into_iter().skip(1) {
        out.entry(r[0].clone())
            .or_default()
            .push((r[1].clone(), r[2].clone()));
    }
    out
}

/// Text of each level-3 section headed by a backticked name, up to the next level-2 or 3 heading.
fn sections() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut cur: Option<(String, String)> = None;
    for line in DOC.lines() {
        if line.starts_with("## ") || line.starts_with("### ") {
            if let Some((n, t)) = cur.take() {
                out.insert(n, t);
            }
            if let Some(rest) = line.strip_prefix("### `") {
                let name = rest.split('`').next().unwrap().to_string();
                cur = Some((name, String::new()));
            }
        } else if let Some((_, t)) = &mut cur {
            t.push_str(line);
            t.push('\n');
        }
    }
    if let Some((n, t)) = cur {
        out.insert(n, t);
    }
    out
}

/// Columns listed in a section's first table: (name, type) per row.
fn doc_columns(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let cells: Vec<&str> = l.split('|').map(str::trim).collect();
            let name = cells.get(1)?.strip_prefix('`')?.strip_suffix('`')?;
            let ty = cells.get(2)?;
            ty.chars()
                .all(|c| c.is_ascii_uppercase())
                .then(|| (name.to_string(), (*ty).to_string()))
        })
        .collect()
}

#[test]
fn every_table_and_column_is_documented() {
    let db = Db::in_memory().unwrap();
    let docs = sections();
    let mut problems = Vec::new();
    for (table, cols) in schema(&db) {
        let Some(text) = docs.get(&table) else {
            problems.push(format!("table or view `{table}` is not in docs/schema.md"));
            continue;
        };
        let doc = doc_columns(text);
        // A section may say "Same columns as `other`" instead of listing them.
        if doc.is_empty() && text.contains("Same columns as") {
            continue;
        }
        for (name, ty) in &cols {
            if !doc.iter().any(|(n, t)| n == name && t == ty) {
                problems.push(format!("`{table}.{name}` ({ty}) is missing or mistyped"));
            }
        }
        for (name, _) in &doc {
            if !cols.iter().any(|(n, _)| n == name) {
                problems.push(format!(
                    "`{table}.{name}` is documented but not in the schema"
                ));
            }
        }
    }
    let known = schema(&db);
    for name in docs.keys() {
        if !known.contains_key(name) {
            problems.push(format!(
                "docs/schema.md describes `{name}`, which is not in the schema"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "docs/schema.md is out of date:\n{}",
        problems.join("\n")
    );
}

#[test]
fn example_queries_run_on_the_fixture() {
    let mut db = Db::in_memory().unwrap();
    let ws = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/crate");
    let vir = include_str!("fixtures/fx/crate.vir");
    let imp = include_str!("fixtures/fx/crate.impl_names");
    load_crate_logs(&mut db, &ws, "fx", "Cargo.toml", vir, imp).unwrap();
    db.resolve().unwrap();
    let mut n = 0;
    let mut in_block = false;
    let mut sql = String::new();
    for line in DOC.lines() {
        if line.trim() == "```sql" {
            in_block = true;
            sql.clear();
        } else if in_block && line.trim() == "```" {
            in_block = false;
            db.query_rows(sql.trim().trim_end_matches(';'))
                .unwrap_or_else(|e| panic!("example query failed: {sql}\n{e:#}"));
            n += 1;
        } else if in_block {
            sql.push_str(line);
            sql.push('\n');
        }
    }
    assert!(n >= 25, "expected an example per table, found {n}");
}

#[test]
fn config_examples_parse() {
    let doc = include_str!("../docs/config.md");
    let (mut n, mut in_block, mut text) = (0, false, String::new());
    for line in doc.lines() {
        if line.trim() == "```toml" {
            in_block = true;
            text.clear();
        } else if in_block && line.trim() == "```" {
            in_block = false;
            verus_lint::config::Config::parse(&text)
                .unwrap_or_else(|e| panic!("config example does not parse: {text}\n{e:#}"));
            n += 1;
        } else if in_block {
            text.push_str(line);
            text.push('\n');
        }
    }
    assert!(n >= 5, "expected the config examples, found {n}");
}
