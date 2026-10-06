//! verus-lint: facts from Verus's VIR log, stored in DuckDB, checked by SQL rules.

pub mod analysis;
pub mod config;
pub mod db;
pub mod extract;
pub mod rules;
pub mod scan;
pub mod sexp;
pub mod version;
pub mod vir;
