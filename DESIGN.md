# verus-lint design

verus-lint is a lint and analysis tool for Verus codebases. It extracts facts
about functions and their uses from Verus's own intermediate representation
(VIR) and from Verus's verification reports, stores them in an embedded DuckDB
database, and runs rules written in SQL or in Rust against that database.
The tool is mechanism only: fact extraction, the database and its views, the
SQL runner, the Rust SDK, the CLI, configuration, baselines and output. It
contains no rules. Rules live in the user's repository; `examples/` holds
sample lints to copy, and nothing loads them unless the user points at them.
Coral (`llm-eq/coral/crates`) is the first user and writes its own rules in
its own tree; nothing in the tool is specific to Coral.

Evidence for the choices below is in `spike/notes.md` (Verus
0.2026.07.18.3a4d30b, measured on coral-spec and coral-effects).

## 1. Overview

```
verus-lint extract  ->  cargo verus build -p C -- --no-verify --log vir --log impl-names   (per crate)
                        cargo verus build -p C -- --time-expanded --output-json            (optional, dynamic)
                    ->  parse logs and reports  ->  facts.duckdb (one database per workspace commit)
verus-lint check    ->  build the user's rules crate (if configured)  ->  run the user's SQL rules and Rust rules
                    ->  apply config levels and the ratchet baseline  ->  text / JSON / SARIF
```

Terms used below:

- **Static facts**: what the source says, from the VIR log. Needs Verus's
  front end only (`--no-verify`).
- **Dynamic facts**: what a verification run measured (rlimit, time, success)
  per function and per module, from `--output-json`.
- **Use**: one reference from a function (the caller) to another function
  (the callee), with the place it occurs (requires, body, trigger, ...).
- **Root**: a function that is live by definition for dead-code analysis.

## 2. Fact schema

All tables are Verus-generic. Paths are VIR paths (`crate::module::impl&%N::f`);
`friendly` is the Rust-style name Verus prints in reports
(`crate::module::Type::f`).

| Table | Key | Columns |
| --- | --- | --- |
| `meta` | `key` | `value`: `schema_version`, `verus_version`, `verus_commit`, `rust_toolchain`, `source_commit`, `source_dirty`, `extracted_at`, `tool_version` |
| `crates` | `crate` | `manifest`, `input_hash`, `log_bytes`, `fn_count`, `verified` (bool: dynamic facts present) |
| `modules` | `module` | `crate`, `file` |
| `functions` | `fn_id` | `path`, `friendly`, `crate`, `module`, `name`, `self_type`, `trait_path`, `mode` (exec, proof, spec), `kind` (static, trait_decl, trait_impl, foreign_trait_impl), `item_kind` (function, const), `vis` (pub, or the restricting module), `body_vis` (open/closed: pub, module, or none for no body), `opaque` (bool), `reveal_vis`, `external_body`, `broadcast_forall`, `broadcast_forall_only`, `rlimit_attr` (nullable, `inf` allowed), `spinoff_prover`, `integer_ring`, `bit_vector`, `nonlinear`, `has_body`, `file`, `line`, `end_line`, `body_lines`, `n_requires`, `n_ensures`, `has_default` (trait method declaration with a default body), `trait_method` (for an implementation, the declaration it implements), `type_invariant`, `generated` (compiler-made `arrow_*` field accessor) |
| `uses` | none (multiset) | `caller_id`, `callee_path`, `callee_id` (null when the callee is outside the extracted crates), `section` (require, ensure, returns, decrease, decrease_by, body, hide, module), `kind` (call, reveal, broadcast_use, fn_value, resolved_impl, hide), `in_trigger` (bool), `fuel`, `file`, `line`, `col` |
| `module_uses` | none | `module`, `callee_path`, `callee_id`, `kind` (broadcast_use), `file`, `line` (module-level `broadcast use`, see section 4.3) |
| `quantifiers` | none | `fn_id`, `quant` (forall, exists, choose), `trigger` (explicit, auto_annotation, none), `n_triggers`, `section`, `file`, `line` |
| `trusted` | none | `fn_id` (nullable), `kind` (assume, admit, external_body, external_fn, external_type, assume_specification, broadcast_axiom), `file`, `line`, `text` |
| `trait_impls` | `impl_path` | `trait_path`, `self_type`, `crate`, `file`, `line` (from `--log impl-names`) |
| `verify_fn` | `run_id, fn_id` | `friendly`, `rlimit`, `time_us`, `success`, `seed` |
| `verify_module` | `run_id, module` | `rlimit`, `smt_time_ms`, `session_time_ms` |
| `broadcast_groups` | `path` | `crate` (groups defined by an extracted crate, from `(group_id ..)` forms) |
| `group_members` | none | `group_path`, `member_path`, `member_id` (null outside the extracted crates), `file`, `line` (source scan of `broadcast group` items) |
| `root_patterns` | none | `pattern`, `like_pattern`, `by_name` (from `[roots] patterns`) |
| `root_names` | none | `name`, `file` (function names found as identifier tokens in the `[roots] name_files` files; first file per name) |
| `api_pins` | none | `entry`, `file`, `line`, `fn_id` (null when the entry matched nothing) |
| `dead_scc` | `fn_id` | `scc_id`, `scc_size` (components of the dead subgraph) |
| `warnings` | none | `crate`, `what` (`feature_gated_item`, `unresolved_broadcast_use`, `module_use_in_function_less_file`, `scan_unreadable_file`, `ambiguous_broadcast_group`), `detail` (`file:line: text`); extraction health, read by `examples/rules/extraction-health.sql` |
| `runs` | `run_id` | `crate`, `seed`, `verus_args`, `source_commit`, `started_at`, `wall_s` |

