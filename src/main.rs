use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use verus_lint::{db::Db, extract, rules};

#[derive(Parser)]
#[command(version, about = "Lint and analysis for Verus codebases")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct ExtractArgs {
    /// Cargo workspace root.
    #[arg(long, default_value = ".")]
    workspace: PathBuf,
    /// Output directory (database and per-crate logs).
    #[arg(long, default_value = ".verus-lint")]
    out: PathBuf,
    /// Command prefix for `cargo verus`, split on spaces (for example `./coral/verify`).
    #[arg(long, default_value = "")]
    toolchain: String,
    /// Extract only this package (repeatable); default: all verified members.
    #[arg(long = "crate")]
    crates: Vec<String>,
    /// Cargo target directory for the extraction builds.
    #[arg(long)]
    target_dir: Option<PathBuf>,
    /// Reuse existing per-crate logs instead of running Verus.
    #[arg(long)]
    reuse_logs: bool,
}

#[derive(Args, Clone)]
struct CheckArgs {
    /// Database written by `extract`.
    #[arg(long)]
    db: Option<PathBuf>,
    /// Extra directory of SQL rules.
    #[arg(long)]
    rules: Vec<PathBuf>,
    /// Only run rules whose id contains this text.
    #[arg(long)]
    only: Option<String>,
    /// Rule parameter override, `name=value` (repeatable).
    #[arg(long = "param")]
    params: Vec<String>,
    /// Print at most N findings per rule (0 for all).
    #[arg(long, default_value_t = 20)]
    top: usize,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run Verus per crate and load facts into DuckDB.
    Extract(ExtractArgs),
    /// Run SQL rules against an extracted database.
    Check(CheckArgs),
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

fn do_extract(a: &ExtractArgs) -> Result<PathBuf> {
    let s = extract::extract(&extract::Options {
        workspace: a.workspace.clone(),
        out: a.out.clone(),
        toolchain: a.toolchain.split_whitespace().map(String::from).collect(),
        crates: a.crates.clone(),
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

fn do_check(a: &CheckArgs, db: PathBuf) -> Result<ExitCode> {
    let db = Db::open(&db)?;
    let mut all = rules::builtin()?;
    for d in &a.rules {
        all.extend(rules::load_dir(d)?);
    }
    let mut overrides = BTreeMap::new();
    for p in &a.params {
        let (k, v) = p
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--param needs name=value, got `{p}`"))?;
        overrides.insert(k.to_string(), v.to_string());
    }
    for r in all
        .iter()
        .filter(|r| a.only.as_ref().is_none_or(|o| r.id.contains(o.as_str())))
    {
        let mut fs = rules::run_rule(&db.conn, r, &overrides)?;
        fs.sort_by(|x, y| {
            y.metric
                .partial_cmp(&x.metric)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        println!("== {} ({} findings): {}", r.id, fs.len(), r.summary);
        let n = if a.top == 0 { fs.len() } else { a.top };
        for f in fs.iter().take(n) {
            println!("{}", rules::format_finding(f));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn run(cli: Cli) -> Result<ExitCode> {
    match cli.cmd {
        Cmd::Extract(a) => {
            do_extract(&a)?;
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Check(a) => {
            let db =
                a.db.clone()
                    .unwrap_or_else(|| PathBuf::from(".verus-lint/facts.duckdb"));
            do_check(&a, db)
        }
        Cmd::Query { db, sql } => {
            let db = Db::open(&db)?;
            for row in db.query_rows(&sql)? {
                println!("{}", row.join("\t"));
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Run { extract, check } => {
            let db = do_extract(&extract)?;
            do_check(&check, db)
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e:#}");
            if e.downcast_ref::<verus_lint::version::Unsupported>()
                .is_some()
            {
                ExitCode::from(3)
            } else {
                ExitCode::from(2)
            }
        }
    }
}
