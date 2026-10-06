//! Command-line interface: extract, check and query commands over the fact database.
//!
//! [`run`] is the whole CLI. The `verus-lint` binary calls it with no Rust rules; a user rules
//! crate calls it with its own, and `check` and `run` start that crate's binary when the config
//! names one (`[rules] rust`).

#![allow(clippy::print_stdout, reason = "the CLI's output is its stdout")]

use crate::baseline::Baseline;
use crate::config::Config;
use crate::db::{Db, SCHEMA_VERSION};
use crate::engine::{EvalOptions, Source, evaluate, updated_baseline};
use crate::output::{Format, ReportMeta};
use crate::sdk::{Facts, Rule};
use crate::{extract, output, rules};
use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Set in the environment of a rules binary started by the CLI, so it does not start itself again.
const DELEGATED: &str = "VERUS_LINT_DELEGATED";

#[derive(Parser)]
#[command(
    version,
    about = "Lint and analysis for Verus codebases",
    after_help = "Exit status: 0 clean, 1 gated findings not covered by the baseline, 2 rule, \
                  config or runtime error, 3 unsupported Verus version."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct Common {
    /// Cargo workspace root.
    #[arg(long, default_value = ".")]
    workspace: PathBuf,
    /// Config file (default: `verus-lint.toml` in the workspace, if present).
    #[arg(long)]
    config: Option<PathBuf>,
}

#[derive(Args, Clone)]
struct ExtractArgs {
    #[command(flatten)]
    common: Common,
    /// Output directory (database and per-crate logs), relative to the workspace.
    #[arg(long, default_value = ".verus-lint")]
    out: PathBuf,
    /// Command prefix for `cargo verus`, split on spaces (for example `./verify`).
    #[arg(long, default_value = "")]
    toolchain: String,
    /// Extract only this package (repeatable); default: all verified members.
    #[arg(long = "crate")]
    crates: Vec<String>,
    /// Skip this member (package name or manifest-directory glob; repeatable), for example a
    /// `verify = true` crate that Verus cannot build.
    #[arg(long)]
    exclude: Vec<String>,
    /// Cargo target directory for the extraction builds.
    #[arg(long)]
    target_dir: Option<PathBuf>,
    /// Reuse existing per-crate logs instead of running Verus.
    #[arg(long)]
    reuse_logs: bool,
}

#[derive(Args, Clone)]
struct VerifyArgs {
    #[command(flatten)]
    common: Common,
    /// Directory of the database written by `extract`, relative to the workspace.
    #[arg(long, default_value = ".verus-lint")]
    out: PathBuf,
    /// Command prefix for `cargo verus`, split on spaces (for example `./verify`).
    #[arg(long, default_value = "")]
    toolchain: String,
    /// Verify only this package (repeatable); default: every crate in the database.
    #[arg(long = "crate")]
    crates: Vec<String>,
    /// Skip this member (package name or manifest-directory glob; repeatable).
    #[arg(long)]
    exclude: Vec<String>,
    /// Cargo target directory for the verification builds.
    #[arg(long)]
    target_dir: Option<PathBuf>,
    /// Solver seeds, comma separated: one run per crate and seed (default: one run, no seed).
    #[arg(long, value_delimiter = ',')]
    seeds: Vec<u32>,
    /// Ingest the reports saved by an earlier `verify` instead of running Verus.
    #[arg(long)]
    reuse_reports: bool,
    /// Ingest this saved `--output-json` output for the one `--crate` instead of running Verus.
    #[arg(long, requires = "crates")]
    report: Option<PathBuf>,
}

