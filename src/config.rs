//! `verus-lint.toml`: extraction, roots, rule, level and baseline settings.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[allow(
    missing_docs,
    reason = "plain data row; field names match the schema columns"
)]
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExtractCfg {
    /// Command prefix for `cargo verus`, for example `["./verify"]`.
    pub toolchain: Vec<String>,
    /// Members to extract: package names or manifest directories (globs); empty means all verified members.
    pub crates: Vec<String>,
    /// Members to skip (same patterns), for `verify = true` members Verus cannot build.
    pub exclude: Vec<String>,
}

#[allow(
    missing_docs,
    reason = "plain data row; field names match the schema columns"
)]
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RootsCfg {
    /// Glob patterns for top theorems, negative controls and fixtures. A pattern with `::` matches
    /// the function path (or its friendly path); one without matches the bare function name.
    pub patterns: Vec<String>,
    /// Treat every `pub` function of the extracted crates as a root.
    pub public_api: bool,
    /// API pin files (globs, relative to the workspace): one function path or friendly name per line.
    /// Pinned items are not roots; they are reported as unused public API.
    pub pins: Vec<String>,
    /// Treat pinned functions as roots too (they are then live, and still listed by rules that
    /// read `api_pins`). Off by default: a pinned function nothing uses is unused API, and the
    /// helpers only it calls are dead.
    pub pins_are_roots: bool,
    /// Files outside the Rust sources that mention functions by name (globs relative to the
    /// workspace; `!pattern` excludes). Every function named by an identifier token in such a
    /// file is a root. For tooling that checks or drives the proofs: pin files, scripts, lists.
    pub name_files: Vec<String>,
}

#[allow(
    missing_docs,
    reason = "plain data row; field names match the schema columns"
)]
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RulesCfg {
    /// Directories of SQL rules, relative to the workspace. No rules are built in.
    pub dirs: Vec<String>,
    /// Directory of a Rust rules crate (holds a `Cargo.toml`), relative to the workspace.
    /// `check` and `run` build it and let its binary do the checking.
    pub rust: Option<String>,
    /// Cargo profile for building the rules crate (default `release`).
    pub rust_profile: Option<String>,
    /// Level per rule id: `off`, `note`, `warn` or `gate`. Rules not listed are `warn`.
    pub levels: BTreeMap<String, String>,
    /// Parameter values per rule id (numbers), overriding the rule's defaults.
    pub params: BTreeMap<String, BTreeMap<String, toml::Value>>,
}

#[allow(
    missing_docs,
    reason = "plain data row; field names match the schema columns"
)]
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BaselineCfg {
    /// Baseline file, relative to the workspace (default `verus-lint-baseline.json`).
    pub file: Option<String>,
}

/// What a rule's findings do to the exit status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// The rule is not run.
    Off,
    /// Findings are printed as notes and never change the exit status.
    Note,
    /// Findings are printed at the rule's severity and never change the exit status.
    Warn,
    /// Findings the baseline does not cover make `check` exit with status 1.
    Gate,
}

impl Level {
    /// Parse `off`, `note`, `warn` or `gate`.
    ///
    /// # Errors
    /// Fails on any other text.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "off" => Ok(Self::Off),
            "note" => Ok(Self::Note),
            "warn" => Ok(Self::Warn),
            "gate" => Ok(Self::Gate),
            _ => anyhow::bail!("bad level `{s}` (off, note, warn or gate)"),
        }
    }

    /// The text form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Note => "note",
            Self::Warn => "warn",
            Self::Gate => "gate",
        }
    }
}

#[allow(
    missing_docs,
    reason = "plain data row; field names match the schema columns"
)]
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub extract: ExtractCfg,
    pub roots: RootsCfg,
    pub rules: RulesCfg,
    pub baseline: BaselineCfg,
}

impl Config {
    /// Parse config text.
    ///
    /// # Errors
    /// Fails on invalid TOML or unknown keys in `[extract]`, `[roots]` or `[rules]`.
    pub fn parse(text: &str) -> Result<Self> {
        let c: Self = toml::from_str(text).context("parsing verus-lint.toml")?;
        for (id, l) in &c.rules.levels {
            Level::parse(l).with_context(|| format!("[rules.levels] \"{id}\""))?;
        }
        for (id, ps) in &c.rules.params {
            for (k, v) in ps {
                if !(v.is_integer() || v.is_float()) {
                    anyhow::bail!("[rules.params.\"{id}\"] {k} must be a number");
                }
            }
        }
        Ok(c)
    }

    /// Level of a rule (`warn` unless the config says otherwise).
    ///
    /// # Errors
    /// Fails when the configured level text is invalid.
    pub fn level(&self, rule: &str) -> Result<Level> {
        self.rules
            .levels
            .get(rule)
            .map_or(Ok(Level::Warn), |l| Level::parse(l))
    }

    /// Configured parameter values of a rule as text.
    #[must_use]
    pub fn rule_params(&self, rule: &str) -> BTreeMap<String, String> {
        self.rules
            .params
            .get(rule)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.to_string())).collect())
            .unwrap_or_default()
    }

    /// Read and parse a config file.
    ///
    /// # Errors
    /// Fails when the file is unreadable or invalid.
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| path.display().to_string())
    }
}

/// Glob match where `*` matches any run of characters (including `::` and `/`).
#[must_use]
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
        assert!(glob_match("*::theorem_*", "proj::a::theorem_x"));
        assert!(!glob_match("*::theorem_*", "proj::a::lemma_x"));
        assert!(glob_match("crates/**", "crates/a/b"));
        assert!(glob_match("a", "a") && !glob_match("a", "ab"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn levels_and_params() {
        let c =
            Config::parse("[rules.levels]\n\"a/b\"=\"gate\"\n[rules.params.\"a/b\"]\nn=3\nr=0.5\n")
                .unwrap();
        assert_eq!(c.level("a/b").unwrap(), Level::Gate);
        assert_eq!(c.level("other").unwrap(), Level::Warn);
        assert_eq!(c.rule_params("a/b")["n"], "3");
        assert_eq!(c.rule_params("a/b")["r"], "0.5");
        assert!(Config::parse("[rules.levels]\nx=\"loud\"\n").is_err());
        assert!(Config::parse("[rules.params.x]\nn=\"s\"\n").is_err());
    }

    #[test]
    fn parses_sections() {
        let c = Config::parse(
            "[extract]\ntoolchain=[\"./v\"]\nexclude=[\"my-cuda-sys\"]\n[roots]\npatterns=[\"*::neg_*\"]\n[rules]\ndirs=[\"x\"]\n[baseline]\nfile=\"b.json\"\n",
        )
        .unwrap();
        assert_eq!(c.extract.exclude, ["my-cuda-sys"]);
        assert_eq!(c.roots.patterns, ["*::neg_*"]);
        assert_eq!(c.rules.dirs, ["x"]);
        assert!(Config::parse("[extract]\nbogus=1\n").is_err());
        assert_eq!(c.baseline.file.as_deref(), Some("b.json"));
    }
}
