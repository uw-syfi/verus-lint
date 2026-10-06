//! DuckDB storage of extracted facts.

use crate::vir::CrateFacts;
use anyhow::{Context, Result};
use duckdb::{Connection, params};
use std::collections::HashMap;
use std::path::Path;

pub const SCHEMA_VERSION: &str = "1.0.0";
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
                    r.n_ensures
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
             UPDATE module_uses SET callee_id = f.fn_id FROM functions f WHERE f.path = module_uses.callee_path;",
        )?;
        Ok(())
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
