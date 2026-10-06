//! Dynamic facts: per-function and per-module verification cost from Verus's `--output-json`
//! report (`--time-expanded`), joined to the extracted functions by their friendly name.
//!
//! `cargo verus build` prints one report object per crate it verifies, dependencies first, so a
//! run's output holds several objects; the one whose functions belong to the crate under test is
//! the crate's report.

use crate::db::Db;
use crate::extract::{
    cargo_metadata, clean_args, git, members_from_metadata, now_utc, select_members,
    toolchain_command,
};
use crate::num::to_i64;
use crate::version::{self, VerusVersion};
use anyhow::{Context, Result, anyhow, bail};
use duckdb::params;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// Arguments of one crate's verification run (after the toolchain prefix).
#[must_use]
pub fn verify_args(krate: &str, target_dir: Option<&Path>, seed: Option<u32>) -> Vec<String> {
    let mut a: Vec<String> = ["cargo", "verus", "build", "-p", krate]
        .iter()
        .map(ToString::to_string)
        .collect();
    if let Some(t) = target_dir {
        a.extend(["--target-dir".to_string(), t.display().to_string()]);
    }
    a.extend(
        [
            "--fwd-verus-args-to",
            "roots",
            "--",
            "--time-expanded",
            "--output-json",
        ]
        .iter()
        .map(ToString::to_string),
    );
    if let Some(s) = seed {
        a.extend(["--smt-option".to_string(), format!("smt.random_seed={s}")]);
    }
    a
}

/// All JSON objects in `text`, which may start with other output (the first `{` begins the
/// first object); objects follow each other with whitespace between.
///
/// # Errors
/// Fails when no object is found or one is malformed.
pub fn parse_reports(text: &str) -> Result<Vec<Value>> {
    let start = text
        .find('{')
        .ok_or_else(|| anyhow!("no JSON object in the Verus output"))?;
    serde_json::Deserializer::from_str(&text[start..])
        .into_iter::<Value>()
        .collect::<Result<Vec<_>, _>>()
        .context("parsing the Verus --output-json report")
}

/// Each function entry of a report object with the module it was verified under.
fn breakdown(obj: &Value) -> impl Iterator<Item = (&str, &Value)> {
    obj["times-ms"]["smt"]["smt-run-module-times"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|m| {
            let module = m["module"].as_str().unwrap_or("");
            m["function-breakdown"]
                .as_array()
                .into_iter()
                .flatten()
                .map(move |f| (module, f))
        })
}

/// The report object of crate `ident`: the one with the most functions under `ident::`.
///
/// # Errors
/// Fails when no object has a function of the crate.
pub fn select_crate_report<'a>(objs: &'a [Value], ident: &str) -> Result<&'a Value> {
    let prefix = format!("{ident}::");
    objs.iter()
        .map(|o| {
            let n = breakdown(o)
                .filter(|(_, f)| {
                    f["function"]
                        .as_str()
                        .is_some_and(|n| n.starts_with(&prefix))
                })
                .count();
            (n, o)
        })
        .filter(|(n, _)| *n > 0)
        .max_by_key(|(n, _)| *n)
        .map(|(_, o)| o)
        .ok_or_else(|| anyhow!("no report object has a function of crate {ident}"))
}

/// Check that a report was produced by a supported Verus release.
///
/// # Errors
/// Fails with `Unsupported` for another release, or when the object has no `verus` section.
pub fn check_report_version(obj: &Value) -> Result<()> {
    let v = &obj["verus"];
    let s = |k: &str| v[k].as_str().map(String::from);
    let (Some(version), Some(commit)) = (s("version"), s("commit")) else {
        bail!("report has no verus.version and verus.commit");
    };
    version::check(&VerusVersion {
        version,
        commit,
        toolchain: s("toolchain").unwrap_or_default(),
    })
}

#[allow(
    missing_docs,
    reason = "plain data row; field names match the schema columns"
)]
pub struct RunInfo {
    pub krate: String,
    pub seed: Option<u32>,
    pub verus_args: String,
    pub source_commit: String,
    pub started_at: String,
    pub wall_s: f64,
}

/// What one ingested report contributed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Ingested {
    /// Run id of the new `runs` row.
    pub run_id: i64,
    /// `verify_fn` rows added.
    pub functions: usize,
    /// Of those, rows that matched no extracted function.
    pub unjoined: usize,
    /// Of those, rows that matched more than one function.
    pub ambiguous: usize,
    /// `verify_module` rows added.
    pub modules: usize,
}