`functions.end_line` comes from the body expression's span: the `Function`
form's own span covers only the header line. `open_spec` means a spec
function that is not opaque and whose body visibility is wider than its own
module (`pub open`, `open(crate)` and `open(in ancestor)` all record a body
visibility restricted to `None` or an ancestor module).

Derived views shipped with the schema: `roots`, `graph_edges` and `live_nodes` (section 6), `edges` (uses with resolved
`callee_id`, de-duplicated per caller, callee and section), `open_spec`
(spec functions with a public body and not opaque), `verify_latest` (latest
run per function at the default seed), `verify_worst` (maximum rlimit over
seeds).

Versioning: `meta.schema_version` is semver. A rule declares the range it was
written against (`schema: ^1.2`). Adding a table, column or enum value is a
minor bump; renaming, removing or changing the meaning of one is a major bump.
The SDK's typed accessors are generated from the schema, so a Rust rule that
compiles against SDK 1.x runs against any 1.y database with y at least x.

## 3. Verus version check

The VIR log is a pretty-print of Verus's internal AST: it has no stability
promise and no version header. Compatibility is therefore exact, not
best-effort:

1. The extractor holds a table of supported Verus releases (version string
   and full commit), each mapped to a parser profile (field names and
   constructor spellings for that release). Version 1 supports
   0.2026.07.18.3a4d30b.
2. Before extraction it runs `verus --version --output-json` through the
   configured toolchain command and refuses (exit status 3, listing the
   supported versions) if the commit is not in the table.
3. Each `--output-json` report carries the same `verus` object; dynamic
   facts from a report with a different commit are refused.
4. A log handed in directly (`extract --from-log`) must come with the
   sidecar `version.json` the extractor writes next to logs it produces.
5. The parser fails closed: an unknown constructor or field in a position it
   interprets (function header, call target, `Fuel`, quantifier, trigger,
   assume) is an error naming the form and span, not a skipped item.

Adding a Verus release means adding a table entry plus a fixture: a small
Verus crate in `tests/fixtures/` that exercises every fact kind, whose logged
output is parsed and compared against expected tables. CI runs the fixture
against every supported release.

## 4. Extraction pipeline

### 4.1 Static facts

For each workspace member with `[package.metadata.verus] verify = true`, in
dependency order, the extractor runs

```
<toolchain> cargo verus build -p <crate> --fwd-verus-args-to roots \
  -- --no-verify --log vir --log impl-names --log-dir <cache>/<crate>/log
```

One invocation per crate is required: Verus names the log `crate.vir`
regardless of crate and deletes the log directory first. Dependencies are
built once and cached by cargo. `--no-verify` skips SMT; on coral-spec this
is 41.5 s against 64 s for a verify, with identical facts.

Cargo treats a root crate as fresh when only the forwarded Verus arguments
changed, and a fresh crate writes no log (seen on Coral). The extractor runs
`cargo clean -p <crate>` immediately before each run (so the next normal build
of that crate recompiles it) and fails if no `crate.vir` appears. `--target-dir`
isolates extraction builds from the user's own target directory at the price of
a cold dependency build.

The `<toolchain>` command is configurable so that a container wrapper such as
Coral's `./verify` works unchanged.

The parser streams top-level forms (one function at a time, no whole-file
tree), keeps only the crate's own items (imported items in the log are pruned
copies), and writes rows through DuckDB's appender. The spike prototype
parses 148 MB in about 3 s on one thread; forms are independent, so the
production parser splits the file at top-level blank lines and parses on all
cores.

### 4.2 Dynamic facts

`verus-lint verify [--seeds 1,2,3]` runs the same per-crate command with
`--time-expanded --output-json` (and `-V smt.random_seed=N` per seed), or
`extract --report FILE` ingests an existing report. Rows join to `functions`
by the friendly name: direct match, then trait impls through `trait_impls`,
then inherent impls through the type of `self`, then the return type. The
spike leaves 2 of 98 coral-effects functions unjoined (associated `new`
functions); unjoined rows are kept with a null `fn_id` and counted in
`meta.unjoined_verify_rows` so rules can refuse to trust a low join rate.

### 4.3 Gaps filled outside the log

- Module-level `broadcast use` is not printed in the log (Verus's printer
  writes module ids only). Phase 1 recovers it with a source scan of the
  files that hold the crate's functions (`scan.rs`): every `broadcast use`
  item outside comments and strings, minus those inside a known function span
  (function-local ones are in the log), with brace and comma lists expanded.
  Names resolve by exact match against the crate's functions and groups
  (as written, `crate::` expanded, or relative to the module and its
  ancestors); names outside the crate (`vstd::`, `core::`) are stored as
  written with a null `callee_id`; any other unresolved name is a row in
  `warnings`. Measured on Coral (12 crates): 14 module-level lines give 18
  `module_uses` rows, matching a grep of the source (18 `broadcast use`
  lines, 4 of them function-local); no warnings. The scan is dropped for a
  Verus version whose printer emits module reveals (see Future work).