#[derive(Args, Clone)]
struct CheckArgs {
    /// Database written by `extract` (default: `.verus-lint/facts.duckdb` in the workspace).
    #[arg(long)]
    db: Option<PathBuf>,
    /// Directory of SQL rules (repeatable); also `[rules] dirs` in the config. No rules are built in.
    #[arg(long)]
    rules: Vec<PathBuf>,
    /// Only run rules whose id contains this text.
    #[arg(long)]
    only: Option<String>,
    /// Rule parameter override, `name=value` (repeatable).
    #[arg(long = "param")]
    params: Vec<String>,
    /// Print at most N findings per rule in text output (0 for all).
    #[arg(long, default_value_t = 20)]
    top: usize,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
    /// Write the report to this file instead of stdout.
    #[arg(long)]
    output: Option<PathBuf>,
    /// Baseline file (default: `[baseline] file`, else `verus-lint-baseline.json`, in the workspace).
    #[arg(long)]
    baseline: Option<PathBuf>,
    /// Write the baseline from the current findings of the rules that ran, then exit 0.
    #[arg(long)]
    update_baseline: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run Verus per crate and load facts into `DuckDB`.
    Extract(ExtractArgs),
    /// Run Verus's verification per crate and load per-function cost (rlimit, time) into the database.
    Verify(VerifyArgs),
    /// Run SQL and Rust rules against an extracted database.
    Check {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        check: CheckArgs,
    },
    /// Run one read-only SQL query against the database and print tab-separated rows.
    Query {
        /// Database written by `extract`.
        #[arg(long, default_value = ".verus-lint/facts.duckdb")]
        db: PathBuf,
        sql: String,
    },
    /// Extract, then check.
    Run {
        #[command(flatten)]
        extract: ExtractArgs,
        #[command(flatten)]
        check: CheckArgs,
    },
}

fn load_config(c: &Common) -> Result<Config> {
    if let Some(p) = &c.config {
        return Config::load(p);
    }
    let p = c.workspace.join("verus-lint.toml");
    if p.exists() {
        Config::load(&p)
    } else {
        Ok(Config::default())
    }
}
#[allow(
    clippy::cast_precision_loss,
    reason = "approximate megabytes for display"
)]
fn do_extract(a: &ExtractArgs) -> Result<PathBuf> {
    let cfg = load_config(&a.common)?;
    let mut toolchain: Vec<String> = a.toolchain.split_whitespace().map(String::from).collect();
    if toolchain.is_empty() {
        toolchain.clone_from(&cfg.extract.toolchain);
    }
    let mut crates = a.crates.clone();
    if crates.is_empty() {
        crates.clone_from(&cfg.extract.crates);
    }
    let mut exclude = cfg.extract.exclude.clone();
    exclude.extend(a.exclude.iter().cloned());
    let s = extract::extract(&extract::Options {
        workspace: a.common.workspace.clone(),
        out: a.out.clone(),
        toolchain,
        crates,
        exclude,
        roots: cfg.roots,
        target_dir: a.target_dir.clone(),
        reuse_logs: a.reuse_logs,
    })?;
    eprintln!(
        "extracted {} crates, {} functions, {} uses; logs {:.1} MB; verus {:.1}s, parse+load {:.1}s; {}",
        s.crates,
        s.functions,
        s.uses,
        s.log_bytes as f64 / 1e6,
        s.verus_seconds,
        s.parse_load_seconds,
        s.db.display()
    );
    Ok(s.db)
}

fn do_verify(a: &VerifyArgs) -> Result<()> {
    let cfg = load_config(&a.common)?;
    if let Some(report) = &a.report {
        let [krate] = a.crates.as_slice() else {
            bail!("--report needs exactly one --crate");
        };
        let seed = match a.seeds.as_slice() {
            [] => None,
            [s] => Some(*s),
            _ => bail!("--report takes at most one seed"),
        };
        let n = crate::verify::ingest_file(&a.common.workspace, &a.out, report, krate, seed)?;
        eprintln!(
            "ingested {} functions ({} unjoined), {} modules as run {}",
            n.functions, n.unjoined, n.modules, n.run_id
        );
        return Ok(());
    }
    let mut toolchain: Vec<String> = a.toolchain.split_whitespace().map(String::from).collect();
    if toolchain.is_empty() {
        toolchain.clone_from(&cfg.extract.toolchain);
    }
    let mut exclude = cfg.extract.exclude.clone();
    exclude.extend(a.exclude.iter().cloned());
    let s = crate::verify::run(&crate::verify::Options {
        workspace: a.common.workspace.clone(),
        out: a.out.clone(),
        toolchain,
        crates: a.crates.clone(),
        exclude,
        target_dir: a.target_dir.clone(),
        seeds: a.seeds.clone(),
        reuse_reports: a.reuse_reports,
    })?;
    eprintln!(
        "verified {} runs, {} function rows ({} unjoined); verus {:.1}s",
        s.runs, s.functions, s.unjoined, s.verus_seconds
    );
    Ok(())
}