/// `column` value to the `(fn_id, module)` of each function of the crate that has it.
fn id_map(db: &Db, column: &str, krate: &str) -> Result<HashMap<String, Vec<(i64, String)>>> {
    let mut stmt = db.conn.prepare(&format!(
        "SELECT {column}, fn_id, module FROM functions WHERE crate = ? ORDER BY fn_id"
    ))?;
    let mut rows = stmt.query(params![krate])?;
    let mut m: HashMap<String, Vec<(i64, String)>> = HashMap::new();
    while let Some(r) = rows.next()? {
        m.entry(r.get(0)?).or_default().push((r.get(1)?, r.get(2)?));
    }
    Ok(m)
}

/// Add one crate's report object to the database as a new run.
///
/// Report names match a function's `friendly` name, else its path. A name with several matches
/// stays unjoined (null `fn_id`) and is counted in `meta.ambiguous_verify_rows`; unjoined rows
/// are in `meta.unjoined_verify_rows`.
///
/// # Errors
/// Fails when the report is from an unsupported Verus or a database step fails.
pub fn ingest(db: &Db, obj: &Value, info: &RunInfo) -> Result<Ingested> {
    check_report_version(obj)?;
    let facts_commit: Option<String> = db
        .conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'verus_commit'",
            [],
            |r| r.get(0),
        )
        .ok();
    if let Some(fc) = facts_commit.filter(|c| !c.is_empty())
        && obj["verus"]["commit"].as_str() != Some(fc.as_str())
    {
        bail!("report is from a different Verus than the extracted facts ({fc})");
    }
    let run_id: i64 =
        db.conn
            .query_row("SELECT coalesce(max(run_id), 0) + 1 FROM runs", [], |r| {
                r.get(0)
            })?;
    db.conn.execute(
        "INSERT INTO runs VALUES (?, ?, ?, ?, ?, ?, ?)",
        params![
            run_id,
            info.krate,
            info.seed,
            info.verus_args,
            info.source_commit,
            info.started_at,
            info.wall_s
        ],
    )?;
    let mut out = Ingested {
        run_id,
        ..Ingested::default()
    };
    insert_functions(db, obj, info, &mut out)?;
    out.modules = insert_modules(db, obj, info, run_id)?;
    db.conn.execute(
        "UPDATE crates SET verified = true WHERE crate = ?",
        params![info.krate],
    )?;
    refresh_join_counts(db)?;
    Ok(out)
}

/// Module name as `functions.module` spells it: the report omits the crate prefix.
fn qualify(krate: &str, module: &str) -> String {
    if module.is_empty() {
        krate.to_string()
    } else {
        format!("{krate}::{module}")
    }
}

fn insert_functions(db: &Db, obj: &Value, info: &RunInfo, out: &mut Ingested) -> Result<()> {
    let by_friendly = id_map(db, "friendly", &info.krate)?;
    let by_path = id_map(db, "path", &info.krate)?;
    let mut a = db.conn.appender("verify_fn")?;
    for (module, f) in breakdown(obj) {
        let Some(name) = f["function"].as_str() else {
            continue;
        };
        let mut ids: Vec<&(i64, String)> = by_friendly
            .get(name)
            .or_else(|| by_path.get(name))
            .map_or_else(Vec::new, |v| v.iter().collect());
        let candidates = ids.len();
        if candidates > 1 {
            // Two impls of one type print the same name; the report says which module owns each.
            let module = qualify(&info.krate, module);
            ids.retain(|(_, m)| *m == module);
        }
        let fn_id = (ids.len() == 1).then(|| ids[0].0);
        out.functions += 1;
        if fn_id.is_none() {
            out.unjoined += 1;
            out.ambiguous += usize::from(candidates > 1);
        }
        // Verus spells the key "mode:" (with the colon) in this release.
        let mode = f["mode:"].as_str().or_else(|| f["mode"].as_str());
        a.append_row(params![
            out.run_id,
            fn_id,
            name,
            info.krate,
            qualify(&info.krate, module),
            mode,
            f["rlimit"].as_i64().unwrap_or(0),
            f["time-micros"].as_i64().unwrap_or(0),
            f["success"].as_bool().unwrap_or(true),
            info.seed
        ])?;
    }
    Ok(())
}

