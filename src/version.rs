//! Verus release detection: from `verus --version --output-json` and from the `verus` section
//! of a report. Which releases are accepted is decided in `crate::adapters`.

use anyhow::{Context, Result, bail};
use std::process::Command;

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

/// The release that produced a `--output-json` report object, from its `verus` section.
///
/// # Errors
/// Fails when the object has no `verus.version` and `verus.commit`.
pub fn from_report(obj: &serde_json::Value) -> Result<VerusVersion> {
    let v = &obj["verus"];
    let s = |k: &str| v[k].as_str().map(String::from);
    let (Some(version), Some(commit)) = (s("version"), s("commit")) else {
        bail!("report has no verus.version and verus.commit");
    };
    Ok(VerusVersion {
        version,
        commit,
        toolchain: s("toolchain").unwrap_or_default(),
    })
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
{"verus": {"profile": "release", "version": "0.2026.08.30.b432e82", "toolchain": "1.97.1",
 "commit": "b432e82fed7e05090fd53b5e5fc39020f725aabe"}}
"#;

    #[test]
    fn parses_version_json_after_noise() {
        let v = parse_version_json(OUT).unwrap();
        assert_eq!(v.version, "0.2026.08.30.b432e82");
        assert_eq!(v.commit, "b432e82fed7e05090fd53b5e5fc39020f725aabe");
        assert_eq!(v.toolchain, "1.97.1");
    }

    #[test]
    fn reads_a_reports_verus_section() {
        let o: serde_json::Value = serde_json::from_str(OUT.trim_start_matches("noise\n")).unwrap();
        assert_eq!(from_report(&o).unwrap(), parse_version_json(OUT).unwrap());
        assert!(from_report(&serde_json::json!({"times-ms": {}})).is_err());
    }

    #[test]
    fn rejects_malformed() {
        assert!(parse_version_json("no json").is_err());
        assert!(parse_version_json("{\"x\": 1}").is_err());
    }
}
