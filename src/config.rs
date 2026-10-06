//! `verus-lint.toml`: extraction and roots settings.
//!
//! Sections the later phases own (`rules`, `baseline`) are accepted and ignored.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExtractCfg {
    /// Command prefix for `cargo verus`, for example `["./coral/verify"]`.
    pub toolchain: Vec<String>,
    /// Members to extract: package names or manifest directories (globs); empty means all verified members.
    pub crates: Vec<String>,
    /// Members to skip (same patterns), for `verify = true` members Verus cannot build.
    pub exclude: Vec<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RootsCfg {
    /// Glob patterns over function paths: top theorems, negative controls, fixtures.
    pub patterns: Vec<String>,
    /// Treat every `pub` function of the extracted crates as a root.
    pub public_api: bool,
    /// API pin files (globs, relative to the workspace): one function path or friendly name per line.
    /// Pinned items are not roots; they are reported as unused public API.
    pub pins: Vec<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub extract: ExtractCfg,
    pub roots: RootsCfg,
}

impl Config {
    pub fn parse(text: &str) -> Result<Config> {
        toml::from_str(text).context("parsing verus-lint.toml")
    }

    pub fn load(path: &Path) -> Result<Config> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| path.display().to_string())
    }
}

/// Glob match where `*` matches any run of characters (including `::` and `/`).
pub fn glob_match(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("*::theorem_*", "coral::a::theorem_x"));
        assert!(!glob_match("*::theorem_*", "coral::a::lemma_x"));
        assert!(glob_match("coral/crates/**", "coral/crates/a/b"));
        assert!(glob_match("a", "a") && !glob_match("a", "ab"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn parses_sections() {
        let c = Config::parse(
            "[extract]\ntoolchain=[\"./v\"]\nexclude=[\"sea-lion-cuda-sys\"]\n[roots]\npatterns=[\"*::neg_*\"]\n[rules]\ndirs=[\"x\"]\n",
        )
        .unwrap();
        assert_eq!(c.extract.exclude, ["sea-lion-cuda-sys"]);
        assert_eq!(c.roots.patterns, ["*::neg_*"]);
        assert!(Config::parse("[extract]\nbogus=1\n").is_err());
    }
}
