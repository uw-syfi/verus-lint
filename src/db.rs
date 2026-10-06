//! DuckDB storage of extracted facts.

use crate::vir::CrateFacts;
use anyhow::{Context, Result};
use duckdb::{Connection, params};
use std::collections::HashMap;
use std::path::Path;

pub const SCHEMA_VERSION: &str = "1.1.0";
const SCHEMA_SQL: &str = include_str!("schema.sql");

pub struct Db {
    pub conn: Connection,
    next_fn_id: i64,
}

pub struct CrateInfo<'a> {
    pub manifest: &'a str,
    pub log_bytes: u64,
}

impl Db {
    /// Create a fresh database file (an existing one is replaced).
    pub fn create(path: &Path) -> Result<Db> {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("duckdb.wal"));
        let conn =
            Connection::open(path).with_context(|| format!("creating {}", path.display()))?;
        conn.execute_batch(SCHEMA_SQL)?;
        conn.execute(
            "INSERT INTO meta VALUES ('schema_version', ?)",
            params![SCHEMA_VERSION],
        )?;
        Ok(Db {
            conn,
            next_fn_id: 0,
        })
    }

    pub fn open(path: &Path) -> Result<Db> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Ok(Db {
            conn,
            next_fn_id: 0,
        })
    }

    pub fn in_memory() -> Result<Db> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA_SQL)?;
        conn.execute(
            "INSERT INTO meta VALUES ('schema_version', ?)",
            params![SCHEMA_VERSION],
        )?;
        Ok(Db {
            conn,
            next_fn_id: 0,
        })
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO meta VALUES (?, ?)",
            params![key, value],
        )?;
        Ok(())
    }

    /// Load one crate's facts. Callee ids stay null until `resolve`.
    pub fn load_crate(&mut self, f: &CrateFacts, info: &CrateInfo) -> Result<()> {
        let base = self.next_fn_id;
        self.next_fn_id += f.functions.len() as i64;
        self.conn.execute(
            "INSERT INTO crates VALUES (?, ?, ?, ?, false)",
            params![
                f.krate,
                info.manifest,
                info.log_bytes as i64,
                f.functions.len() as i64
            ],
        )?;
        {
            let mut a = self.conn.appender("functions")?;
            for (i, r) in f.functions.iter().enumerate() {
                a.append_row(params![
                    base + i as i64,
                    r.path,
                    r.friendly,
                    r.krate,
                    r.module,
                    r.name,
                    r.self_type,
                    r.trait_path,
                    r.mode,
                    r.kind,
                    r.item_kind,
                    r.vis,
                    r.body_vis,
                    r.opaque,
                    r.reveal_vis,
                    r.external_body,
                    r.broadcast_forall,
                    r.broadcast_forall_only,
                    r.rlimit_attr,
                    r.spinoff_prover,
                    r.integer_ring,
                    r.bit_vector,
                    r.nonlinear,
                    r.has_body,
                    r.file,
                    r.line,
                    r.end_line,
                    r.body_lines,
                    r.n_requires,
                    r.n_ensures,
                    r.has_default,
                    r.trait_method,
                    r.type_invariant,
                    r.generated
                ])?;
            }
        }
        {
            let mut a = self.conn.appender("uses")?;
            for u in &f.uses {
                a.append_row(params![
                    base + u.caller as i64,
                    u.callee_path,
                    None::<i64>,
                    u.section,
                    u.kind,
                    u.in_trigger,
                    u.fuel,
                    u.file,
                    u.line,
                    u.col
                ])?;
            }
        }
        {
            let mut a = self.conn.appender("quantifiers")?;
            for q in &f.quants {
                a.append_row(params![
                    base + q.caller as i64,
                    q.quant,
                    q.trigger,
                    q.n_triggers,
                    q.section,
                    q.file,
                    q.line
                ])?;
            }
            let mut a = self.conn.appender("trusted")?;
            for t in &f.trusted {
                a.append_row(params![
                    t.caller.map(|c| base + c as i64),
                    t.kind,
                    t.file,
                    t.line,
                    t.text
                ])?;
            }
            // Own-crate external ids: located at the function of the same path when present.
            for (kind, path) in &f.externals {
                let at = f.functions.iter().position(|r| &r.path == path);
                let (id, file, line) = match at {
                    Some(i) => (
                        Some(base + i as i64),
                        f.functions[i].file.clone(),
                        f.functions[i].line,
                    ),
                    None => (None, String::new(), 0),
                };
                a.append_row(params![id, *kind, file, line, path])?;
            }
        }
        for t in &f.trait_impls {
            self.conn.execute(
                "INSERT OR IGNORE INTO trait_impls VALUES (?, ?, ?, ?, ?, ?)",
                params![
                    t.impl_path,
                    t.trait_path,
                    t.self_type,
                    f.krate,
                    t.file,
                    t.line
                ],
            )?;
        }
        let mut mod_file: HashMap<&str, &str> = HashMap::new();
        for r in &f.functions {
            let e = mod_file.entry(r.module.as_str()).or_insert(r.file.as_str());
            if r.file.as_str() < *e {
                *e = r.file.as_str();
            }
        }
        for m in &f.modules {
            self.conn.execute(
                "INSERT OR IGNORE INTO modules VALUES (?, ?, ?)",
                params![m, f.krate, mod_file.get(m.as_str()).copied()],
            )?;
        }
        for g in &f.groups {
            self.conn.execute(
                "INSERT OR IGNORE INTO broadcast_groups VALUES (?, ?)",
                params![g, f.krate],
            )?;
        }
        Ok(())
    }

    /// Resolve `callee_id` by exact VIR path across all loaded crates. Call once after all loads.
    pub fn resolve(&self) -> Result<()> {
        self.conn.execute_batch(
            "UPDATE uses SET callee_id = f.fn_id FROM functions f WHERE f.path = uses.callee_path;
             UPDATE module_uses SET callee_id = f.fn_id FROM functions f WHERE f.path = module_uses.callee_path;
             UPDATE group_members SET member_id = f.fn_id FROM functions f WHERE f.path = group_members.member_path;",
        )?;
        Ok(())
    }

    /// Run a query and return every cell as text (header row first).
    pub fn query_rows(&self, sql: &str) -> Result<Vec<Vec<String>>> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query([])?;
        let names: Vec<String> = rows.as_ref().map(|s| s.column_names()).unwrap_or_default();
        let mut out = vec![names.clone()];
        while let Some(r) = rows.next()? {
            let mut cells = Vec::new();
            for i in 0..names.len() {
                cells.push(crate::rules::cell_text(
                    &r.get::<_, duckdb::types::Value>(i)?,
                ));
            }
            out.push(cells);
        }
        Ok(out)
    }

    /// Run a query returning one integer column.
    pub fn query_ids(&self, sql: &str, args: &[&String]) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query(duckdb::params_from_iter(args.iter()))?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(r.get(0)?);
        }
        Ok(out)
    }

    pub fn warn(&self, krate: &str, what: &str, detail: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO warnings VALUES (?, ?, ?)",
            params![krate, what, detail],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vir::{ImplNames, parse_log};

    #[test]
    fn open_in_ancestor_is_open_spec() {
        // open_a's body visibility restricted to the crate root instead of pub.
        let text = include_str!("../tests/fixtures/mini.vir").replacen(
            "(BodyVisibility Visibility (Visibility :restricted_to None))",
            "(BodyVisibility Visibility (Visibility :restricted_to mini))",
            1,
        );
        let facts = parse_log(&text, "mini", &ImplNames::new()).unwrap();
        let mut db = Db::in_memory().unwrap();
        db.load_crate(
            &facts,
            &CrateInfo {
                manifest: "Cargo.toml",
                log_bytes: 0,
            },
        )
        .unwrap();
        let rows = db
            .query_rows("SELECT name FROM open_spec ORDER BY name")
            .unwrap();
        assert_eq!(
            rows,
            vec![vec!["name".to_string()], vec!["open_a".to_string()]]
        );
    }

    #[test]
    fn load_and_resolve_fixture() {
        let text = include_str!("../tests/fixtures/mini.vir");
        let facts = parse_log(text, "mini", &ImplNames::new()).unwrap();
        let mut db = Db::in_memory().unwrap();
        db.load_crate(
            &facts,
            &CrateInfo {
                manifest: "Cargo.toml",
                log_bytes: text.len() as u64,
            },
        )
        .unwrap();
        db.resolve().unwrap();
        let n: i64 = db
            .conn
            .query_row("SELECT count(*) FROM functions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 4);
        // open_a is the only open spec fn; lemma_u names it.
        let (name, callers): (String, i64) = db
            .conn
            .query_row(
                "SELECT d.name, count(DISTINCT e.caller_id) FROM open_spec d JOIN edges e ON e.callee_id = d.fn_id GROUP BY d.name",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((name.as_str(), callers), ("open_a", 1));
    }
}
