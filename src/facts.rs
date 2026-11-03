//! The normalized fact model: what every Verus adapter produces and everything downstream
//! (database, rules, reports) consumes. Nothing here depends on a Verus release; an adapter maps
//! its release's logs and reports onto these rows. A fact a release cannot provide is `None`
//! (stored as NULL), never a guessed value.

#[derive(Debug, Default, Clone)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "mirrors the boolean columns of the functions table"
)]
pub struct FunctionRow {
    pub path: String,
    pub friendly: String,
    pub krate: String,
    pub module: String,
    pub name: String,
    pub self_type: Option<String>,
    pub trait_path: Option<String>,
    pub mode: String,
    pub kind: String,
    pub item_kind: String,
    pub vis: String,
    pub body_vis: String,
    pub opaque: bool,
    pub reveal_vis: Option<String>,
    pub external_body: bool,
    pub broadcast_forall: bool,
    pub broadcast_forall_only: bool,
    pub rlimit_attr: Option<String>,
    pub spinoff_prover: bool,
    pub integer_ring: bool,
    pub bit_vector: bool,
    pub nonlinear: bool,
    pub has_body: bool,
    pub file: String,
    pub line: u32,
    pub end_line: u32,
    pub body_lines: u32,
    pub n_requires: u32,
    pub n_ensures: u32,
    /// Trait method declaration with a default body.
    pub has_default: bool,
    /// For a trait method implementation: path of the declaration it implements.
    pub trait_method: Option<String>,
    /// `#[verifier::type_invariant]` function: used implicitly by the verifier.
    pub type_invariant: bool,
    /// Compiler-generated datatype field accessor (`arrow_Variant_0`): no source item to delete.
    pub generated: bool,
}

#[derive(Debug, Clone)]
pub struct UseRow {
    /// Index into `CrateFacts::functions`.
    pub caller: usize,
    pub callee_path: String,
    pub section: &'static str,
    pub kind: &'static str,
    pub in_trigger: bool,
    pub fuel: Option<String>,
    pub file: String,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone)]
pub struct QuantRow {
    pub caller: usize,
    /// forall, exists or choose.
    pub quant: &'static str,
    /// explicit (`#[trigger]` or `#![trigger ..]`), `auto_annotation` (`#![auto]`), or none.
    pub trigger: &'static str,
    pub n_triggers: u32,
    pub section: &'static str,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub struct TrustedRow {
    pub caller: Option<usize>,
    /// assume, admit, `external_body`, `external_fn`, `external_type`, `assume_specification`, `broadcast_axiom`.
    pub kind: &'static str,
    pub file: String,
    pub line: u32,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct TraitImplRow {
    pub impl_path: String,
    pub trait_path: String,
    pub self_type: String,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Default)]
pub struct CrateFacts {
    pub krate: String,
    pub functions: Vec<FunctionRow>,
    pub uses: Vec<UseRow>,
    pub modules: Vec<String>,
    /// Broadcast group paths owned by this crate (`(group_id ..)` forms).
    pub groups: Vec<String>,
    pub quants: Vec<QuantRow>,
    pub trusted: Vec<TrustedRow>,
    pub trait_impls: Vec<TraitImplRow>,
    /// Own-crate `external_fn` and `external_type` ids: (kind, path).
    pub externals: Vec<(&'static str, String)>,
    pub forms: usize,
}

/// One function's entry in a verification report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnCost {
    /// Module the function was verified under, as the report names it (without the crate).
    pub module: String,
    /// Function name as Verus prints it (the friendly name).
    pub function: String,
    /// `exec`, `proof` or `spec`; `None` when the release does not report it.
    pub mode: Option<String>,
    /// SMT resource count.
    pub rlimit: i64,
    /// SMT time in microseconds.
    pub time_us: i64,
    /// Whether the function verified.
    pub success: bool,
}

/// One module's entry in a verification report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleCost {
    /// Module name as the report spells it (without the crate).
    pub module: String,
    /// SMT resource count of the module.
    pub rlimit: i64,
    /// SMT time in milliseconds.
    pub smt_time_ms: i64,
    /// Longest verification session of the module in milliseconds.
    pub session_time_ms: i64,
}

/// One crate's verification report (`--time-expanded --output-json`), normalized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Verus release that produced the report.
    pub version: String,
    /// Full git commit of that release.
    pub commit: String,
    /// Per-function costs.
    pub functions: Vec<FnCost>,
    /// Per-module costs.
    pub modules: Vec<ModuleCost>,
}
