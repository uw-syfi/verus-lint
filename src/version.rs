//! Strict Verus version check. The VIR log has no stability promise, so the
//! extractor accepts only releases it has a parser profile for.

use anyhow::{Context, Result, bail};
use std::process::Command;

/// A supported Verus release.
pub struct Supported {
    /// Release version string.
    pub version: &'static str,
    /// Full git commit of the release.
    pub commit: &'static str,
}

/// Supported Verus releases. Adding one means a fixture crate for it.
pub const SUPPORTED: &[Supported] = &[Supported {
    version: "0.2026.07.18.3a4d30b",
    commit: "3a4d30bcdc4571e7927af97be9c4664973083eda",
}];

#[derive(Debug, PartialEq, Eq, Clone)]
/// The `verus` object of `verus --version --output-json`.
pub struct VerusVersion {
    /// Release version string.
    pub version: String,
    /// Full git commit.
    pub commit: String,
    /// Rust toolchain Verus was built with (may be empty).
    pub toolchain: String,
}

/// Extract the `verus` object from the output of `verus --version --output-json`.
/// Container wrappers may print other lines first; parsing starts at the first `{`.
///
/// # Errors
/// Fails when the operation's I/O, parsing or database step fails; the error says which.
pub fn parse_version_json(out: &str) -> Result<VerusVersion> {
    let start = out
        .find('{')
        .context("no JSON object in `verus --version --output-json` output")?;
    let mut de = serde_json::Deserializer::from_str(&out[start..]).into_iter::<serde_json::Value>();
    let v = de.next().context("empty output")??;
    let o = v.get("verus").context("missing `verus` object")?;
    let s = |k: &str| o.get(k).and_then(|x| x.as_str()).map(String::from);
    Ok(VerusVersion {
        version: s("version").context("missing verus.version")?,
        commit: s("commit").context("missing verus.commit")?,
        toolchain: s("toolchain").unwrap_or_default(),
    })
}

/// Whether the Verus version is in the supported range.
#[must_use]
pub fn is_supported(v: &VerusVersion) -> bool {
    SUPPORTED.iter().any(|s| s.commit == v.commit)
}

/// Error for an unsupported release; the CLI maps it to exit status 3.
#[derive(Debug)]
pub struct Unsupported(pub String);
impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Unsupported {}

/// Check the Verus version.
///
/// # Errors
/// Fails with `Unsupported` outside the supported range.
pub fn check(v: &VerusVersion) -> Result<()> {
    if is_supported(v) {
        return Ok(());
    }
    let list: Vec<String> = SUPPORTED
        .iter()
        .map(|s| format!("{} ({})", s.version, s.commit))
        .collect();
    bail!(Unsupported(format!(
        "unsupported Verus {} (commit {}); supported: {}",
        v.version,
        v.commit,
        list.join(", ")
    )))
}

/// Run `<toolchain> verus --version --output-json` (plain `verus` without a wrapper).
///
/// # Errors
/// Fails when the operation's I/O, parsing or database step fails; the error says which.
pub fn query(toolchain: &[String], cwd: &std::path::Path) -> Result<VerusVersion> {
    let mut cmd = match toolchain.split_first() {
        Some((p, rest)) => {
            let mut c = Command::new(p);
            c.args(rest).arg("verus");
            c
        }
        None => Command::new("verus"),
    };
    let out = cmd
        .current_dir(cwd)
        .args(["--version", "--output-json"])
        .output()
        .context("running verus --version")?;
    if !out.status.success() {
        bail!(
            "verus --version failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    parse_version_json(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: &str = r#"noise
{"verus": {"profile": "release", "version": "0.2026.07.18.3a4d30b", "toolchain": "1.96.0",
 "commit": "3a4d30bcdc4571e7927af97be9c4664973083eda"}}
"#;

    #[test]
    fn parses_and_accepts_supported() {
        let v = parse_version_json(OUT).unwrap();
        assert_eq!(v.version, "0.2026.07.18.3a4d30b");
        check(&v).unwrap();
    }

    #[test]
    fn refuses_other_commit() {
        let v = parse_version_json(&OUT.replace("3a4d30bcdc", "0000000000")).unwrap();
        let e = check(&v).unwrap_err();
        assert!(e.downcast_ref::<Unsupported>().is_some());
        assert!(e.to_string().contains("supported:"));
    }

    #[test]
    fn rejects_malformed() {
        assert!(parse_version_json("no json").is_err());
        assert!(parse_version_json("{\"x\": 1}").is_err());
    }
}
