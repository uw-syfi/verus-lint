#![allow(
    clippy::unwrap_used,
    reason = "integration test helpers; a panic is the failure report"
)]
//! The CLI builds the example rules crate and runs it against a database.

use std::path::{Path, PathBuf};
use std::process::Command;
use verus_lint::config::Config;
use verus_lint::db::Db;
use verus_lint::extract::load_crate_logs;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vl-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn mini_db(path: &Path) {
    let mut db = Db::create(path).unwrap();
    let vir = include_str!("fixtures/mini.vir");
    load_crate_logs(
        &mut db,
        path.parent().unwrap(),
        "mini",
        "Cargo.toml",
        vir,
        "",
    )
    .unwrap();
    db.resolve().unwrap();
    db.set_meta("verus_commit", "test").unwrap();
}

fn cli(ws: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_verus-lint"))
        .args(args)
        .current_dir(ws)
        .output()
        .unwrap()
}

#[test]
fn cli_builds_and_runs_the_rules_crate_with_levels_and_baseline() {
    let ws = scratch("rules-crate");
    mini_db(&ws.join("facts.duckdb"));
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/rust-rules");
    let toml = format!(
        "[rules]\nrust = {:?}\nrust_profile = \"dev\"\n\n[rules.levels]\n\"example/opaque-reveal-spread\" = \"gate\"\n\n[rules.params.\"example/opaque-reveal-spread\"]\nmax_modules = 0\n",
        crate_dir.display().to_string()
    );
    assert!(Config::parse(&toml).is_ok());
    std::fs::write(ws.join("verus-lint.toml"), &toml).unwrap();
    let base = ["check", "--db", "facts.duckdb"];

    // A gated finding the baseline does not cover: exit 1.
    let o = cli(&ws, &base);
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(
        o.status.code(),
        Some(1),
        "{out}\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(out.contains("example/opaque-reveal-spread"), "{out}");
    assert!(out.contains("mini::m::opaque_b"), "{out}");

    // Writing the baseline accepts it; the next check passes.
    let mut upd = base.to_vec();
    upd.push("--update-baseline");
    assert_eq!(cli(&ws, &upd).status.code(), Some(0));
    let b = std::fs::read_to_string(ws.join("verus-lint-baseline.json")).unwrap();
    assert!(b.contains("\"metric\""), "{b}");
    assert_eq!(cli(&ws, &base).status.code(), Some(0));

    // The parameter override beyond the baseline's metric still passes only while covered;
    // JSON output reports the finding as baselined.
    let mut js = base.to_vec();
    js.extend(["--format", "json"]);
    let o = cli(&ws, &js);
    assert_eq!(o.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["findings"][0]["baselined"], true);
    assert_eq!(v["gate_failures"], 0);

    // A config error is exit status 2.
    std::fs::write(ws.join("verus-lint.toml"), "[rules.levels]\nx = \"loud\"\n").unwrap();
    assert_eq!(cli(&ws, &base).status.code(), Some(2));
    std::fs::remove_dir_all(&ws).unwrap();
}
