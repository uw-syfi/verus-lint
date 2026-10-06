//! Dead-code analysis inputs: root patterns, API pins, and strongly connected components.

use crate::config::{RootsCfg, glob_match};
use crate::db::Db;
use crate::num::to_u32;
use anyhow::Result;
use duckdb::params;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Convert a glob (`*` matches any run) to a SQL LIKE pattern with `\` as the escape.
#[must_use]
pub fn glob_to_like(g: &str) -> String {
    let mut out = String::new();
    for c in g.chars() {
        match c {
            '*' => out.push('%'),
            '%' | '_' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

/// Entries of one pin file, each with its 1-based line.
///
/// `## name` headers when the file has any (a common generated layout), otherwise
/// every nonblank line that does not start with `#`. Each entry carries its 1-based line.
#[must_use]
pub fn parse_pin(text: &str) -> Vec<(String, u32)> {
    let has_headers = text.lines().any(|l| l.starts_with("## "));
    text.lines()
        .enumerate()
        .filter_map(|(i, l)| {
            let e = if has_headers {
                l.strip_prefix("## ")?.trim()
            } else if l.trim().is_empty() || l.trim_start().starts_with('#') {
                return None;
            } else {
                l.trim()
            };
            (!e.is_empty()).then(|| (e.to_string(), to_u32(i) + 1))
        })
        .collect()
}

