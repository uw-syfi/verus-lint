//! verus-lint: facts from Verus's VIR log, stored in `DuckDB`, checked by SQL rules.

pub mod analysis;
pub mod config;
pub mod db;
pub mod extract;
pub(crate) mod num;
pub mod rules;
pub mod sdk;
pub(crate) mod scan;
pub(crate) mod sexp;
pub mod version;
pub(crate) mod vir;
