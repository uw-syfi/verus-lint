//! Example rules crate: one Rust rule, run through the verus-lint command line.
//!
//! Configure it in `verus-lint.toml`:
//!
//! ```toml
//! [rules]
//! rust = "lints"   # directory of this crate, relative to the workspace
//! ```
//!
//! then `verus-lint check` builds this crate and runs its binary.

use std::collections::BTreeSet;
use std::process::ExitCode;
use verus_lint::rules::Finding;
use verus_lint::sdk::{Cx, Findings, Mode, Ratchet, Rule, RuleMeta, Severity, UseKind};

/// Opaque spec functions that are revealed from many modules.
///
/// Closing a definition (making it opaque) only helps proof stability when few places have to
/// reveal it again. The rule counts the distinct modules that contain a `reveal` of each opaque
/// spec function and reports those above `max_modules`, worst first.
struct OpaqueRevealSpread;

impl Rule for OpaqueRevealSpread {
    fn meta(&self) -> RuleMeta {
        RuleMeta::new(
            "example/opaque-reveal-spread",
            "Opaque spec function revealed from many modules.",
        )
        .severity(Severity::Warning)
        .param("max_modules", 8)
        .ratchet(Ratchet::Metric {
            ratio: 1.0,
            abs: 0.0,
        })
    }

    fn check(&self, cx: &Cx, out: &mut Findings) -> anyhow::Result<()> {
        let max = cx.params.get_u64("max_modules")?;
        for def in cx
            .facts
            .functions()
            .iter()
            .filter(|f| f.mode == Mode::Spec && f.opaque)
        {
            let modules: BTreeSet<&str> = cx
                .facts
                .uses_of(def.id)
                .filter(|u| u.kind == UseKind::Reveal)
                .map(|u| cx.facts.function(u.caller).module.as_str())
                .collect();
            let n = u32::try_from(modules.len())?;
            if u64::from(n) > max {
                out.push(
                    Finding::at(def, format!("revealed from {n} modules (limit {max})"))
                        .metric(f64::from(n)),
                );
            }
        }
        Ok(())
    }
}

fn main() -> ExitCode {
    verus_lint::run(&[&OpaqueRevealSpread])
}
