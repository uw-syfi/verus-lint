//! verus-lint: facts from Verus's VIR log, stored in `DuckDB`, checked by SQL rules.

pub mod analysis;
pub mod baseline;
pub mod cli;
pub mod config;
pub mod db;
pub mod engine;
pub mod extract;
pub(crate) mod num;
pub mod output;
pub mod rules;
pub(crate) mod scan;
pub mod sdk;
pub(crate) mod sexp;
pub mod version;
pub(crate) mod vir;

pub use cli::run;
