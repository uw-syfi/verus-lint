//! Adapter for the VIR log and report format of Verus 0.2026.07.18 through 0.2026.08.30.
//!
//! Format notes (what this generation's logs look like, for the next adapter's author):
//! - `crate.vir` is a pretty-printed S-expression per item; functions are
//!   `(@ "span" (Function :name (Fun :path P) ...))`.
//! - 0.2026.08.30 prints `&&`, `||` and `==>` as `(> Logical (LogicalOp ..))` instead of
//!   `(> Binary (BinaryOp ..))` and adds function attributes such as
//!   `:impls_cannot_extend_spec`; the parser reads neither, so one parser serves both.
//! - 0.2026.08.30 gives compiler-generated field accessors (`arrow_*`) one `requires`
//!   clause; the facts report it as it is (`n_requires = 1`).
//! - `--log impl-names` lines are `impl ### trait ### self type, args ### span`.
//! - The report's per-function entries spell the mode key `"mode:"`.

mod report;
mod vir;

use crate::facts::{CrateFacts, Report};

/// The adapter value registered in `adapters::SUPPORTED`.
pub struct V2026_07;

impl super::Adapter for V2026_07 {
    fn name(&self) -> &'static str {
        "v2026_07"
    }

    fn parse_crate(&self, vir: &str, impl_names: &str, krate: &str) -> Result<CrateFacts, String> {
        let names = vir::parse_impl_names(impl_names);
        let mut facts = vir::parse_log(vir, krate, &names)?;
        vir::drop_generated_externals(&mut facts, impl_names);
        facts.trait_impls = vir::parse_impl_rows(impl_names, krate);
        Ok(facts)
    }

    fn parse_report(&self, obj: &serde_json::Value) -> Result<Report, String> {
        report::parse(obj)
    }
}