/// Files under `ws` whose workspace-relative path matches `pattern` (`*` matches `/` too).
fn glob_files(ws: &Path, pattern: &str) -> Vec<PathBuf> {
    let prefix: String = pattern
        .split('/')
        .take_while(|s| !s.contains('*'))
        .collect::<Vec<_>>()
        .join("/");
    let mut out = Vec::new();
    let mut stack = vec![ws.join(&prefix)];
    while let Some(p) = stack.pop() {
        let Ok(md) = std::fs::metadata(&p) else {
            continue;
        };
        if md.is_dir() {
            if p.file_name().is_some_and(|n| n == "target" || n == ".git") {
                continue;
            }
            if let Ok(rd) = std::fs::read_dir(&p) {
                stack.extend(rd.filter_map(|e| e.ok().map(|e| e.path())));
            }
        } else if let Ok(rel) = p.strip_prefix(ws)
            && glob_match(pattern, &rel.display().to_string())
        {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// Identifier tokens of a text (`[A-Za-z_][A-Za-z0-9_]*`).
fn identifiers(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty() && !t.starts_with(|c: char| c.is_ascii_digit()))
}

/// Files selected by `name_files` globs (a leading `!` excludes).
fn name_file_list(ws: &Path, globs: &[String]) -> Vec<PathBuf> {
    let (neg, pos): (Vec<&String>, Vec<&String>) = globs.iter().partition(|g| g.starts_with('!'));
    let mut files: Vec<PathBuf> = pos.iter().flat_map(|g| glob_files(ws, g)).collect();
    files.sort();
    files.dedup();
    files.retain(|f| {
        let rel = f.strip_prefix(ws).unwrap_or(f).display().to_string();
        !neg.iter().any(|n| glob_match(&n[1..], &rel))
    });
    files
}

/// Store the roots section of the config: patterns, pin entries (matched to functions) and
/// the `public_api` switch.
///
/// # Errors
/// Fails when the operation's I/O, parsing or database step fails; the error says which.
pub fn store_roots(db: &Db, ws: &Path, cfg: &RootsCfg) -> Result<()> {
    for p in &cfg.patterns {
        db.conn.execute(
            "INSERT INTO root_patterns VALUES (?, ?, ?)",
            params![p, glob_to_like(p), !p.contains("::")],
        )?;
    }
    db.set_meta("roots_public_api", &cfg.public_api.to_string())?;
    db.set_meta("roots_pins", &cfg.pins_are_roots.to_string())?;
    store_root_names(db, ws, &cfg.name_files)?;
    for pat in &cfg.pins {
        for file in glob_files(ws, pat) {
            let rel = file.strip_prefix(ws).unwrap_or(&file).display().to_string();
            let text = std::fs::read_to_string(&file)?;
            for (entry, line) in parse_pin(&text) {
                let last = entry.rsplit("::").next().unwrap_or(&entry).to_string();
                // Exact path or friendly name, or a path ending in the entry; failing those,
                // a pub function of that name (pin keys name a layer, not a module path).
                let mut ids: Vec<i64> = db.query_ids(
                    "SELECT fn_id FROM functions WHERE path = ?1 OR friendly = ?1 OR path LIKE '%::' || ?1",
                    &[&entry],
                )?;
                if ids.is_empty() {
                    ids = db.query_ids(
                        "SELECT fn_id FROM functions WHERE name = ?1 AND vis = 'pub' AND kind = 'static'",
                        &[&last],
                    )?;
                }
                if ids.is_empty() {
                    db.conn.execute(
                        "INSERT INTO api_pins VALUES (?, ?, ?, NULL)",
                        params![entry, rel, line],
                    )?;
                }
                for id in ids {
                    db.conn.execute(
                        "INSERT INTO api_pins VALUES (?, ?, ?, ?)",
                        params![entry, rel, line, id],
                    )?;
                }
            }
        }
    }
    Ok(())
}

/// Fill `root_names` from the `name_files` globs: the functions whose name occurs as an
/// identifier token in one of the files. Files that are not UTF-8 text are skipped.
fn store_root_names(db: &Db, ws: &Path, globs: &[String]) -> Result<()> {
    if globs.is_empty() {
        return Ok(());
    }
    let known: std::collections::HashSet<String> = db
        .query_rows("SELECT DISTINCT name FROM functions")?
        .into_iter()
        .skip(1)
        .filter_map(|r| r.into_iter().next())
        .collect();
    let mut seen = std::collections::HashSet::new();
    for file in name_file_list(ws, globs) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let rel = file.strip_prefix(ws).unwrap_or(&file).display().to_string();
        for t in identifiers(&text) {
            if known.contains(t) && seen.insert(t.to_string()) {
                db.conn
                    .execute("INSERT INTO root_names VALUES (?, ?)", params![t, rel])?;
            }
        }
    }
    Ok(())
}

/// Strongly connected components of a directed graph on `0..n` (iterative Tarjan).
/// Returns the component index of each node; components are numbered in finishing order.
#[must_use]
pub fn sccs(n: usize, adj: &[Vec<usize>]) -> Vec<usize> {
    const UNSEEN: usize = usize::MAX;
    let mut index = vec![UNSEEN; n];
    let mut low = vec![0; n];
    let mut on = vec![false; n];
    let mut comp = vec![UNSEEN; n];
    let (mut stack, mut next, mut ncomp) = (Vec::new(), 0, 0);
    for root in 0..n {
        if index[root] != UNSEEN {
            continue;
        }
        let mut call: Vec<(usize, usize)> = vec![(root, 0)];
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on[root] = true;
        while let Some(&mut (v, ref mut ei)) = call.last_mut() {
            if *ei < adj[v].len() {
                let w = adj[v][*ei];
                *ei += 1;
                if index[w] == UNSEEN {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on[w] = true;
                    call.push((w, 0));
                } else if on[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                call.pop();
                if let Some(&(p, _)) = call.last() {
                    low[p] = low[p].min(low[v]);
                }
                if low[v] == index[v] {
                    while let Some(w) = stack.pop() {
                        on[w] = false;
                        comp[w] = ncomp;
                        if w == v {
                            break;
                        }
                    }
                    ncomp += 1;
                }
            }
        }
    }
    comp
}

/// Fill `dead_scc` with the components of the dead proof and spec functions.
///
/// # Errors
/// Fails when the operation's I/O, parsing or database step fails; the error says which.
pub fn store_dead_sccs(db: &Db) -> Result<()> {
    let dead: Vec<i64> = db.query_ids(
        "SELECT fn_id FROM functions WHERE mode IN ('proof', 'spec') AND NOT generated AND path NOT IN (SELECT path FROM live_nodes)",
        &[],
    )?;
    let idx: HashMap<i64, usize> = dead.iter().enumerate().map(|(i, &f)| (f, i)).collect();
    let mut adj = vec![Vec::new(); dead.len()];
    let mut stmt = db
        .conn
        .prepare("SELECT DISTINCT caller_id, callee_id FROM edges")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let (a, b): (i64, i64) = (r.get(0)?, r.get(1)?);
        if let (Some(&x), Some(&y)) = (idx.get(&a), idx.get(&b)) {
            adj[x].push(y);
        }
    }
    let comp = sccs(dead.len(), &adj);
    let mut min_id: HashMap<usize, i64> = HashMap::new();
    let mut size: HashMap<usize, i32> = HashMap::new();
    for (i, &c) in comp.iter().enumerate() {
        let m = min_id.entry(c).or_insert(dead[i]);
        *m = (*m).min(dead[i]);
        *size.entry(c).or_insert(0) += 1;
    }
    let mut a = db.conn.appender("dead_scc")?;
    for (i, &c) in comp.iter().enumerate() {
        a.append_row(params![dead[i], min_id[&c], size[&c]])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_patterns_escape() {
        assert_eq!(glob_to_like("*::theorem_*"), "%::theorem\\_%");
    }

    #[test]
    fn identifier_tokens() {
        let t: Vec<_> = identifiers("a_b = [\"c1\", 9x] # lemma::d").collect();
        assert_eq!(t, ["a_b", "c1", "lemma", "d"]);
    }

    #[test]
    fn pin_entries() {
        let text = "# gen\n## core::end_to_end\npub proof fn(a)\n  requires x\n## core::Term\n";
        assert_eq!(
            parse_pin(text),
            [
                ("core::end_to_end".to_string(), 2),
                ("core::Term".to_string(), 5)
            ]
        );
        let plain = "# c\nfoo::bar\n\n  baz\n";
        assert_eq!(
            parse_pin(plain),
            [("foo::bar".to_string(), 2), ("baz".to_string(), 4)]
        );
    }

    #[test]
    fn tarjan_components() {
        // 0 -> 1 -> 2 -> 0 is a cycle; 3 -> 0 and 4 alone.
        let adj = vec![vec![1], vec![2], vec![0], vec![0], vec![]];
        let c = sccs(5, &adj);
        assert_eq!(c[0], c[1]);
        assert_eq!(c[1], c[2]);
        assert_ne!(c[3], c[0]);
        assert_ne!(c[4], c[3]);
        // self-loop is its own component
        let c = sccs(2, &[vec![0], vec![]]);
        assert_ne!(c[0], c[1]);
    }
}