- Broadcast groups (settled in phase 1). A group appears in the log only as a
  `(group_id path)` form (Coral's own crates define none; vstd's appear in
  the imported part). It carries no member list. A function-local
  `broadcast use group_x` is a `Fuel (Fun :path group_x) 1 true` node, so it
  is a `broadcast_use` row in `uses` whose callee is the group path
  (`callee_id` null: groups are not functions). `broadcast proof fn` is a
  function with `:broadcast_forall true` in its attributes (0 in Coral, many
  in vstd). Consequence for phase 2 and 3: group membership needs the same
  source scan (`broadcast group name { a, b }`), and reachability must treat a
  group as a node whose members become live with it; `broadcast_groups`
  holds the group paths now, a `group_members` table is added with the scan.

- What the log omits, and what the scan reports. The log is one build: items
  under `cfg(feature = ..)` that the build did not enable and `#[cfg(test)]`
  items are absent, and so are their references. Tests need no handling (a
  function used only by tests is dead for a proof lint). For features, the
  source scan covers every `.rs` file under each crate's `src/` (not only files
  that hold functions) and records each `#[cfg(.. feature ..)]` attribute line
  as a `feature_gated_item` warning, so a count can be read next to the list of
  code it cannot see. On Coral: 379 such notes, mostly `neg_*` negative
  controls. The scan also finds `broadcast group` items in files without
  functions; a module-level `broadcast use` in such a file has no known module
  and is recorded as `module_use_in_function_less_file`.

Linking Verus's `vir` crate to read the bincode export cargo-verus already
writes is not needed: that export drops proof and exec bodies, so it cannot
give uses, and it would tie the binary to one Verus commit at compile time.

### 4.4 Multi-crate join

Each crate's log is authoritative for that crate's functions. After all crates
are loaded, `uses.callee_id` is resolved by exact VIR path across all
crates; uses of vstd or of crates outside the workspace keep `callee_id` null
and their `callee_path`. Cross-crate uses therefore need no extra step.

### 4.5 Caching

The cache lives in `.verus-lint/cache/`. A crate's static facts are keyed by
`input_hash`: the git tree id of the crate's directory, the input hashes of
its workspace dependencies, `Cargo.lock`, the Verus commit and the parser
version. A crate whose key is unchanged is not re-run; its facts are stored as
Parquet and loaded into the database by `COPY`. A dirty working tree hashes
the changed files with `git hash-object`. Dynamic facts are keyed the same
way plus seed and Verus arguments. `facts.duckdb` is rebuilt from the
per-crate Parquet files in under a second.

## 5. SQL rules

The tool ships no rules. The examples in this section and the files under
`examples/rules/` show the format and the available mechanisms; a user
copies what they need into their own repository and lists the directory under
`[rules] dirs`.

A SQL rule is one `.sql` file with a comment header and one `SELECT`.

```sql
-- id: verus/fanin-open-spec
-- summary: Open spec functions named by many functions outside their module.
-- severity: warning
-- schema: ^1.0
-- params: min_fns = 20
-- ratchet: metric, ratio = 1.2, abs = 5
```

Header keys: `id` (required, unique), `summary` (required), `severity`
(note, warning, error; the default level, overridable in config), `schema`
(semver range), `params` (defaults, overridable per rule in config, read in
SQL as `param('name')`), `needs` (`dynamic` if the rule reads `verify_*`; the
rule is skipped with a note when no dynamic facts exist), `ratchet` (see
section 7).

Result columns:

| Column | Required | Meaning |
| --- | --- | --- |
| `entity` | yes | Stable key of the finding (usually a function path); used by the baseline, so never a line number |
| `message` | yes | One line |
| `file`, `line` | no | Location for text and SARIF output |
| `metric` | no | Number compared by metric ratchets |
| `severity` | no | Per-row override |
| any other | no | Kept as properties in JSON and SARIF |

Example 1, definition fan-in (what `coral/tools/fanin.py` computes
syntactically, here from resolved paths):

```sql
-- id: verus/fanin-open-spec
-- summary: Open spec functions named by many functions outside their module.
-- severity: note
-- params: min_fns = 20
-- ratchet: metric, ratio = 1.2, abs = 5
SELECT d.friendly                            AS entity,
       d.file, d.line,
       count(DISTINCT c.fn_id)               AS metric,
       count(DISTINCT c.module)              AS mods,
       count(DISTINCT c.crate)               AS crates,
       d.body_lines                          AS body,
       format('{} functions in {} modules name this open definition',
              count(DISTINCT c.fn_id), count(DISTINCT c.module)) AS message
FROM open_spec d
JOIN edges u     ON u.callee_id = d.fn_id AND u.kind = 'call'
JOIN functions c ON c.fn_id = u.caller_id
WHERE c.module <> d.module
GROUP BY ALL
HAVING count(DISTINCT c.fn_id) >= param('min_fns')
ORDER BY metric DESC;
```

Example 2, dead proof code as a recursive CTE. `roots` is a view the core
builds from config patterns plus the implicit roots of section 6.

```sql
-- id: verus/dead-proof-code
-- summary: Proof and spec functions unreachable from any root.
-- severity: warning
-- ratchet: set
WITH RECURSIVE live(fn_id) AS (
    SELECT fn_id FROM roots
  UNION
    SELECT e.callee_id
    FROM live l JOIN edges e ON e.caller_id = l.fn_id
    WHERE e.callee_id IS NOT NULL
  UNION
    -- a live module's broadcast use keeps the lemma or group live
    SELECT m.callee_id FROM module_uses m
    WHERE m.callee_id IS NOT NULL
      AND m.module IN (SELECT f.module FROM live JOIN functions f USING (fn_id))
  UNION
    -- a live trait method keeps every implementation live
    SELECT i.fn_id FROM live l
    JOIN functions d ON d.fn_id = l.fn_id AND d.kind = 'trait_decl'
    JOIN functions i ON i.kind = 'trait_impl' AND i.trait_path = d.trait_path AND i.name = d.name
)
SELECT f.path AS entity, f.file, f.line,
       format('{} fn {} is unreachable from every root', f.mode, f.friendly) AS message
FROM functions f
WHERE f.mode IN ('proof', 'spec') AND f.fn_id NOT IN (SELECT fn_id FROM live)
ORDER BY f.file, f.line;
```