fn insert_modules(db: &Db, obj: &Value, info: &RunInfo, run_id: i64) -> Result<usize> {
    // A module with spun-off functions has several session entries under one name; the longest
    // one bounds the module's wall time.
    let mut session: HashMap<&str, i64> = HashMap::new();
    for m in obj["times-ms"]["total-verify-module-times"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let e = session
            .entry(m["module"].as_str().unwrap_or(""))
            .or_insert(0);
        *e = (*e).max(m["time"].as_i64().unwrap_or(0));
    }
    let mut a = db.conn.appender("verify_module")?;
    let mut n = 0;
    for m in obj["times-ms"]["smt"]["smt-run-module-times"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let name = m["module"].as_str().unwrap_or("");
        a.append_row(params![
            run_id,
            qualify(&info.krate, name),
            info.krate,
            m["rlimit"].as_i64().unwrap_or(0),
            m["time"].as_i64().unwrap_or(0),
            session.get(name).copied().unwrap_or(0)
        ])?;
        n += 1;
    }
    Ok(n)
}

#[allow(
    missing_docs,
    reason = "plain data row; field names match the schema columns"
)]
pub struct Options {
    pub workspace: std::path::PathBuf,
    pub out: std::path::PathBuf,
    pub toolchain: Vec<String>,
    pub crates: Vec<String>,
    pub exclude: Vec<String>,
    pub target_dir: Option<std::path::PathBuf>,
    /// One run per crate and seed; empty means one run without a seed.
    pub seeds: Vec<u32>,
    /// Ingest the reports saved by an earlier run instead of running Verus.
    pub reuse_reports: bool,
}

/// What a `verify` command added.
#[derive(Debug, Default)]
pub struct Summary {
    /// Runs ingested.
    pub runs: usize,
    /// `verify_fn` rows added.
    pub functions: usize,
    /// Rows that matched no single function.
    pub unjoined: usize,
    /// Seconds spent in Verus.
    pub verus_seconds: f64,
}

fn open_facts(out: &Path) -> Result<Db> {
    let path = out.join("facts.duckdb");
    if !path.exists() {
        bail!("{} does not exist; run `extract` first", path.display());
    }
    Db::open(&path)
}

fn out_dir(ws: &Path, out: &Path) -> std::path::PathBuf {
    if out.is_absolute() {
        out.to_path_buf()
    } else {
        ws.join(out)
    }
}

/// Run Verus's verification on one crate, once, and return the crate's report object.
fn run_verus(o: &Options, ws: &Path, krate: &str, ident: &str, seed: Option<u32>) -> Result<Value> {
    eprintln!("verify {krate}: cargo clean, then Verus --time-expanded");
    let st = toolchain_command(&o.toolchain, clean_args(krate, o.target_dir.as_deref()))
        .current_dir(ws)
        .status()
        .with_context(|| format!("cleaning {krate}"))?;
    if !st.success() {
        bail!("cargo clean failed for crate {krate} ({st})");
    }
    let res = toolchain_command(
        &o.toolchain,
        verify_args(krate, o.target_dir.as_deref(), seed),
    )
    .current_dir(ws)
    .stderr(std::process::Stdio::inherit())
    .output()
    .with_context(|| format!("running Verus for {krate}"))?;
    let text = String::from_utf8_lossy(&res.stdout);
    let objs =
        parse_reports(&text).with_context(|| format!("Verus for {krate} ({})", res.status))?;
    let mut obj = select_crate_report(&objs, ident)?.clone();
    if !res.status.success() {
        eprintln!(
            "warning: Verus exited with {} for {krate}; failures are in the report",
            res.status
        );
    }
    // The per-function notes are large and unused.
    if let Some(m) = obj.as_object_mut() {
        m.remove("func-details");
    }
    Ok(obj)
}

