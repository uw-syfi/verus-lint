//! Source scan for module-level `broadcast use` items.
//!
//! Verus's log printer drops `ModuleX::reveals`, so a module-level
//! `broadcast use` does not appear in the VIR log. Function-local ones do
//! (as `Fuel .. true`). The scan finds every `broadcast use` item in a source
//! file and skips those inside a known function span, which leaves the
//! module-level ones.

use crate::vir::CrateFacts;
use std::collections::BTreeSet;

#[derive(Debug, PartialEq, Eq)]
pub struct ScanUse {
    pub module: String,
    pub path: String,
    pub line: u32,
}

/// Blank out comments and string literals, keeping newlines so line numbers hold.
fn blank(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                out.push(' ');
                i += 1;
            }
        } else if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            let mut depth = 0;
            while i < b.len() {
                if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(if b[i] == b'\n' { '\n' } else { ' ' });
                    i += 1;
                }
            }
        } else if b[i] == b'"' {
            out.push(' ');
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    out.push(' ');
                    i += 1;
                }
                out.push(if b[i] == b'\n' { '\n' } else { ' ' });
                i += 1;
            }
            out.push(' ');
            i += 1;
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// Names of one `broadcast use` item body (text between `use` and `;`):
/// `a::b::c`, `{a::b, c::d}`, `a::{b, c}`.
fn expand(body: &str) -> Vec<String> {
    fn split_top(s: &str) -> Vec<&str> {
        let (mut depth, mut start, mut parts) = (0, 0, Vec::new());
        for (i, c) in s.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                ',' if depth == 0 => {
                    parts.push(&s[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        parts.push(&s[start..]);
        parts
    }
    fn rec(prefix: &str, s: &str, out: &mut Vec<String>) {
        let s = s.trim();
        if let Some(open) = s.find('{') {
            let close = s.rfind('}').unwrap_or(s.len());
            let pre = format!("{prefix}{}", s[..open].trim());
            for part in split_top(&s[open + 1..close]) {
                rec(&pre, part, out);
            }
        } else if !s.is_empty() {
            out.push(format!("{prefix}{s}"));
        }
    }
    let mut out = Vec::new();
    for part in split_top(body) {
        rec("", part, &mut out);
    }
    out.into_iter()
        .map(|p| p.split_whitespace().collect::<String>())
        .collect()
}

/// Scan `src` (the text of `file`) for `broadcast use` items outside any function span.
pub fn scan_file(src: &str, file: &str, facts: &CrateFacts) -> Vec<ScanUse> {
    let text = blank(src);
    let spans: Vec<(u32, u32)> = facts
        .functions
        .iter()
        .filter(|f| f.file == file)
        .map(|f| (f.line, f.end_line))
        .collect();
    // Module of a line: the module of the nearest function at or after it, else before it.
    let mut fns: Vec<(u32, &str)> = facts
        .functions
        .iter()
        .filter(|f| f.file == file)
        .map(|f| (f.line, f.module.as_str()))
        .collect();
    fns.sort();
    let crate_root = || facts.krate.clone();
    let mut out = Vec::new();
    let mut offset = 0;
    let bytes = text.as_bytes();
    while let Some(p) = text[offset..].find("broadcast") {
        let at = offset + p;
        offset = at + 9;
        let before_ok =
            at == 0 || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
        let rest = &text[at + 9..];
        let after = rest.trim_start();
        if !before_ok || rest.len() == after.len() || !after.starts_with("use") {
            continue;
        }
        let after_use = &after[3..];
        if !after_use.starts_with(char::is_whitespace) && !after_use.starts_with('{') {
            continue;
        }
        let Some(end) = after_use.find(';') else {
            continue;
        };
        let line = text[..at].matches('\n').count() as u32 + 1;
        if spans.iter().any(|&(a, b)| a <= line && line <= b) {
            continue; // function-local: present in the log
        }
        let module = fns
            .iter()
            .find(|(l, _)| *l >= line)
            .or(fns.last())
            .map(|(_, m)| m.to_string())
            .unwrap_or_else(crate_root);
        for path in expand(&after_use[..end]) {
            out.push(ScanUse {
                module: module.clone(),
                path,
                line,
            });
        }
        offset = at + 9 + (rest.len() - after.len()) + 3 + end;
    }
    out
}

/// Resolve a scanned path to a candidate absolute VIR path set: as written,
/// `crate::` rewritten, and relative to the module and its ancestors.
pub fn candidates(path: &str, module: &str, krate: &str) -> Vec<String> {
    let mut c = BTreeSet::new();
    if let Some(r) = path.strip_prefix("crate::") {
        c.insert(format!("{krate}::{r}"));
    } else if let Some(r) = path.strip_prefix("self::") {
        c.insert(format!("{module}::{r}"));
    } else {
        c.insert(path.to_string());
        let mut m = module;
        loop {
            c.insert(format!("{m}::{path}"));
            match m.rsplit_once("::") {
                Some((p, _)) => m = p,
                None => break,
            }
        }
    }
    c.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vir::{ImplNames, parse_log};

    #[test]
    fn expands_braces() {
        assert_eq!(expand(" vstd::a::b "), ["vstd::a::b"]);
        assert_eq!(expand(" {vstd::a::b, x::y}"), ["vstd::a::b", "x::y"]);
        assert_eq!(expand(" vstd::{a::b,\n c}"), ["vstd::a::b", "vstd::c"]);
        assert_eq!(
            expand(" vstd::a::b, vstd::c::d"),
            ["vstd::a::b", "vstd::c::d"]
        );
        assert_eq!(expand(" {x::{y, z}, w}"), ["x::y", "x::z", "w"]);
    }

    #[test]
    fn skips_comments_strings_and_function_local() {
        let facts = parse_log(
            include_str!("../tests/fixtures/mini.vir"),
            "mini",
            &ImplNames::new(),
        )
        .unwrap();
        // lemma_u of the fixture spans src/n.rs lines 8..16.
        let src = format!(
            "broadcast use vstd::a::group_a;\n// broadcast use vstd::no::comment;\n\
             let s = \"broadcast use vstd::no::string;\";\n{}proof fn lemma_u() {{\n    broadcast use vstd::local::group_l;\n}}\n{}broadcast use {{vstd::b::one, vstd::c::two}};\n",
            "\n".repeat(4),
            "\n".repeat(10)
        );
        let padded = src;
        let got = scan_file(&padded, "src/n.rs", &facts);
        let names: Vec<_> = got.iter().map(|u| u.path.as_str()).collect();
        assert_eq!(names, ["vstd::a::group_a", "vstd::b::one", "vstd::c::two"]);
        assert!(got.iter().all(|u| u.module == "mini::n"));
    }

    #[test]
    fn candidate_paths() {
        let c = candidates("crate::m::f", "mini::n", "mini");
        assert_eq!(c, ["mini::m::f"]);
        let c = candidates("m::f", "mini::n", "mini");
        assert!(c.contains(&"mini::m::f".to_string()) && c.contains(&"mini::n::m::f".to_string()));
    }
}