Reachability from roots is the fixpoint of deleting callerless functions, and
it also catches dead cycles (mutually recursive lemmas no root reaches). The
Rust version of this rule (section 6) additionally groups dead functions by
strongly connected component so a dead cycle is reported once.

## 6. Rust SDK

The user writes a small crate, by convention `lints/`:

```toml
# lints/Cargo.toml
[package]
name = "my-lints"
edition = "2021"
[dependencies]
verus-lint = "1"
```

```rust
// lints/src/main.rs
fn main() -> std::process::ExitCode {
    verus_lint::run(&[&NoBigOpaqueReveal])
}
```

`verus_lint::run` is the complete command line (`extract`, `check`, `query`,
`run`). When `[rules] rust` names a crate, `check` and `run` build it
(`cargo build --manifest-path <crate>/Cargo.toml`, release profile unless
`rust_profile` says otherwise, `CARGO_*` package variables removed from the
child's environment) and start its binary with the same arguments and
`VERUS_LINT_DELEGATED=1`, which stops the child from delegating again. The
child loads the SQL rule directories from the config, adds its Rust rules, and
produces the one report, so levels, baselines and output are identical for both
kinds of rule. Without a `rust` entry the `verus-lint` binary is
`verus_lint::run(&[])`. No dynamic libraries are loaded.

API (module `verus_lint::sdk`; `Finding` is `verus_lint::rules::Finding`):

```rust
pub trait Rule {
    fn meta(&self) -> RuleMeta;        // id, summary, severity, needs_dynamic, ratchet, params, schema
    fn check(&self, cx: &Cx, out: &mut Findings) -> anyhow::Result<()>;
}
pub struct Cx<'a> { pub facts: &'a Facts, pub params: &'a Params }   // params: get_str, get_u64, get_f64

impl Facts {
    pub fn functions(&self) -> &[Function];            // typed rows of `functions` (main columns)
    pub fn function(&self, id: FnId) -> &Function;
    pub fn by_path(&self, path: &str) -> Option<&Function>;
    pub fn uses(&self) -> &[Use];
    pub fn uses_from(&self, f: FnId) -> impl Iterator<Item = &Use>;
    pub fn uses_of(&self, f: FnId) -> impl Iterator<Item = &Use>;
    pub fn roots(&self) -> &[FnId];                    // the `roots` view
    pub fn live(&self) -> &FnSet;                      // the `live_nodes` view, as functions
    pub fn graph(&self, keep: impl Fn(&Use) -> bool) -> Graph;   // resolved uses only
    pub fn query(&self, sql: &str) -> anyhow::Result<Vec<Vec<String>>>;  // SELECT or WITH only
    pub fn connection(&self) -> &duckdb::Connection;
    pub fn has_dynamic(&self) -> bool;
}
impl Graph {
    pub fn reachable(&self, from: &[FnId]) -> FnSet;
    pub fn sccs(&self) -> Vec<Vec<FnId>>;              // every function in exactly one component
    pub fn callers(&self, f: FnId) -> Vec<FnId>;
    pub fn callees(&self, f: FnId) -> Vec<FnId>;
}
impl Finding {   // builders; the runner fills in rule id and default severity
    pub fn new(entity: &str, message: impl Into<String>) -> Self;
    pub fn at(f: &Function, message: impl Into<String>) -> Self;  // entity = path, location = definition
    pub fn location(self, file: &str, line: u32) -> Self;
    pub fn metric(self, m: f64) -> Self;
    pub fn severity(self, s: Severity) -> Self;
    pub fn prop(self, key: &str, value: impl ToString) -> Self;
}
```

`Facts::graph` has an edge per resolved use. It does not include the module,
group and trait-dispatch edges of `graph_edges`; `Facts::live` does, so a Rust
dead-code rule starts from `live()` and uses `sccs()` to group the dead
functions (on Coral that rule and the SQL one agree on all 774). The typed
enums fail closed on a value the SDK does not know (`UseKind::Other` exists
because real logs have path mentions that are neither calls nor reveals).
`Facts::verify` (dynamic facts) arrives with phase 5.

Example: opaque definitions revealed in many modules (closing a definition
only helps if few places reveal it).

```rust
struct NoBigOpaqueReveal;

impl Rule for NoBigOpaqueReveal {
    fn meta(&self) -> RuleMeta {
        RuleMeta::new("my/opaque-reveal-spread", "Opaque definition revealed in many modules")
            .severity(Severity::Warning)
            .param("max_modules", 8)
    }
    fn check(&self, cx: &Cx, out: &mut Findings) -> anyhow::Result<()> {
        let max = cx.params.get_u64("max_modules")?;
        for d in cx.facts.functions().iter().filter(|f| f.mode == Mode::Spec && f.opaque) {
            let mods: BTreeSet<_> = cx.facts.uses_of(d.id).iter()
                .filter(|u| u.kind == UseKind::Reveal)
                .map(|u| &cx.facts.function(u.caller).module)
                .collect();
            if mods.len() as u64 > max {
                out.push(Finding::at(d, format!("revealed in {} modules", mods.len()))
                    .metric(mods.len() as f64));
            }
        }
        Ok(())
    }
}
```

Default roots (the `roots` view, and `Facts::roots` once the SDK exists):
exec functions; functions matched by config root patterns (top theorems and
negative controls; a pattern without `::` matches the bare name); functions
named in `name_files` files (tooling outside Rust); trait impl
methods of traits declared outside the extracted crates (callers may dispatch
to them generically); type invariants; when `roots.public_api` is on (for
libraries with users outside the workspace), every `pub` function. API pin
entries are not roots: a pinned function that nothing reaches is reported by
`verus/unused-public-api` instead of `verus/dead-proof-code`. Trait methods
need no root: the `graph_edges` view leads from a trait method declaration to
its implementations, from a group to its members, and from a function to its
module's `broadcast use` items. Uses through triggers, reveals, `hide`, and
cross-crate calls are ordinary edges. `#[test]` functions and anything under
`cfg(feature = ..)` that the extracted build does not enable are not in the
log, so references from them are invisible; list their callees as patterns.
Compiler-generated `arrow_*` field accessors are never reported.

## 7. Configuration

`verus-lint.toml` at the workspace root:

```toml
[extract]
toolchain = ["./coral/verify"]          # command prefix for cargo verus; default none
crates = ["coral/crates/**"]            # members to extract (package names or manifest directory globs); default all verified members
exclude = ["sea-lion-cuda-sys"]        # verify = true members Verus cannot build

[roots]
patterns = ["theorem_*", "neg_*", "fixture_*"]   # bare names; with `::` the pattern matches the path
name_files = ["tools/*.py", "!tools/baseline*"]  # every function named by an identifier token in these files
                                        # is a root (globs; `*` crosses `/`; a leading `!` excludes)
pins_are_roots = false                  # true: pinned functions are roots too
public_api = false
pins = ["tools/pins/*.pin"]             # API pin files, one function path or friendly name per line;
                                        # not roots: listed items are reported as "unused public API"

[rules]
dirs = ["lints/sql"]                    # SQL rule directories (the tool has no built-in rules)
rust = "lints"                          # Rust rule crate; omitted if absent
rust_profile = "release"                # cargo profile for that crate (default release)

[rules.levels]                          # off, note, warn, gate
"verus/dead-proof-code" = "gate"
"verus/fanin-open-spec" = "warn"

[rules.params."verus/rlimit-headroom"]
budget = 10_000_000
warn = 0.5
fail = 0.8

[baseline]
file = "verus-lint-baseline.json"
```

Levels (default `warn`): `off` skips the rule; `note` prints its findings as
notes; `warn` prints them at the rule's severity; none of these change the exit
status. `gate` findings that the baseline does not cover make `check` exit 1.
Parameter values in `[rules.params]` must be numbers (SQL rules substitute them
as literals); the order of precedence is rule default, config, `--param`.

Baseline (ratchet): a JSON file the tool writes with `check --update-baseline`
and users commit.

```json
{
  "schema": 1,
  "verus_commit": "3a4d30bcdc4571e7927af97be9c4664973083eda",
  "rules": {
    "verus/dead-proof-code": { "set": ["coral_spec::x::lemma_old", "..."] },
    "verus/rlimit-function": { "metric": { "coral_spec::rewrite::controls::lemma_qkv_inserted_causal": 147988 } }
  }
}
```

A rule with `ratchet: set` fails on entities not in its set; a rule with
`ratchet: metric, ratio = r, abs = a` fails when an entity's metric exceeds
`max(r * base, base + a)`, or the entity is not in the baseline (the rule's own
threshold already decided that it is a finding). A baselined finding is still
reported in JSON and SARIF (`baselined: true`, SARIF `baselineState`) and is
left out of the text listing. Entities in the baseline that no longer appear are
reported as fixed, so the baseline only shrinks unless updated on purpose.
`--update-baseline` rewrites the entries of the rules that ran (and only those
with a ratchet header), keeps the others, and exits 0.

## 8. Output

- Text (default): one line per finding not covered by the baseline,
  `file:line: severity rule-id: message [entity]`, at most `--top N` per rule (default
  20, 0 for all), then a summary line per rule (new, baselined, fixed) and the
  gate verdict.
- JSON: `{ meta, rules: [{id, level, severity, summary, findings, new, baselined, fixed}], findings: [{rule, level, severity, entity, message, file, line, metric, properties, baselined}], skipped, gate_failures }`, fixed field order, for scripts.
- SARIF 2.1.0: one `run`, rules as `reportingDescriptor`s with their summary,
  findings as `result`s with `partialFingerprints.entity` set to the entity so
  code-scanning UIs track findings across line moves.

Exit status: 0 clean, 1 gate findings, 2 rule or config error, 3 unsupported
Verus version. `--format` and `--output FILE` select the rendering; `--baseline FILE`
overrides the baseline path.

## 9. Performance targets

Measured base: coral-spec (82k lines) gives a 148 MB log, parsed in 3 s on
one thread; `--no-verify` extraction takes about 42 s of Verus time.

| Step | Target (all of Coral, about 250k lines in 12 verified crates) |
| --- | --- |
| Parse and load all logs, cold cache | under 10 s on 8 cores, under 2 GB memory |
| Load from Parquet cache, nothing changed | under 1 s |
| The example SQL and Rust rules | under 5 s total; any single rule under 1 s |
| `check` after a one-crate change | Verus `--no-verify` time of that crate and its dependents plus 5 s |
| Logging overhead in a verify run | under 5% of Verus time (measured 1.8%) |

## 10. Example lints

These are sample rules, kept under `examples/` (SQL files now, an example Rust
rules crate with the SDK in phase 4). They are documentation and test input,
not part of the tool. The mechanisms they rely on are in the tool: the
`roots`, `graph_edges` and `live_nodes` views, `dead_scc`, `[roots]`
patterns and pins, the baseline ratchets.

| Rule id | What it reports |
| --- | --- |
| `verus/dead-proof-code` | Proof and spec functions unreachable from roots, dead cycles grouped by SCC |
| `verus/unused-public-api` | Pinned public proof and spec functions that no root reaches (the category dead-proof-code leaves out) |
| `verus/dead-exec-ghost` | Exec functions only reachable from dead functions (off by default) |
| `verus/fanin-open-spec` | Open spec definitions by fan-in outside their module (functions, modules, crates) |
| `verus/fanin-reveal` | Closed or opaque definitions by reveal sites outside their module |
| `verus/rlimit-headroom` | Functions over a fraction of the rlimit budget (warn and fail fractions) |
| `verus/rlimit-function` | Per-function rlimit ratchet against the baseline (metric ratchet) |
| `verus/rlimit-module` | Per-module rlimit sum and session time against budgets |
| `verus/seed-instability` | Functions whose worst-seed rlimit exceeds the default-seed rlimit by a ratio, or which fail under some seed |
| `verus/hotspot-growth` | Modules whose total rlimit grew by a fraction and an absolute amount since the baseline |
| `verus/spinoff-candidate` | Heavy functions in a heavy module that lack `spinoff_prover` |
| `verus/quantifier-auto-trigger` | Functions with quantifiers that have no explicit trigger (`#[trigger]` or `#![trigger]`; `#![auto]` counts as explicit); a warning |
| `verus/quantifier-auto-trigger-note` | Verus's own "automatically chose triggers" notes, from the build log; informational, separate from the rule above |
| `verus/trait-spec-default` | Trait spec functions with a default body that implementations may override, and the implementations that do |
| `verus/trusted-inventory` | Every `assume`, `admit`, `external_body`, `external_fn`, `assume_specification` and broadcast axiom, with a count per crate (a set ratchet keeps the trusted surface from growing silently) |
| `verus/extraction-health` | Unjoined verification rows, unresolved `broadcast use` names, crates without logs |

## 11. Coral's Python tools as Coral-owned rules

Coral rewrites these as rules in its own tree (`coral/lints/`), written against
the schema; they may start from the examples but never reference them. Tool
mechanisms they need (roots, SCCs, baselines) are in verus-lint.

| Tool | Becomes | Notes |
| --- | --- | --- |
| `tools/fanin.py` (syntactic mode) | Coral rules like `examples/rules/fanin-*.sql` | Exact paths replace import-evidence matching, so the `amb` column and the method and trait-impl blind spots go away. `--compare` becomes the metric ratchet. `--detail NAME` becomes `verus-lint query fanin --entity NAME`. The semantic mode (close a definition, re-verify) stays a separate experiment driver; the rule's count is the input to it. |
| `tools/provenance_sites.py` | A Coral SQL rule in `lints/sql/` plus a small table of the old-form names and the path to phase buckets | Counts uses (and source lines) naming each listed fact, bucketed by caller path. Uses replace grep, so comments and strings no longer need special handling. Its phase table is Coral data, not core. |
| `tools/budget.py` | `verus/rlimit-headroom`, `verus/rlimit-function`, `verus/rlimit-module`, `verus/seed-instability`, `verus/hotspot-growth`, `verus/spinoff-candidate` | `baseline.json` and `rlimit_exceptions.txt` become the baseline file and per-entity param overrides. `--run` becomes `verus-lint verify`. |
| `tools/dead_fns.py` (also present) | A Coral rule like `examples/rules/dead-proof-code.sql` | Its `ROOTS` list moves to `[roots]` in Coral's config; name merging across modules disappears. |

## 12. Risks and open questions

Risks:

- The log format changes with any Verus AST change. Mitigation: exact
  version table, fail-closed parser, one fixture crate per release. Cost: a
  few hours per supported Verus release.
- Module-level `broadcast use` comes from a source scan until Verus prints
  module reveals; a scan can mis-resolve renamed imports.
- The JSON join is by friendly name; inherent associated functions without
  `self` may stay unjoined (2 of 98 in coral-effects).
- Per-crate Verus invocations lose cross-crate build parallelism on a cold
  run; a driver wrapper that sets a per-crate `--log-dir` would restore it
  but depends on cargo-verus internals.
- Log size: a full Coral extraction is likely 400 to 600 MB of logs; they
  are deleted after parsing and only Parquet is cached.

Open questions:

1. (Resolved, see Decided and Future work.)
2. (Resolved, see Decided.)
3. (Resolved in phase 1, see section 4.3: the log has group ids only; members come from a source scan.)

Decided (2026-10-06):

- Trigger rule (phase 2): flag quantifiers with no explicit trigger as
  warnings. Verus's auto-trigger notes are a separate informational rule, so
  question 2 resolves to "no explicit trigger is enough for the warning".
- Dead-code roots (phase 3): configurable. The default set is exec functions,
  functions named in config as top theorems, tests, and negative controls.
  Items listed in API pin files are not roots; they are reported in a separate
  "unused public API" category. (This replaces the earlier `roots.files`
  default of pins as roots in section 7 and question 4.)
- No upstream Verus changes for now; the two proposals are under Future work.
- License: MIT; the repository stays private for now.

## 13. Future work

Two small upstream Verus changes would remove workarounds. Neither is
proposed yet.

- Print module-level `broadcast use`: `write_krate` in `vir/src/printer.rs`
  writes modules as ids and drops `ModuleX::reveals`. Printing it would drop
  the source scan of section 4.3.
- Per-crate log names: name the log `<crate>.vir` (or keep the log directory)
  so one build of a workspace yields every crate's log, which removes the
  per-crate invocation and the `cargo clean` step of section 4.1.

## 14. Build plan

Each phase ends with a commit that builds, passes its tests, and is pushed.
Estimates are agent hours.

| Phase | Content | Hours |
| --- | --- | --- |
| 1 | Workspace (`verus-lint` core, CLI, SDK in one crate to start); version table and check; streaming parser to `functions` and `uses`; DuckDB load; `extract` driving `./coral/verify` per crate; SQL runner with header parsing; example lints `fanin-open-spec` and `fanin-reveal`. Ends with a run on Coral (llm-eq, read-only) whose top fan-in table is compared to `fanin.py --top 20`, with every difference explained. | 1.5 |
| 2 | Remaining static facts: `quantifiers`, `trusted`, `trait_impls`, module `broadcast use` scan; fixture crate and parser fixture tests; example lints `quantifier-auto-trigger`, `trusted-inventory`, `trait-spec-default`. | 1.5 |
| 3 | Mechanisms: roots config, `roots`, `graph_edges` and `live_nodes` views, SCCs (`dead_scc`, later an SDK `Graph::sccs`); example lint `dead-proof-code` in SQL; compare against `dead_fns.py` on Coral. | 1.0 |
| 4 | Rust SDK surface (`Facts`, `Graph`, `Rule`, `Findings`, `run`), `lints/` crate build and execution by the CLI, one example user rules crate under `examples/`. | 1.5 |
| 5 | Dynamic facts: `verify` command, report ingestion, friendly-name join, seeds; example lints for rlimit, seed instability, hotspot growth and spinoff candidates; compare with `budget.py` on Coral. | 1.5 |
| 6 | Config levels, baseline file, set and metric ratchets, `--update-baseline`; text, JSON and SARIF output; exit statuses. | 1.5 |
| 7 | Caching by input hash with Parquet, parallel parse, performance targets measured on Coral; extraction-health facts (unjoined rows, unresolved names) as a view. | 1.0 |
| 8 | Second codebase (a public Verus project, for example a vstd-only example crate set or a published Verus verification project) to check genericity; docs; Coral's own `provenance_sites` rule, in Coral's tree, as a worked project rule. | 1.0 |

Total: about 10.5 agent hours.

Phase 1 status (2026-10-06): done. `verus-lint run` extracts and checks a
workspace; `verus-lint query` runs ad hoc SQL. Differences from the plan above:
the SDK is not started (phase 4); the `check` command takes `--param` and
`--rules DIR` instead of a config file (phase 6); each root crate is cleaned
before its run (section 4.1); the module-level `broadcast use` scan is already
in. Coral dogfood (12 crates at `claude/coral-prov-flip`): 4 m 47 s of Verus
time for 15 verified members including cold dependency verification, 566 MB of
logs, 10,434 own functions and 214,003 uses, parse and load 7 to 9 s on one
thread, 8.9 MB database. The top fan-in table agrees with `fanin.py`
(syntactic): 14 of its top 15 are in our top 15, the other (`MemManager::len`)
is rank 16 with 124 against 217 functions. Differences:
`fanin.py` matches method calls by name and import evidence, so it
over-counts generic method names (`len`, and `nreq`/`inv` shared by `Engine`
and `MemManager`, its `amb` column) while resolved paths do not; free
functions differ by 1 to 2 per file in both directions (`ctx_at` 302 against
319) because it parses source text and the log has one entry per compiled
function; trait-impl methods such as `ExecModelGraph::view` (177 functions)
appear in our table and not in `fanin.py`, which skips them; the log has no
`#[cfg(test)]` items. No semantic-mode results are recorded in Coral's
`findings`, so the comparison is against syntactic mode only.

Phase 2 status (2026-10-06): done. New facts (schema 1.1.0): `quantifiers`,
`trusted`, `trait_impls`, `group_members`, and `functions.has_default`,
`trait_method`, `type_invariant`. `--exclude` and `verus-lint.toml` (`[extract]`
and `[roots]`; other sections are ignored until phase 6) are in. The parser is
tested against a real Verus log of `tests/fixtures/crate` (regenerate with
`tests/fixtures/regen.sh`, needs Docker and the oracle image). Findings from
the log: `#![auto]` is `Unary Trigger(AutoTrigger)` around the body, an explicit
trigger is `WithTriggers` with a nonempty `:triggers` or `Unary Trigger(Trigger
g)` on a subterm; `assume(false)` is how `admit()` appears; a function with a
non-null `:proxy` is an `assume_specification`; `AssertAssumeUserDefinedTypeInvariant`
is compiler-inserted and not a trusted item; `external_fn` and `external_type`
ids are mostly vstd's (own-crate ones only are stored); `#[verifier::external]`
items have no body in the log. A trait method implementation names its
declaration in `:method`, so dispatch needs no name matching. On Coral:
224 functions with untriggered quantifiers, 712 trusted rows, 2 trait spec
functions with defaults. CI had been failing since phase 1 because `*.vir` in
`.gitignore` hid `mini.vir`; fixed.

Phase 3 status (2026-10-06): done. `roots`, `graph_edges` and `live_nodes`
views, `dead_scc` (Tarjan in Rust, written by `extract`), `verus/dead-proof-code`
and `verus/unused-public-api`. The recursive query runs over path nodes
(functions, groups, `mod:` module nodes) and takes 0.3 s on Coral's 214k uses.
Coral dogfood (12 crates, `[roots]` mirroring `dead_fns.py`'s name patterns and
`tools/pins/*.pin`): 876 dead proof and spec functions (14,537 body lines), 229
unused public API items, no dead cycles. `dead_fns.py` reports 500 in the
same crates: all 500 are in our 876, none is missing. Of our 361 extra: 113
have a name that appears under `tools/` (`dead_fns.py` roots every such name;
we need a root-names file for that, see phase 4 and 6), 167 are
referenced only from `use` re-exports or from other dead functions (the Python
script counts an import line as a use), and 81 are referenced from code the
build does not compile (`cfg(feature = "neg_*")` negative controls, tests,
crates outside the extraction) or share a name with an unrelated item (it
merges by name; for example `numel` and `in_range` are also used elsewhere). `dead_fns.py` pinned names are live; ours are reported in the
separate API category (229). `arrow_*` field accessors were 662 of the first
run's 1,437 findings and are now excluded.

Design change (2026-10-06): the tool contains no rules. The `verus/...` rules
written in phases 1 to 3 moved from `rules/` to `examples/rules/`; `check`
loads only directories given by `--rules` or `[rules] dirs`, and prints a hint
when there are none. The Rust types behind `rules::examples()` are used by tests
only. `roots`, `graph_edges`, `live_nodes` and `dead_scc` stay in the tool as
mechanisms so a user can write the dead-code rule themselves.

Phase 3 follow-up (2026-10-06): the three precision gaps are closed.
`[roots] name_files` roots every function whose name is an identifier token in
a listed file (`root_names`, schema 1.2.0); feature-gated code is documented in
section 4.3 and reported as `feature_gated_item` warnings; the source scan now
covers files with no functions. Coral re-run (12 crates at
`claude/coral-prov-flip` 149841ecd, `[roots]` mirroring `dead_fns.py`: name
patterns, `tools/pins`, and `name_files` limited to the `.py`, `.toml`, `.json`
and `.txt` files under `tools/` that the script reads, minus pins and
baselines): 774 dead proof and spec functions, 206 unused public API items
(the tree moved since the first comparison, which had 876 and 229).
`dead_fns.py` reports 498 in the same crates (489 distinct file and name
pairs): all of them are in our 774, none is missing. Our 270 extra, by class:
157 are referenced from files that hold `cfg(test)` or `cfg(feature)` code
(tests and feature-gated negative controls, absent from the log; the class is
an upper bound, since a file with such code may also have live references);
61 are reachable only from functions we treat as unused API, because we do not
root pinned items and the script does; 41 share a name with an unrelated
function (it merges by name); 11 appear only in `use` lines. Whether callees of
an unused pinned function should count as live is a rule decision, so
`[roots] pins_are_roots = true` is an opt-in switch (pinned functions become
roots with reason `pin`; they are still in `api_pins`). With it on, Coral has
751 dead functions (247 extra), 23 fewer; the rest of the 61 are also reachable
only from other dead code.

Phase 4 status (2026-10-06): done. `verus_lint::sdk` (`Facts`, `Graph` with
`sccs`, `Rule`, `RuleMeta`, `Params`, `Findings`) and `verus_lint::run`; the CLI
moved into the library. `check` and `run` build the crate named by `[rules]
rust` and start its binary, which loads the SQL rules too, so one report covers
both. `examples/rust-rules/` is a workspace member with one rule
(`example/opaque-reveal-spread`); `tests/rules_crate.rs` drives the CLI through
build, gate, baseline update and error status. Findings: a child `cargo build`
started from a test inherits `CARGO_PKG_*` and related variables, which made
bundled DuckDB rebuild on every run (2 minutes against 2 seconds), so the CLI
removes them; `check` opens the database read-only; real logs have a use kind
`other` (1,100 on Coral) that the typed enum needed. Coral dogfood with a
scratch rules crate outside both repositories (dead-code rule on
`Facts::live()` plus `sccs()`): 774 findings, identical to the SQL rule;
a `check` with the Rust crate, 9 rules and the build check takes 6 to 10 s
(development-profile DuckDB).

Phase 6 status (2026-10-06): done. Levels, baseline file, set and metric
ratchets, `--update-baseline`, text, JSON and SARIF output, exit statuses (all
in sections 7 and 8). Coral dogfood: baseline of 9 rules (237 KB: 774 dead
functions, 711 trusted rows, fan-in metrics and so on), second `check` exits 0,
removing one set entry and halving one metric entry gives exactly 2 uncovered
gated findings, SARIF has 3,359 results. Not done: the `verify_*` tables and
`needs: dynamic` rules wait for phase 5 (a rule that needs them is skipped with
a note while `verify_fn` is missing or empty); no Parquet cache yet (phase 7).
