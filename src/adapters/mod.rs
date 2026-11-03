//! The one dispatch point between Verus releases and the fact model.
//!
//! Each supported Verus release maps to an adapter: a module that parses that release's VIR log,
//! `--log impl-names` output and `--output-json` report into the normalized rows of
//! `crate::facts`. Consecutive releases that share a format share an adapter. Everything outside
//! this module sees only normalized facts, so rules never branch on the Verus version.
//! `strata.toml` enforces that only this file names an adapter and that adapters do not reference
//! each other. See `docs/verus-versions.md` for adding a release.

mod v2026_07;

use crate::facts::{CrateFacts, Report};
use crate::version::VerusVersion;
use anyhow::{Result, bail};

/// Parser for one VIR-format generation.
pub trait Adapter: Sync {
    /// Short name of the format generation (for messages).
    fn name(&self) -> &'static str;

    /// Facts of crate `krate` from its `crate.vir` text and `--log impl-names` text.
    ///
    /// # Errors
    /// Fails closed on a form the adapter interprets that does not have the expected shape.
    fn parse_crate(&self, vir: &str, impl_names: &str, krate: &str) -> Result<CrateFacts, String>;

    /// One crate's `--time-expanded --output-json` report object.
    ///
    /// # Errors
    /// Fails when the object lacks the fields the format requires.
    fn parse_report(&self, obj: &serde_json::Value) -> Result<Report, String>;
}

/// A supported Verus release and the adapter that reads its output.
pub struct Supported {
    /// Release version string.
    pub version: &'static str,
    /// Full git commit of the release.
    pub commit: &'static str,
    /// Adapter for the release's log and report format.
    pub adapter: &'static dyn Adapter,
}

/// Supported Verus releases, oldest first. Adding one means a fixture for it
/// (`tests/fixtures/fx/<version>/`) and an adapter, new or reused.
pub const SUPPORTED: &[Supported] = &[
    Supported {
        version: "0.2026.07.18.3a4d30b",
        commit: "3a4d30bcdc4571e7927af97be9c4664973083eda",
        adapter: &v2026_07::V2026_07,
    },
    Supported {
        version: "0.2026.08.30.b432e82",
        commit: "b432e82fed7e05090fd53b5e5fc39020f725aabe",
        adapter: &v2026_07::V2026_07,
    },
];

/// Error for an unsupported release; the CLI maps it to exit status 3.
#[derive(Debug)]
pub struct Unsupported(pub String);
impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Unsupported {}

/// The supported release with this commit, if any.
#[must_use]
pub fn supported(v: &VerusVersion) -> Option<&'static Supported> {
    SUPPORTED.iter().find(|s| s.commit == v.commit)
}

/// The adapter for a detected Verus release.
///
/// # Errors
/// Fails with `Unsupported`, naming the detected release and the supported list.
pub fn select(v: &VerusVersion) -> Result<&'static dyn Adapter> {
    if let Some(s) = supported(v) {
        return Ok(s.adapter);
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

/// The adapter of the newest supported release (for tests and tools that load saved logs).
#[must_use]
pub fn newest() -> &'static dyn Adapter {
    SUPPORTED[SUPPORTED.len() - 1].adapter
}

/// Normalize a report object: detect its release from the `verus` section, then parse it with
/// that release's adapter.
///
/// # Errors
/// Fails with `Unsupported` for another release, or when the object is malformed.
pub fn parse_report(obj: &serde_json::Value) -> Result<Report> {
    let v = crate::version::from_report(obj)?;
    select(&v)?
        .parse_report(obj)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ver(commit: &str) -> VerusVersion {
        VerusVersion {
            version: "x".into(),
            commit: commit.into(),
            toolchain: String::new(),
        }
    }

    #[test]
    fn every_supported_release_selects_an_adapter() {
        for s in SUPPORTED {
            assert!(select(&ver(s.commit)).is_ok(), "{}", s.version);
        }
    }

    #[test]
    fn unknown_release_names_itself_and_the_supported_list() {
        let e = select(&VerusVersion {
            version: "0.2099.01.01.0000000".into(),
            commit: "0000000".into(),
            toolchain: String::new(),
        })
        .err()
        .unwrap();
        assert!(e.downcast_ref::<Unsupported>().is_some());
        let m = e.to_string();
        assert!(m.contains("0.2099.01.01.0000000"), "{m}");
        for s in SUPPORTED {
            assert!(m.contains(s.version), "{m}");
        }
    }
}