/// Verify each selected crate (once per seed) and ingest the reports.
///
/// The database is the one `extract` wrote. Each report is saved under `<out>/cache/<crate>/`
/// so `reuse_reports` can ingest it again.
///
/// # Errors
/// Fails when the database is missing, Verus produces no report, or ingestion fails.
pub fn run(o: &Options) -> Result<Summary> {
    let ws = o.workspace.canonicalize().context("workspace path")?;
    let out = out_dir(&ws, &o.out);
    let db = open_facts(&out)?;
    let members = select_members(
        members_from_metadata(&cargo_metadata(&ws)?)?,
        &ws,
        &o.crates,
        &o.exclude,
    )?;
    let known: std::collections::BTreeSet<String> = db
        .query_rows("SELECT crate FROM crates")?
        .into_iter()
        .skip(1)
        .map(|r| r[0].clone())
        .collect();
    let seeds: Vec<Option<u32>> = if o.seeds.is_empty() {
        vec![None]
    } else {
        o.seeds.iter().copied().map(Some).collect()
    };
    let commit = git(&ws, &["rev-parse", "HEAD"]).unwrap_or_default();
    let mut sum = Summary::default();
    for m in members.iter().filter(|m| known.contains(&m.ident())) {
        for seed in &seeds {
            let tag = seed.map_or_else(|| "default".to_string(), |s| format!("seed{s}"));
            let saved = out
                .join("cache")
                .join(&m.name)
                .join(format!("report-{tag}.json"));
            let (started, t) = (now_utc(), std::time::Instant::now());
            let obj = if o.reuse_reports && saved.exists() {
                let text = std::fs::read_to_string(&saved)
                    .with_context(|| format!("reading {}", saved.display()))?;
                parse_reports(&text)?.swap_remove(0)
            } else {
                let obj = run_verus(o, &ws, &m.name, &m.ident(), *seed)?;
                sum.verus_seconds += t.elapsed().as_secs_f64();
                if let Some(d) = saved.parent() {
                    std::fs::create_dir_all(d)?;
                }
                std::fs::write(&saved, serde_json::to_string(&obj)?)?;
                obj
            };
            let info = RunInfo {
                krate: m.ident(),
                seed: *seed,
                verus_args: verify_args(&m.name, o.target_dir.as_deref(), *seed).join(" "),
                source_commit: commit.clone(),
                started_at: started,
                wall_s: t.elapsed().as_secs_f64(),
            };
            let n = ingest(&db, &obj, &info)?;
            eprintln!(
                "verify {} ({tag}): {} functions, {} unjoined, {} modules",
                m.name, n.functions, n.unjoined, n.modules
            );
            sum.runs += 1;
            sum.functions += n.functions;
            sum.unjoined += n.unjoined;
        }
    }
    Ok(sum)
}

/// Ingest a saved `cargo verus build -- --time-expanded --output-json` output as a new run of
/// `krate`.
///
/// # Errors
/// Fails when the file holds no report of the crate, or ingestion fails.
pub fn ingest_file(
    workspace: &Path,
    out: &Path,
    report: &Path,
    krate: &str,
    seed: Option<u32>,
) -> Result<Ingested> {
    let db = open_facts(&out_dir(workspace, out))?;
    let text =
        std::fs::read_to_string(report).with_context(|| format!("reading {}", report.display()))?;
    let objs = parse_reports(&text)?;
    let ident = krate.replace('-', "_");
    ingest(
        &db,
        select_crate_report(&objs, &ident)?,
        &RunInfo {
            krate: ident,
            seed,
            verus_args: format!("report {}", report.display()),
            source_commit: String::new(),
            started_at: now_utc(),
            wall_s: 0.0,
        },
    )
}

