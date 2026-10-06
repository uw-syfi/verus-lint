//! Extraction driver: one Verus `--no-verify --log vir` run per workspace crate.
//!
//! Verus names the log `crate.vir` and deletes the log directory first, so
//! each crate gets its own log directory under `<out>/cache/<crate>/log`.

use crate::db::{CrateInfo, Db};
use crate::version::{self, VerusVersion};
use crate::vir::{ImplNames, parse_impl_names, parse_log};
use anyhow::{Context, Result, anyhow, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

pub struct Options {
    pub workspace: PathBuf,
    pub out: PathBuf,
    /// Command prefix for `cargo verus` (a container wrapper), may be empty.
    pub toolchain: Vec<String>,
    /// Restrict to these crates (package names); empty means all verified members.
    pub crates: Vec<String>,
    /// Cargo target directory for the extraction builds (default: the toolchain's own).
    pub target_dir: Option<PathBuf>,
    /// Reuse an existing log instead of running Verus.
    pub reuse_logs: bool,
}

#[derive(Debug, Clone)]
pub struct Member {
    pub name: String,
    pub manifest: PathBuf,
    pub deps: Vec<String>,
}

impl Member {
    pub fn ident(&self) -> String {
        self.name.replace('-', "_")
    }
}

/// Verified workspace members (`[package.metadata.verus] verify = true`) in dependency order.
pub fn members_from_metadata(json: &str) -> Result<Vec<Member>> {
    let v: serde_json::Value = serde_json::from_str(json).context("parsing cargo metadata")?;
    let pkgs = v["packages"]
        .as_array()
        .ok_or_else(|| anyhow!("no packages in cargo metadata"))?;
    let mut all: BTreeMap<String, Member> = BTreeMap::new();
    for p in pkgs {
        if p["metadata"]["verus"]["verify"].as_bool() != Some(true) {
            continue;
        }
        let name = p["name"].as_str().unwrap_or_default().to_string();
        let deps = p["dependencies"]
            .as_array()
            .map(|d| {
                d.iter()
                    .filter_map(|x| x["name"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        all.insert(
            name.clone(),
            Member {
                name,
                manifest: PathBuf::from(p["manifest_path"].as_str().unwrap_or_default()),
                deps,
            },
        );
    }
    // Topological order over verified members; ties by name.
    let names: BTreeSet<String> = all.keys().cloned().collect();
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut order = Vec::new();
    while order.len() < all.len() {
        let next = all
            .values()
            .find(|m| {
                !done.contains(&m.name)
                    && m.deps
                        .iter()
                        .all(|d| !names.contains(d) || done.contains(d) || *d == m.name)
            })
            .ok_or_else(|| anyhow!("dependency cycle among verified crates"))?;
        done.insert(next.name.clone());
        order.push(next.clone());
    }
    Ok(order)
}

fn cargo_metadata(ws: &Path) -> Result<String> {
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(ws)
        .output()
        .context("running cargo metadata")?;
    if !out.status.success() {
        bail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `cargo verus build` arguments for one crate's extraction run (after the toolchain prefix).
pub fn verus_args(krate: &str, log_dir: &Path, target_dir: Option<&Path>) -> Vec<String> {
    let mut a: Vec<String> = ["cargo", "verus", "build", "-p", krate]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Some(t) = target_dir {
        a.extend(["--target-dir".to_string(), t.display().to_string()]);
    }
    a.extend(
        ["--fwd-verus-args-to", "roots", "--"]
            .iter()
            .map(|s| s.to_string()),
    );
    a.extend(
        [
            "--no-verify",
            "--log",
            "vir",
            "--log",
            "impl-names",
            "--log-dir",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    a.push(log_dir.display().to_string());
    a
}

pub fn clean_args(krate: &str, target_dir: Option<&Path>) -> Vec<String> {
    let mut a: Vec<String> = ["cargo", "clean", "-p", krate]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Some(t) = target_dir {
        a.extend(["--target-dir".to_string(), t.display().to_string()]);
    }
    a
}

/// `<toolchain prefix> <args>`; without a prefix the first argument is the program.
fn toolchain_command(toolchain: &[String], args: Vec<String>) -> Command {
    let mut all: Vec<String> = toolchain.to_vec();
    all.extend(args);
    let mut c = Command::new(&all[0]);
    c.args(&all[1..]);
    c
}

fn git(ws: &Path, args: &[&str]) -> Option<String> {
    let o = Command::new("git")
        .args(args)
        .current_dir(ws)
        .output()
        .ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

pub struct Summary {
    pub crates: usize,
    pub functions: usize,
    pub uses: usize,
    pub db: PathBuf,
    pub verus_seconds: f64,
    pub parse_load_seconds: f64,
    pub log_bytes: u64,
}

pub fn extract(o: &Options) -> Result<Summary> {
    let ws = o.workspace.canonicalize().context("workspace path")?;
    let out = if o.out.is_absolute() {
        o.out.clone()
    } else {
        ws.join(&o.out)
    };
    let ver: VerusVersion = version::query(&o.toolchain, &ws)?;
    version::check(&ver)?;

    let mut members = members_from_metadata(&cargo_metadata(&ws)?)?;
    if !o.crates.is_empty() {
        members.retain(|m| o.crates.contains(&m.name));
        if members.is_empty() {
            bail!(
                "no verified workspace member matches --crate {:?}",
                o.crates
            );
        }
    }
    std::fs::create_dir_all(&out)?;
    let db_path = out.join("facts.duckdb");
    let mut db = Db::create(&db_path)?;
    db.set_meta("verus_version", &ver.version)?;
    db.set_meta("verus_commit", &ver.commit)?;
    db.set_meta("rust_toolchain", &ver.toolchain)?;
    db.set_meta("tool_version", env!("CARGO_PKG_VERSION"))?;
    db.set_meta(
        "source_commit",
        &git(&ws, &["rev-parse", "HEAD"]).unwrap_or_default(),
    )?;
    db.set_meta(
        "source_dirty",
        &git(&ws, &["status", "--porcelain"])
            .map(|s| !s.is_empty())
            .unwrap_or(false)
            .to_string(),
    )?;
    let now = Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok();
    db.set_meta(
        "extracted_at",
        now.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .as_deref()
            .unwrap_or(""),
    )?;

    let (mut verus_s, mut parse_s, mut log_bytes) = (0.0, 0.0, 0u64);
    let (mut n_fn, mut n_use) = (0, 0);
    for m in &members {
        let log_dir = out.join("cache").join(&m.name).join("log");
        let vir = log_dir.join("crate.vir");
        if !(o.reuse_logs && vir.exists()) {
            let t = Instant::now();
            // Cargo treats a root crate as fresh when only the forwarded Verus
            // arguments changed, which would leave no log; clean it first.
            let mut clean =
                toolchain_command(&o.toolchain, clean_args(&m.name, o.target_dir.as_deref()));
            eprintln!("extract {}: cargo clean -p {}", m.name, m.name);
            let st = clean
                .current_dir(&ws)
                .status()
                .with_context(|| format!("cleaning {}", m.name))?;
            if !st.success() {
                bail!("cargo clean failed for crate {} ({st})", m.name);
            }
            let mut cmd = toolchain_command(
                &o.toolchain,
                verus_args(&m.name, &log_dir, o.target_dir.as_deref()),
            );
            eprintln!("extract {}: running Verus --no-verify", m.name);
            let st = cmd
                .current_dir(&ws)
                .status()
                .with_context(|| format!("running Verus for {}", m.name))?;
            if !st.success() {
                bail!("Verus failed for crate {} ({st})", m.name);
            }
            verus_s += t.elapsed().as_secs_f64();
            if !vir.exists() {
                bail!("Verus wrote no {} for crate {}", vir.display(), m.name);
            }
            std::fs::write(
                log_dir.join("version.json"),
                format!(
                    "{{\"version\":\"{}\",\"commit\":\"{}\"}}\n",
                    ver.version, ver.commit
                ),
            )?;
        }
        let t = Instant::now();
        let text =
            std::fs::read_to_string(&vir).with_context(|| format!("reading {}", vir.display()))?;
        let names: ImplNames = std::fs::read_to_string(log_dir.join("crate.impl_names"))
            .map(|t| parse_impl_names(&t))
            .unwrap_or_default();
        let facts = parse_log(&text, &m.ident(), &names).map_err(|e| anyhow!("{}: {e}", m.name))?;
        let manifest = m
            .manifest
            .strip_prefix(&ws)
            .unwrap_or(&m.manifest)
            .display()
            .to_string();
        db.load_crate(
            &facts,
            &CrateInfo {
                manifest: &manifest,
                log_bytes: text.len() as u64,
            },
        )?;
        scan_module_uses(&db, &ws, &m.manifest, &facts)?;
        n_fn += facts.functions.len();
        n_use += facts.uses.len();
        log_bytes += text.len() as u64;
        parse_s += t.elapsed().as_secs_f64();
        eprintln!(
            "extract {}: {} functions, {} uses",
            m.name,
            facts.functions.len(),
            facts.uses.len()
        );
    }
    db.resolve()?;
    Ok(Summary {
        crates: members.len(),
        functions: n_fn,
        uses: n_use,
        db: db_path,
        verus_seconds: verus_s,
        parse_load_seconds: parse_s,
        log_bytes,
    })
}

/// Module-level `broadcast use` from a scan of the crate's source files.
fn scan_module_uses(
    db: &Db,
    ws: &Path,
    manifest: &Path,
    facts: &crate::vir::CrateFacts,
) -> Result<()> {
    use crate::scan::{candidates, scan_file};
    let files: BTreeSet<&str> = facts.functions.iter().map(|f| f.file.as_str()).collect();
    let fn_paths: BTreeSet<&str> = facts.functions.iter().map(|f| f.path.as_str()).collect();
    let _ = manifest;
    for file in files {
        let p = ws.join(file);
        let Ok(src) = std::fs::read_to_string(&p) else {
            db.warn(&facts.krate, "scan_unreadable_file", file)?;
            continue;
        };
        for u in scan_file(&src, file, facts) {
            let cands = candidates(&u.path, &u.module, &facts.krate);
            let own = |c: &String| fn_paths.contains(c.as_str()) || facts.groups.contains(c);
            let callee = match cands.iter().find(|c| own(c)) {
                Some(c) => c.clone(),
                None => {
                    // Not an item of this crate: keep the path as written (`crate::` expanded).
                    let root = u.path.split("::").next().unwrap_or("");
                    if !matches!(root, "vstd" | "core" | "alloc" | "std" | "builtin") {
                        db.warn(
                            &facts.krate,
                            "unresolved_broadcast_use",
                            &format!("{file}:{}: {}", u.line, u.path),
                        )?;
                    }
                    u.path
                        .strip_prefix("crate::")
                        .map_or(u.path.clone(), |r| format!("{}::{r}", facts.krate))
                }
            };
            db.conn.execute(
                "INSERT INTO module_uses VALUES (?, ?, NULL, 'broadcast_use', ?, ?)",
                duckdb::params![u.module, callee, file, u.line],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const META: &str = r#"{"packages":[
      {"name":"top","manifest_path":"/w/top/Cargo.toml","metadata":{"verus":{"verify":true}},
       "dependencies":[{"name":"mid"},{"name":"serde"}]},
      {"name":"mid","manifest_path":"/w/mid/Cargo.toml","metadata":{"verus":{"verify":true}},
       "dependencies":[{"name":"leaf-c"}]},
      {"name":"leaf-c","manifest_path":"/w/leaf/Cargo.toml","metadata":{"verus":{"verify":true}},"dependencies":[]},
      {"name":"plain","manifest_path":"/w/plain/Cargo.toml","metadata":null,"dependencies":[]}]}"#;

    #[test]
    fn dependency_order_and_filter() {
        let m = members_from_metadata(META).unwrap();
        assert_eq!(
            m.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
            ["leaf-c", "mid", "top"]
        );
        assert_eq!(m[0].ident(), "leaf_c");
    }

    #[test]
    fn per_crate_log_dir_args() {
        let a = verus_args("c", Path::new("/o/c/log"), None);
        let s = a.join(" ");
        assert!(s.contains("-p c --fwd-verus-args-to roots -- --no-verify --log vir --log impl-names --log-dir /o/c/log"));
    }
}