fn parse_overrides(params: &[String]) -> Result<BTreeMap<String, String>> {
    let mut overrides = BTreeMap::new();
    for p in params {
        let (k, v) = p
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--param needs name=value, got `{p}`"))?;
        overrides.insert(k.to_string(), v.to_string());
    }
    Ok(overrides)
}

fn report_meta(facts: &Facts) -> Result<ReportMeta> {
    let rows = facts.query("SELECT key, value FROM meta")?;
    let get = |k: &str| {
        rows.iter()
            .skip(1)
            .find(|r| r[0] == k)
            .map(|r| r[1].clone())
            .unwrap_or_default()
    };
    let major = |v: &str| v.split('.').next().unwrap_or("").to_string();
    if major(&get("schema_version")) != major(SCHEMA_VERSION) {
        bail!(
            "database schema {} is not readable by this verus-lint (schema {SCHEMA_VERSION}); run extract again",
            get("schema_version")
        );
    }
    Ok(ReportMeta {
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        schema_version: get("schema_version"),
        verus_commit: get("verus_commit"),
        source_commit: get("source_commit"),
    })
}

fn do_check(
    native: &[&dyn Rule],
    a: &CheckArgs,
    db: &Path,
    cfg: &Config,
    ws: &Path,
) -> Result<ExitCode> {
    let facts = Facts::load(Db::open_read_only(db)?.conn)?;
    let meta = report_meta(&facts)?;
    let mut sources: Vec<Source> = Vec::new();
    let dirs = a
        .rules
        .iter()
        .cloned()
        .chain(cfg.rules.dirs.iter().map(|d| ws.join(d)));
    for d in dirs {
        sources.extend(rules::load_dir(&d)?.into_iter().map(Source::Sql));
    }
    sources.extend(native.iter().map(|r| Source::Native(*r)));
    if sources.is_empty() {
        eprintln!(
            "no rules: pass --rules DIR, set [rules] dirs or [rules] rust in verus-lint.toml (see examples/)"
        );
    }
    let overrides = parse_overrides(&a.params)?;
    let base_path = a.baseline.clone().unwrap_or_else(|| {
        ws.join(
            cfg.baseline
                .file
                .as_deref()
                .unwrap_or("verus-lint-baseline.json"),
        )
    });
    let baseline = Baseline::load(&base_path)?;
    let out = evaluate(
        &facts,
        &sources,
        &baseline,
        &EvalOptions {
            cfg,
            overrides: &overrides,
            only: a.only.as_deref(),
        },
    )?;
    let report = match a.format {
        Format::Text => output::text(&out, a.top),
        Format::Json => output::json(&out, &meta)?,
        Format::Sarif => output::sarif(&out, &meta)?,
    };
    if let Some(p) = &a.output {
        std::fs::write(p, &report).with_context(|| format!("writing {}", p.display()))?;
    } else {
        print!("{report}");
    }
    if a.update_baseline {
        updated_baseline(&baseline, &out, &meta.verus_commit).save(&base_path)?;
        eprintln!("baseline written to {}", base_path.display());
        return Ok(ExitCode::SUCCESS);
    }
    if out.gate_failures() > 0 {
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}

/// Build the Rust rules crate and return its executable.
fn build_rules_crate(manifest: &Path, profile: &str) -> Result<PathBuf> {
    let manifest = manifest
        .canonicalize()
        .with_context(|| format!("rules crate manifest {}", manifest.display()))?;
    let mut cmd = Command::new("cargo");
    cmd.arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--message-format=json-render-diagnostics");
    match profile {
        "dev" => {}
        "release" => {
            cmd.arg("--release");
        }
        p => {
            cmd.args(["--profile", p]);
        }
    }
    cmd.stderr(std::process::Stdio::inherit());
    // Variables cargo sets for the package that is running us (a test, `cargo run`) are not
    // part of this build; leaking them makes build scripts rebuild native code every time.
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy();
        if k.starts_with("CARGO_PKG_")
            || k.starts_with("CARGO_BIN_")
            || matches!(
                k.as_ref(),
                "CARGO_MANIFEST_DIR"
                    | "CARGO_MANIFEST_PATH"
                    | "CARGO_CRATE_NAME"
                    | "CARGO_PRIMARY_PACKAGE"
                    | "CARGO_TARGET_TMPDIR"
                    | "CARGO"
            )
        {
            cmd.env_remove(k.as_ref());
        }
    }
    eprintln!("building rules crate {}", manifest.display());
    let o = cmd
        .output()
        .context("running cargo build for the rules crate")?;
    if !o.status.success() {
        bail!("building the rules crate failed ({})", o.status);
    }
    let mut exe = None;
    for line in String::from_utf8_lossy(&o.stdout).lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["reason"] == "compiler-artifact"
            && v["manifest_path"].as_str().map(Path::new) == Some(manifest.as_path())
            && let Some(e) = v["executable"].as_str()
        {
            exe = Some(PathBuf::from(e));
        }
    }
    exe.ok_or_else(|| anyhow::anyhow!("{} builds no binary", manifest.display()))
}