/// Store the unjoined and ambiguous counts of the default runs in `meta`.
fn refresh_join_counts(db: &Db) -> Result<()> {
    let (unjoined, ambiguous): (i64, i64) = db.conn.query_row(
        "SELECT count(*) FILTER (WHERE fn_id IS NULL),
                count(*) FILTER (WHERE fn_id IS NULL AND (
                    SELECT count(*) FROM functions f
                    WHERE f.crate = v.crate AND (f.friendly = v.friendly OR f.path = v.friendly)) > 1)
         FROM verify_latest v",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    db.set_meta("unjoined_verify_rows", &unjoined.to_string())?;
    db.set_meta("ambiguous_verify_rows", &ambiguous.to_string())?;
    let total: i64 = db
        .conn
        .query_row("SELECT count(*) FROM verify_latest", [], |r| r.get(0))?;
    db.set_meta(
        "verify_rows",
        &to_i64(usize::try_from(total).unwrap_or(0)).to_string(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vir::{ImplNames, parse_log};

    const MINI: &str = include_str!("../tests/fixtures/mini.vir");

    fn report(commit: &str, funcs: &[(&str, &str, i64, bool)]) -> String {
        let fb: Vec<String> = funcs
            .iter()
            .map(|(n, m, r, ok)| {
                format!(
                    r#"{{"function":"{n}","mode:":"{m}","time":1,"time-micros":{r},"rlimit":{r},"success":{ok}}}"#
                )
            })
            .collect();
        format!(
            r#"noise line
{{"func-details":{{}},"verification-results":{{"success":true}},
 "times-ms":{{"total-verify-module-times":[{{"module":"m","time":9}},{{"module":"m","time":30}}],
   "smt":{{"smt-run-module-times":[{{"module":"m","time":5,"rlimit":77,"function-breakdown":[{}]}}]}}}},
 "verus":{{"version":"0.2026.07.18.3a4d30b","commit":"{commit}"}}}}
{{"times-ms":{{"smt":{{"smt-run-module-times":[{{"module":"x","rlimit":1,"function-breakdown":[
  {{"function":"vstd::seq::len","mode:":"spec","rlimit":1,"time-micros":1,"success":true}}]}}]}}}},
 "verus":{{"version":"0.2026.07.18.3a4d30b","commit":"{commit}"}}}}"#,
            fb.join(",")
        )
    }

    const OK_COMMIT: &str = "3a4d30bcdc4571e7927af97be9c4664973083eda";

    fn db() -> Db {
        let facts = parse_log(MINI, "mini", &ImplNames::new()).unwrap();
        let mut db = Db::in_memory().unwrap();
        db.load_crate(
            &facts,
            &crate::db::CrateInfo {
                manifest: "Cargo.toml",
                log_bytes: 0,
            },
        )
        .unwrap();
        db
    }

    fn info(seed: Option<u32>) -> RunInfo {
        RunInfo {
            krate: "mini".into(),
            seed,
            verus_args: String::new(),
            source_commit: String::new(),
            started_at: String::new(),
            wall_s: 1.0,
        }
    }

    #[test]
    fn selects_the_crates_object_and_joins_by_name() {
        let text = report(
            OK_COMMIT,
            &[
                ("mini::n::lemma_u", "proof", 500, true),
                ("mini::m::open_a", "spec", 20, true),
                ("mini::n::gone", "proof", 5, false),
            ],
        );
        let objs = parse_reports(&text).unwrap();
        assert_eq!(objs.len(), 2);
        let o = select_crate_report(&objs, "mini").unwrap();
        let db = db();
        let n = ingest(&db, o, &info(None)).unwrap();
        assert_eq!(
            (n.functions, n.unjoined, n.ambiguous, n.modules),
            (3, 1, 0, 1)
        );
        let rows = db
            .query_rows(
                "SELECT friendly, rlimit, success, fn_id IS NULL FROM verify_latest ORDER BY rlimit DESC",
            )
            .unwrap();
        assert_eq!(rows[1], ["mini::n::lemma_u", "500", "true", "false"]);
        assert_eq!(rows[3], ["mini::n::gone", "5", "false", "true"]);
        let m = db
            .query_rows("SELECT module, rlimit, session_time_ms FROM verify_module")
            .unwrap();
        assert_eq!(m[1], ["mini::m", "77", "30"]);
        let meta = db
            .query_rows("SELECT value FROM meta WHERE key = 'unjoined_verify_rows'")
            .unwrap();
        assert_eq!(meta[1][0], "1");
    }

    #[test]
    fn default_run_prefers_no_seed_then_lowest_and_worst_spans_seeds() {
        let db = db();
        for (seed, r) in [(Some(2), 900), (Some(1), 300), (None, 100)] {
            let text = report(OK_COMMIT, &[("mini::n::lemma_u", "proof", r, true)]);
            let objs = parse_reports(&text).unwrap();
            ingest(
                &db,
                select_crate_report(&objs, "mini").unwrap(),
                &info(seed),
            )
            .unwrap();
        }
        let v = db.query_rows("SELECT rlimit FROM verify_latest").unwrap();
        assert_eq!(v[1][0], "100");
        let w = db
            .query_rows("SELECT n_runs, max_rlimit, min_rlimit FROM verify_worst")
            .unwrap();
        assert_eq!(w[1], ["3", "900", "100"]);
        // Without the unseeded run, the lowest seed is the default.
        db.conn
            .execute("DELETE FROM runs WHERE seed IS NULL", [])
            .unwrap();
        let v = db.query_rows("SELECT rlimit FROM verify_latest").unwrap();
        assert_eq!(v[1][0], "300");
    }

    #[test]
    fn same_name_in_two_modules_joins_by_module() {
        let db = db();
        for (id, m) in [(100, "mini::a"), (101, "mini::b"), (102, "mini::b")] {
            db.conn
                .execute(
                    "INSERT INTO functions (fn_id, path, friendly, crate, module) VALUES (?, ?, 'mini::T::f', 'mini', ?)",
                    params![id, format!("{m}::impl&%{id}::f"), m],
                )
                .unwrap();
        }
        let text = r#"{"times-ms":{"smt":{"smt-run-module-times":[
          {"module":"a","rlimit":1,"function-breakdown":[{"function":"mini::T::f","mode:":"exec","rlimit":7,"time-micros":1,"success":true}]},
          {"module":"b","rlimit":1,"function-breakdown":[{"function":"mini::T::f","mode:":"exec","rlimit":8,"time-micros":1,"success":true}]}]}},
          "verus":{"version":"0.2026.07.18.3a4d30b","commit":"3a4d30bcdc4571e7927af97be9c4664973083eda"}}"#;
        let objs = parse_reports(text).unwrap();
        let n = ingest(&db, &objs[0], &info(None)).unwrap();
        assert_eq!((n.functions, n.unjoined, n.ambiguous), (2, 1, 1));
        let r = db
            .query_rows("SELECT rlimit, fn_id FROM verify_fn ORDER BY rlimit")
            .unwrap();
        assert_eq!(r[1], ["7", "100"]);
        assert_eq!(r[2], ["8", ""], "two functions in module b share the name");
    }

    #[test]
    fn example_rules_read_the_dynamic_tables() {
        use std::collections::BTreeMap;
        let db = db();
        // lemma_u: 100k..3.5M across seeds; open_a: cheap, fails under seed 2.
        for (seed, lemma, open, ok) in [
            (None, 4_000_000, 20, true),
            (Some(1), 100_000, 20, true),
            (Some(2), 3_500_000, 20, false),
        ] {
            let text = report(
                OK_COMMIT,
                &[
                    ("mini::n::lemma_u", "proof", lemma, true),
                    ("mini::m::open_a", "spec", open, ok),
                ],
            );
            let objs = parse_reports(&text).unwrap();
            ingest(
                &db,
                select_crate_report(&objs, "mini").unwrap(),
                &info(seed),
            )
            .unwrap();
        }
        let run = |id: &str| {
            let r = crate::rules::examples()
                .unwrap()
                .into_iter()
                .find(|r| r.id == id)
                .unwrap();
            crate::rules::run_rule(&db.conn, &r, &BTreeMap::new()).unwrap()
        };
        // Default run: lemma_u 4M (40% of the budget, under warn_pct) and spinoff-worthy.
        assert!(run("verus/rlimit-headroom").is_empty());
        let hot = run("verus/rlimit-hotspot");
        assert_eq!(
            hot.iter().map(|f| f.entity.as_str()).collect::<Vec<_>>(),
            ["mini::n::lemma_u"]
        );
        let spin = run("verus/spinoff-candidate");
        assert_eq!((spin.len(), spin[0].metric), (1, Some(4_000_000.0)));
        let seeds = run("verus/seed-instability");
        assert_eq!(seeds.len(), 2);
        assert!(
            seeds
                .iter()
                .any(|f| f.entity == "mini::m::open_a" && f.severity == "error")
        );
        assert!(
            seeds
                .iter()
                .any(|f| f.entity == "mini::n::lemma_u" && f.message.contains("40.0x"))
        );
        assert_eq!(run("verus/hotspot-growth").len(), 1);
        let low = crate::rules::examples()
            .unwrap()
            .into_iter()
            .find(|r| r.id == "verus/rlimit-headroom")
            .unwrap();
        let ov = BTreeMap::from([("warn_pct".to_string(), "30".to_string())]);
        let head = crate::rules::run_rule(&db.conn, &low, &ov).unwrap();
        assert_eq!(head.len(), 1);
        assert_eq!(head[0].severity, "warning");
    }

    #[test]
    fn refuses_another_verus_and_missing_crate() {
        let text = report(
            "0000000000000000000000000000000000000000",
            &[("mini::n::f", "proof", 1, true)],
        );
        let objs = parse_reports(&text).unwrap();
        let e = ingest(&db(), &objs[0], &info(None)).unwrap_err();
        assert!(e.downcast_ref::<version::Unsupported>().is_some());
        assert!(select_crate_report(&objs, "other").is_err());
        assert!(parse_reports("nothing here").is_err());
    }

    #[test]
    fn args_set_the_seed() {
        let s = verify_args("c", None, Some(3)).join(" ");
        assert!(s.ends_with("-- --time-expanded --output-json --smt-option smt.random_seed=3"));
        assert!(
            !verify_args("c", None, None)
                .join(" ")
                .contains("random_seed")
        );
    }
}
