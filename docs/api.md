# Rust API reference

This page covers the parts of the `verus-lint` crate that rules and tools use:
the rule SDK (`verus_lint::sdk`), findings (`verus_lint::rules::Finding`), the
command-line entry point (`verus_lint::run`) and the configuration types. The
full item-by-item reference is the rustdoc (`cargo doc --open`, or docs.rs).

The Rust code blocks on this page are compiled by `cargo test --doc` (see
`src/lib.rs`), so the signatures and examples match the crate. Examples that
need facts take them as an argument (`facts: &Facts`); `Facts::load` shows how
to open a database.

Contents: [Overview](#overview) | [`run`](#run) | [`Rule`](#rule) | [`RuleMeta`](#rulemeta)
| [`Severity`](#severity) | [`Ratchet`](#ratchet) | [`Cx` and `Params`](#cx-and-params)
| [`Findings` and `Finding`](#findings-and-finding) | [`Facts`](#facts)
| [Row types](#row-types) | [`Graph`](#graph) | [Config types](#config-types)
| [Other public modules](#other-public-modules)

## Overview

A Rust rule is a type that implements `Rule`. Rules live in a small crate in
your own repository (by convention `lints/`) whose `main` calls `verus_lint::run`
with the rules. Point `[rules] rust = "lints"` at it in `verus-lint.toml`: then
`verus-lint check` and `verus-lint run` build the crate and start its binary
with the same arguments. The binary also loads your SQL rules, so SQL rules,
Rust rules, levels, baselines and output go through one report.

```toml
# lints/Cargo.toml
[package]
name = "my-lints"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
anyhow = "1"
verus-lint = "0.1.1"

[workspace]   # keeps the crate out of the Cargo workspace it sits in
```

A complete rules crate (`lints/src/main.rs`):

```rust,no_run
use verus_lint::rules::Finding;
use verus_lint::sdk::{Cx, Findings, Mode, Rule, RuleMeta};

struct BigProofs;

impl Rule for BigProofs {
    fn meta(&self) -> RuleMeta {
        RuleMeta::new("my/big-proofs", "Proof functions with long bodies.").param("max_lines", 200)
    }

    fn check(&self, cx: &Cx, out: &mut Findings) -> anyhow::Result<()> {
        let max = cx.params.get_u64("max_lines")?;
        for f in cx.facts.functions() {
            if f.mode == Mode::Proof && u64::from(f.body_lines) > max {
                out.push(Finding::at(f, format!("{} lines", f.body_lines)));
            }
        }
        Ok(())
    }
}

fn main() -> std::process::ExitCode {
    verus_lint::run(&[&BigProofs])
}
```

`examples/rust-rules/` in the repository is a similar crate with a graph rule
and a metric ratchet.

## `run`

```rust,ignore
pub fn run(native: &[&dyn Rule]) -> std::process::ExitCode
```

The whole command line: `extract`, `verify`, `check`, `query` and `run`, with
the same arguments as the `verus-lint` binary. `native` are your Rust rules;
the SQL rules named in the config are loaded alongside them. Pass `&[]` for none.
Exit status: 0 clean, 1 gated findings the baseline does not cover, 2 rule,
config or runtime error, 3 unsupported Verus version.

```rust,no_run
fn main() -> std::process::ExitCode {
    verus_lint::run(&[])
}
```

## `Rule`

```rust,ignore
pub trait Rule {
    fn meta(&self) -> RuleMeta;
    fn check(&self, cx: &Cx, out: &mut Findings) -> anyhow::Result<()>;
}
```

`meta` is static: id, severity, parameters, ratchet. `check` reads facts from
`cx` and pushes findings into `out`. An `Err` aborts the run with exit status 2.
A rule holds no state between runs; build one from fields only if they are
constants.

To unit-test a rule, build the `Cx` by hand (see the doctest on `Rule` in the
rustdoc, which loads the fixture log and calls `check` directly).

```rust
use verus_lint::rules::Finding;
use verus_lint::sdk::{Cx, Findings, Kind, Rule, RuleMeta};

/// Trait implementations in the workspace for a trait the workspace does not declare.
struct ForeignImpls;

impl Rule for ForeignImpls {
    fn meta(&self) -> RuleMeta {
        RuleMeta::new("my/foreign-impls", "Implementations of foreign traits.")
    }
    fn check(&self, cx: &Cx, out: &mut Findings) -> anyhow::Result<()> {
        for f in cx.facts.functions().iter().filter(|f| f.kind == Kind::ForeignTraitImpl) {
            out.push(Finding::at(f, "implements a trait from outside the workspace"));
        }
        Ok(())
    }
}
```

## `RuleMeta`

```rust,ignore
pub struct RuleMeta {
    pub id: String,                     // "namespace/name", unique
    pub summary: String,                // one line
    pub severity: Severity,             // default Warning
    pub needs_dynamic: bool,            // skipped with a note when no `verify` data exists
    pub ratchet: Ratchet,               // default None
    pub params: Vec<(String, String)>,  // name and default value
    pub schema: Option<String>,         // schema range written against (informational)
}

impl RuleMeta {
    pub fn new(id: &str, summary: &str) -> Self;
    pub const fn severity(self, s: Severity) -> Self;
    pub fn param(self, name: &str, default: impl ToString) -> Self;
    pub const fn ratchet(self, r: Ratchet) -> Self;
    pub const fn needs_dynamic(self) -> Self;
}
```

```rust
use verus_lint::sdk::{Ratchet, RuleMeta, Severity};

fn meta() -> RuleMeta {
    RuleMeta::new("my/slow-proofs", "Proofs that use a lot of solver resources.")
        .severity(Severity::Warning)
        .param("max_rlimit", 5_000_000)
        .ratchet(Ratchet::Metric { ratio: 1.1, abs: 100_000.0 })
        .needs_dynamic()
}
```

## `Severity`

```rust,ignore
pub enum Severity { Note, Warning, Error }   // ordered: Note < Warning < Error

impl Severity {
    pub fn parse(s: &str) -> anyhow::Result<Self>;   // "note", "warning", "error"
    pub const fn as_str(self) -> &'static str;
}
```

The severity is what output shows. Whether findings fail the run is the
config's level (`off`, `note`, `warn`, `gate`), not the severity. A finding can
override its rule's severity with `Finding::severity`.

```rust
use verus_lint::sdk::Severity;

fn demo() -> anyhow::Result<()> {
    assert_eq!(Severity::parse("error")?, Severity::Error);
    assert!(Severity::Error > Severity::Warning);
    Ok(())
}
```

## `Ratchet`

```rust,ignore
pub enum Ratchet {
    None,                               // every finding counts (default)
    Set,                                // covered when its entity is in the baseline
    Metric { ratio: f64, abs: f64 },    // covered when metric <= max(ratio * base, base + abs)
}

impl Ratchet {
    pub fn parse(s: &str) -> anyhow::Result<Self>;   // "set", "metric, ratio = 1.2, abs = 5"
    pub fn allowed(self, base: f64) -> f64;           // largest metric still covered
}
```

A ratchet only matters together with a baseline file and a `gate` level; see
[config.md](config.md#baseline). Metric ratchets need each finding to carry a
`metric` (`Finding::metric`).

```rust
use verus_lint::sdk::Ratchet;

fn demo() -> anyhow::Result<()> {
    let r = Ratchet::parse("metric, ratio = 1.2, abs = 5")?;
    assert!((r.allowed(100.0) - 120.0).abs() < 1e-9);
    assert!((r.allowed(10.0) - 15.0).abs() < 1e-9);
    Ok(())
}
```

## `Cx` and `Params`

```rust,ignore
pub struct Cx<'a> {
    pub facts: &'a Facts,
    pub params: &'a Params,
}

pub struct Params(pub BTreeMap<String, String>);

impl Params {
    pub fn get_str(&self, name: &str) -> anyhow::Result<&str>;
    pub fn get_u64(&self, name: &str) -> anyhow::Result<u64>;
    pub fn get_f64(&self, name: &str) -> anyhow::Result<f64>;
}
```

`Params` holds the rule's parameters after resolution: the defaults from
`RuleMeta::param`, overridden by `[rules.params."<id>"]`, overridden by
`--param name=value`. A getter fails when the rule never declared the
parameter, or the text does not parse as the requested type.

```rust
use verus_lint::sdk::Cx;

fn threshold(cx: &Cx) -> anyhow::Result<f64> {
    cx.params.get_f64("ratio")
}
```

## `Findings` and `Finding`

```rust,ignore
pub struct Findings(/* private */);

impl Findings {
    pub fn push(&mut self, f: Finding);
    pub const fn len(&self) -> usize;
    pub const fn is_empty(&self) -> bool;
}

// verus_lint::rules::Finding
pub struct Finding {
    pub rule: String,        // filled in by the runner
    pub severity: String,    // empty means the rule's severity; filled in by the runner
    pub entity: String,      // stable key; the baseline matches on it
    pub message: String,
    pub file: Option<String>,
    pub line: Option<i64>,
    pub metric: Option<f64>,
    pub props: BTreeMap<String, String>,   // extra properties in JSON and SARIF
}

impl Finding {
    pub fn new(entity: &str, message: impl Into<String>) -> Self;
    pub fn at(f: &Function, message: impl Into<String>) -> Self;   // entity = f.path, location = f's definition
    pub fn location(self, file: &str, line: u32) -> Self;
    pub const fn metric(self, m: f64) -> Self;
    pub fn severity(self, s: Severity) -> Self;
    pub fn prop(self, key: &str, value: impl ToString) -> Self;
}
```

The **entity** identifies a finding across runs. Use a function path or another
stable name, never a line number or message text: the baseline compares
entities, and SARIF uses them to track findings as code moves.

```rust
use verus_lint::rules::Finding;
use verus_lint::sdk::{Findings, Function, Severity};

fn report(out: &mut Findings, f: &Function, n: u32) {
    out.push(
        Finding::at(f, format!("{n} requires clauses"))
            .metric(f64::from(n))
            .severity(Severity::Note)
            .prop("n_requires", n),
    );
}
```

For something that is not a function (a file, a module), use `Finding::new` with
the stable name and add `location` if there is one:

```rust
use verus_lint::rules::Finding;

fn module_finding(module: &str, file: &str) -> Finding {
    Finding::new(module, "module has no public items").location(file, 1)
}
```

## `Facts`

```rust,ignore
pub struct Facts { /* private */ }

impl Facts {
    pub fn load(conn: duckdb::Connection) -> anyhow::Result<Self>;
    pub fn functions(&self) -> &[Function];                       // ordered by id
    pub fn function(&self, id: FnId) -> &Function;                // panics on a foreign id
    pub fn by_path(&self, path: &str) -> Option<&Function>;
    pub fn uses(&self) -> &[Use];
    pub fn uses_from(&self, f: FnId) -> impl Iterator<Item = &Use>;   // f is the caller
    pub fn uses_of(&self, f: FnId) -> impl Iterator<Item = &Use>;     // f is the callee
    pub fn roots(&self) -> &[FnId];                               // the `roots` view
    pub const fn live(&self) -> &FnSet;                           // reachable from the roots
    pub fn graph(&self, keep: impl Fn(&Use) -> bool) -> Graph;
    pub const fn connection(&self) -> &duckdb::Connection;
    pub fn query(&self, sql: &str) -> anyhow::Result<Vec<Vec<String>>>;
    pub fn has_dynamic(&self) -> bool;
}
```

`Facts` is the in-memory view of one database. `functions` and `uses` are typed
rows of the `functions` and `uses` tables ([schema.md](schema.md)); the typed
`Function` carries the columns most rules need. For any other table or column,
use `query`.

- `load` reads the typed tables, the roots and the live set from an open
  connection. It fails on a value its enums do not know (a database from a newer
  schema).
- `roots` are functions that are live by definition (exec functions, config
  patterns, and so on). `live` is everything reachable from them over the full
  dependency graph: uses of every kind, module-level `broadcast use` items, group
  membership and trait-method dispatch. `Graph::reachable` over `graph` sees only
  the use edges you keep, so it can say less than `live`.
- `graph(keep)` builds a function graph with an edge for each resolved use that
  `keep` accepts.
- `query` accepts `SELECT` or `WITH` statements only and returns every cell as
  text, header row first. A null is the empty string. For typed results use
  `connection()` with the `duckdb` crate directly (add `duckdb` to your rules
  crate with the same version as verus-lint).
- `has_dynamic` is true when `verus-lint verify` has loaded data.

```rust
use std::path::Path;
use verus_lint::db::Db;
use verus_lint::sdk::{Facts, UseKind};

/// Open the database `extract` wrote (outside a rule; inside a rule use `cx.facts`).
fn open(path: &Path) -> anyhow::Result<Facts> {
    Facts::load(Db::open_read_only(path)?.conn)
}

/// Names of the functions `path` calls directly.
fn callees(facts: &Facts, path: &str) -> Vec<String> {
    let Some(f) = facts.by_path(path) else { return Vec::new() };
    facts
        .uses_from(f.id)
        .filter(|u| u.kind == UseKind::Call)
        .filter_map(|u| u.callee)
        .map(|id| facts.function(id).name.clone())
        .collect()
}

/// Typed access for the common case, SQL for the rest.
fn open_specs_per_module(facts: &Facts) -> anyhow::Result<Vec<Vec<String>>> {
    facts.query("SELECT module, count(*) FROM open_spec GROUP BY module ORDER BY 2 DESC")
}
```

## Row types

```rust,ignore
pub struct FnId(pub i64);                  // functions.fn_id; stable only within one database
pub type FnSet = std::collections::BTreeSet<FnId>;

pub struct Function {
    pub id: FnId,
    pub path: String,        // VIR path, unique
    pub friendly: String,    // Rust-style path as Verus reports print it
    pub krate: String,       // functions.crate
    pub module: String,
    pub name: String,
    pub mode: Mode,
    pub kind: Kind,
    pub vis: String,         // "pub" or the restricting module
    pub opaque: bool,
    pub external_body: bool,
    pub has_body: bool,
    pub generated: bool,
    pub file: String,
    pub line: u32,
    pub body_lines: u32,
}

pub struct Use {
    pub caller: FnId,
    pub callee: Option<FnId>,   // None when outside the extracted crates
    pub callee_path: String,
    pub section: String,        // require, ensure, returns, decrease, decrease_by, body, hide
    pub kind: UseKind,
    pub in_trigger: bool,
    pub file: String,
    pub line: u32,
}

pub enum Mode { Exec, Proof, Spec }
pub enum Kind { Static, TraitDecl, TraitImpl, ForeignTraitImpl }
pub enum UseKind { Call, Reveal, BroadcastUse, FnValue, ResolvedImpl, Hide, Other }
```

These are the typed subset of the tables; column meanings are in
[schema.md](schema.md#functions). `Function` and `Use` leave out the rarer
columns (`end_line`, `n_requires`, `fuel`, `rlimit_attr`, and so on): read
those with `Facts::query`. Missing source locations read as an empty file and
line 0.

```rust
use verus_lint::sdk::{Cx, Kind, Mode, UseKind};

/// Spec functions revealed from somewhere and also called from a requires clause.
fn count(cx: &Cx) -> usize {
    cx.facts
        .functions()
        .iter()
        .filter(|f| f.mode == Mode::Spec && f.kind == Kind::Static && !f.opaque)
        .filter(|f| {
            cx.facts
                .uses_of(f.id)
                .any(|u| u.kind == UseKind::Call && u.section == "require")
        })
        .count()
}
```

## `Graph`

```rust,ignore
pub struct Graph { /* private */ }

impl Graph {
    pub fn reachable(&self, from: &[FnId]) -> FnSet;   // includes `from`
    pub fn sccs(&self) -> Vec<Vec<FnId>>;              // every function in exactly one component
    pub fn callers(&self, f: FnId) -> Vec<FnId>;       // functions with an edge to f
    pub fn callees(&self, f: FnId) -> Vec<FnId>;       // functions f has an edge to
}
```

Build one with `Facts::graph`, choosing which uses are edges. `sccs` returns
components sorted by id, each component sorted, ordered by smallest id; a
function outside any cycle is a component of one, so filter on `len() > 1` for
cycles.

```rust
use verus_lint::sdk::{FnId, Facts, UseKind};

/// Groups of functions that call each other, and the functions only they reach.
fn cycles(facts: &Facts) -> Vec<Vec<FnId>> {
    let calls = facts.graph(|u| u.kind == UseKind::Call);
    calls.sccs().into_iter().filter(|c| c.len() > 1).collect()
}

/// Everything a function depends on through calls and reveals, excluding ensures sections.
fn depends_on(facts: &Facts, f: FnId) -> usize {
    let g = facts.graph(|u| {
        matches!(u.kind, UseKind::Call | UseKind::Reveal) && u.section != "ensure"
    });
    g.reachable(&[f]).len() - 1
}
```

## Config types

`verus_lint::config` holds the parsed `verus-lint.toml`. A rules crate rarely
needs it (the CLI applies levels and parameters), but tools that read the same
file can use it. Keys, defaults and meanings are in [config.md](config.md).

```rust,ignore
pub struct Config {
    pub extract: ExtractCfg,    // toolchain, crates, exclude
    pub roots: RootsCfg,        // patterns, public_api, pins, pins_are_roots, name_files
    pub rules: RulesCfg,        // dirs, rust, rust_profile, levels, params
    pub baseline: BaselineCfg,  // file
}

impl Config {
    pub fn parse(text: &str) -> anyhow::Result<Self>;
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self>;
    pub fn level(&self, rule: &str) -> anyhow::Result<Level>;   // Warn unless configured
    pub fn rule_params(&self, rule: &str) -> BTreeMap<String, String>;
}

pub enum Level { Off, Note, Warn, Gate }

impl Level {
    pub fn parse(s: &str) -> anyhow::Result<Self>;
    pub const fn as_str(self) -> &'static str;
}

pub fn glob_match(pat: &str, text: &str) -> bool;   // `*` matches any run, including `::` and `/`
```

```rust
use verus_lint::config::{Config, Level};

fn gated(cfg: &Config, rule: &str) -> anyhow::Result<bool> {
    Ok(cfg.level(rule)? == Level::Gate)
}

fn demo() -> anyhow::Result<()> {
    let cfg = Config::parse("[rules.levels]\n\"my/big-proofs\" = \"gate\"\n")?;
    assert!(gated(&cfg, "my/big-proofs")?);
    assert!(!gated(&cfg, "other/rule")?);
    Ok(())
}
```

## Other public modules

These are public for tools that embed verus-lint; rules do not need them.

| Module | Contents |
| --- | --- |
| `verus_lint::db` | `Db` (open, create, `query_rows`), `SCHEMA_VERSION`. `Db::open_read_only(path)` is how to read a database. |
| `verus_lint::extract` | `extract`, `Options`, `load_crate_logs`: run Verus per crate and load logs. |
| `verus_lint::verify` | `run`, `ingest_file`: load verification reports. |
| `verus_lint::engine` | `evaluate`, `Outcome`, `Reported`: run rules and apply levels and the baseline. |
| `verus_lint::baseline` | `Baseline`: load, save and compare baseline files. |
| `verus_lint::output` | `text`, `json`, `sarif`: render an `Outcome`. |
| `verus_lint::rules` | `Finding`, and the SQL rule loader. |
| `verus_lint::version` | `SUPPORTED` (the Verus releases accepted), `check`. |