/// Start the rules crate's binary with this command line when the config names one.
fn delegate(common: &Common, cfg: &Config) -> Result<Option<ExitCode>> {
    let Some(dir) = &cfg.rules.rust else {
        return Ok(None);
    };
    if std::env::var_os(DELEGATED).is_some() {
        return Ok(None);
    }
    let manifest = common.workspace.join(dir).join("Cargo.toml");
    let exe = build_rules_crate(
        &manifest,
        cfg.rules.rust_profile.as_deref().unwrap_or("release"),
    )?;
    let st = Command::new(&exe)
        .args(std::env::args_os().skip(1))
        .env(DELEGATED, "1")
        .status()
        .with_context(|| format!("running {}", exe.display()))?;
    Ok(Some(
        st.code()
            .and_then(|c| u8::try_from(c).ok())
            .map_or_else(|| ExitCode::from(2), ExitCode::from),
    ))
}

fn dispatch(native: &[&dyn Rule], cli: Cli) -> Result<ExitCode> {
    match cli.cmd {
        Cmd::Extract(a) => {
            do_extract(&a)?;
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Verify(a) => {
            do_verify(&a)?;
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Check { common, check } => {
            let cfg = load_config(&common)?;
            if let Some(c) = delegate(&common, &cfg)? {
                return Ok(c);
            }
            let db = check
                .db
                .clone()
                .unwrap_or_else(|| common.workspace.join(".verus-lint").join("facts.duckdb"));
            do_check(native, &check, &db, &cfg, &common.workspace)
        }
        Cmd::Query { db, sql } => {
            let db = Db::open_read_only(&db)?;
            for row in db.query_rows(&sql)? {
                println!("{}", row.join("\t"));
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Run { extract, check } => {
            let cfg = load_config(&extract.common)?;
            if let Some(c) = delegate(&extract.common, &cfg)? {
                return Ok(c);
            }
            let db = do_extract(&extract)?;
            do_check(native, &check, &db, &cfg, &extract.common.workspace)
        }
    }
}

/// Run the command line with the given Rust rules (alongside the SQL rules the config lists).
///
/// Exit status: 0 clean, 1 gated findings the baseline does not cover, 2 rule, config or
/// runtime error, 3 unsupported Verus version.
#[must_use]
pub fn run(native: &[&dyn Rule]) -> ExitCode {
    match dispatch(native, Cli::parse()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e:#}");
            if e.downcast_ref::<crate::version::Unsupported>().is_some() {
                ExitCode::from(3)
            } else {
                ExitCode::from(2)
            }
        }
    }
}
