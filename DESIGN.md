# verus-lint design

verus-lint is a lint and analysis tool for Verus codebases. It extracts facts
about functions and their uses from Verus's own intermediate representation
(VIR) and from Verus's verification reports, stores them in an embedded DuckDB
database, and runs rules written in SQL or in Rust against that database.
Coral (`llm-eq/coral/crates`) is the first user; nothing in the core or the
built-in rules is specific to Coral.

Evidence for the choices below is in `spike/notes.md` (Verus
0.2026.07.18.3a4d30b, measured on coral-spec and coral-effects).

## 1. Overview

```
verus-lint extract  ->  cargo verus build -p C -- --no-verify --log vir --log impl-names   (per crate)
                        cargo verus build -p C -- --time-expanded --output-json            (optional, dynamic)
                    ->  parse logs and reports  ->  facts.duckdb (one database per workspace commit)
verus-lint check    ->  build lints/ (if present)  ->  run SQL rules and Rust rules
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
| `functions` | `fn_id` | `path`, `friendly`, `crate`, `module`, `name`, `self_type`, `trait_path`, `mode` (exec, proof, spec), `kind` (static, trait_decl, trait_impl, foreign_trait_impl), `item_kind` (function, const), `vis` (pub, or the restricting module), `body_vis` (open/closed: pub, module, or none for no body), `opaque` (bool), `reveal_vis`, `external_body`, `broadcast_forall`, `broadcast_forall_only`, `rlimit_attr` (nullable, `inf` allowed), `spinoff_prover`, `integer_ring`, `bit_vector`, `nonlinear`, `has_body`, `file`, `line`, `end_line`, `body_lines`, `n_requires`, `n_ensures` |
| `uses` | none (multiset) | `caller_id`, `callee_path`, `callee_id` (null when the callee is outside the extracted crates), `section` (require, ensure, returns, decrease, decrease_by, body, hide, module), `kind` (call, reveal, broadcast_use, fn_value, resolved_impl, hide), `in_trigger` (bool), `fuel`, `file`, `line`, `col` |
| `module_uses` | none | `module`, `callee_path`, `callee_id`, `kind` (broadcast_use), `file`, `line` (module-level `broadcast use`, see section 4.3) |
| `quantifiers` | none | `fn_id`, `quant` (forall, exists, choose), `trigger` (explicit, auto_annotation, none), `n_triggers`, `section`, `file`, `line` |
| `trusted` | none | `fn_id` (nullable), `kind` (assume, admit, external_body, external_fn, external_type, assume_specification, broadcast_axiom), `file`, `line`, `text` |
| `trait_impls` | `impl_path` | `trait_path`, `self_type`, `file`, `line` (from `--log impl-names`) |
| `verify_fn` | `run_id, fn_id` | `friendly`, `rlimit`, `time_us`, `success`, `seed` |
| `verify_module` | `run_id, module` | `rlimit`, `smt_time_ms`, `session_time_ms` |
| `runs` | `run_id` | `crate`, `seed`, `verus_args`, `source_commit`, `started_at`, `wall_s` |

Derived views shipped with the schema: `edges` (uses with resolved
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
  writes module ids only). Version 1 recovers it with a source scan of the
  files listed in `modules`: `broadcast use` items, their paths resolved
  against the function table and broadcast group names (suffix match within
  the `use` scope; unresolved names are reported as extraction warnings). We
  will also propose an upstream printer change that prints `ModuleX::reveals`;
  once released, the scan is dropped for that Verus version.
- Broadcast group membership: to be confirmed in phase 1 how groups appear
  (the log shows `group_id` forms); fallback is the same source scan.

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

`verus-lint check` runs `cargo build --release --manifest-path lints/Cargo.toml`
and executes the result with the database path and resolved config. Without a
`lints/` crate it runs its own binary, which is `verus_lint::run(&[])`. `run`
always includes the built-in rules and the SQL rule directories, so a user
crate only adds rules. No dynamic libraries are loaded.

API:

```rust
pub trait Rule {
    fn meta(&self) -> RuleMeta;                 // id, summary, severity, needs, ratchet, params
    fn check(&self, cx: &Cx, out: &mut Findings) -> anyhow::Result<()>;
}

pub struct Cx<'a> { pub facts: &'a Facts, pub params: &'a Params }

impl Facts {
    pub fn functions(&self) -> &[Function];                   // typed rows of `functions`
    pub fn by_path(&self, path: &str) -> Option<&Function>;
    pub fn uses_from(&self, f: FnId) -> &[Use];
    pub fn uses_of(&self, f: FnId) -> &[Use];
    pub fn graph(&self, keep: impl Fn(&Use) -> bool) -> Graph; // filtered use graph
    pub fn roots(&self) -> &[FnId];                             // config + implicit roots
    pub fn verify(&self) -> Option<&VerifyFacts>;               // dynamic facts, if extracted
    pub fn query(&self, sql: &str) -> anyhow::Result<Rows>;     // DuckDB, read-only
}

impl Graph {
    pub fn reachable(&self, from: &[FnId]) -> FnSet;
    pub fn sccs(&self) -> Vec<Vec<FnId>>;
    pub fn callers(&self, f: FnId) -> &[FnId];
}

impl Findings {
    pub fn push(&mut self, f: Finding);   // entity, message, location, metric, properties
}
```

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

Default roots (used by `Facts::roots` and the `roots` view): exec functions;
`#[test]` functions; functions matched by config root patterns (top theorems
and negative controls); API pin entries are not roots, they are reported in the
separate "unused public API" category; trait method
implementations whose trait declaration is live; when `roots.public_api` is
on (for libraries with users outside the workspace), every `pub` function of
the listed crates. Uses through triggers, reveals, `hide`, and cross-crate
calls are ordinary edges.

## 7. Configuration

`verus-lint.toml` at the workspace root:

```toml
[extract]
toolchain = ["./coral/verify"]          # command prefix for cargo verus; default none
crates = ["coral/crates/**"]            # members to extract; default all verified members

[roots]
patterns = ["*::theorem_*", "*::neg_*", "*::fixture_*"]   # top theorems, negative controls
public_api = false
pins = ["tools/pins/*.pin"]             # API pin files, one function path or friendly name per line;
                                        # not roots: listed items are reported as "unused public API"

[rules]
dirs = ["lints/sql"]                    # SQL rule directories, in addition to built-ins
rust = "lints"                          # Rust rule crate; omitted if absent

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

Levels: `warn` findings are printed and do not change the exit status;
`gate` findings that the baseline does not cover make `check` exit 1.

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
`max(r * base, base + a)` or the entity is new and over the rule's threshold.
Entities in the baseline that no longer appear are reported as fixed, so the
baseline only shrinks unless updated on purpose.

## 8. Output

- Text (default): one line per finding, `file:line: level rule-id: message`,
  then a summary per rule; `--top N` tables for metric rules.
- JSON: `{ meta, findings: [{rule, level, entity, message, file, line, metric, properties, baselined}] }`, stable field order, for scripts.
- SARIF 2.1.0: one `run`, rules as `reportingDescriptor`s with their summary,
  findings as `result`s with `partialFingerprints.entity` set to the entity so
  code-scanning UIs track findings across line moves.

Exit status: 0 clean, 1 gate findings, 2 rule or config error, 3 unsupported
Verus version.

## 9. Performance targets

Measured base: coral-spec (82k lines) gives a 148 MB log, parsed in 3 s on
one thread; `--no-verify` extraction takes about 42 s of Verus time.

| Step | Target (all of Coral, about 250k lines in 12 verified crates) |
| --- | --- |
| Parse and load all logs, cold cache | under 10 s on 8 cores, under 2 GB memory |
| Load from Parquet cache, nothing changed | under 1 s |
| All built-in SQL and Rust rules | under 5 s total; any single rule under 1 s |
| `check` after a one-crate change | Verus `--no-verify` time of that crate and its dependents plus 5 s |
| Logging overhead in a verify run | under 5% of Verus time (measured 1.8%) |

## 10. Built-in rules

| Rule id | What it reports |
| --- | --- |
| `verus/dead-proof-code` | Proof and spec functions unreachable from roots, dead cycles grouped by SCC |
| `verus/dead-exec-ghost` | Exec functions only reachable from dead functions (off by default) |
| `verus/fanin-open-spec` | Open spec definitions by fan-in outside their module (functions, modules, crates) |
| `verus/fanin-reveal` | Closed or opaque definitions by reveal sites outside their module |
| `verus/rlimit-headroom` | Functions over a fraction of the rlimit budget (warn and fail fractions) |
| `verus/rlimit-function` | Per-function rlimit ratchet against the baseline (metric ratchet) |
| `verus/rlimit-module` | Per-module rlimit sum and session time against budgets |
| `verus/seed-instability` | Functions whose worst-seed rlimit exceeds the default-seed rlimit by a ratio, or which fail under some seed |
| `verus/hotspot-growth` | Modules whose total rlimit grew by a fraction and an absolute amount since the baseline |
| `verus/spinoff-candidate` | Heavy functions in a heavy module that lack `spinoff_prover` |
| `verus/quantifier-no-trigger` | Quantifiers with no explicit trigger (`#[trigger]` or `#![trigger]`); a warning |
| `verus/quantifier-auto-trigger-note` | Verus's own "automatically chose triggers" notes, from the build log; informational, separate from the rule above |
| `verus/trait-spec-default` | Trait spec functions with a default body that implementations may override, and the implementations that do |
| `verus/trusted-inventory` | Every `assume`, `admit`, `external_body`, `external_fn`, `assume_specification` and broadcast axiom, with a count per crate (a set ratchet keeps the trusted surface from growing silently) |
| `verus/extraction-health` | Unjoined verification rows, unresolved `broadcast use` names, crates without logs |

## 11. Coral's Python tools as rules

| Tool | Becomes | Notes |
| --- | --- | --- |
| `tools/fanin.py` (syntactic mode) | `verus/fanin-open-spec`, `verus/fanin-reveal` | Exact paths replace import-evidence matching, so the `amb` column and the method and trait-impl blind spots go away. `--compare` becomes the metric ratchet. `--detail NAME` becomes `verus-lint query fanin --entity NAME`. The semantic mode (close a definition, re-verify) stays a separate experiment driver; the rule's count is the input to it. |
| `tools/provenance_sites.py` | A Coral SQL rule in `lints/sql/` plus a small table of the old-form names and the path to phase buckets | Counts uses (and source lines) naming each listed fact, bucketed by caller path. Uses replace grep, so comments and strings no longer need special handling. Its phase table is Coral data, not core. |
| `tools/budget.py` | `verus/rlimit-headroom`, `verus/rlimit-function`, `verus/rlimit-module`, `verus/seed-instability`, `verus/hotspot-growth`, `verus/spinoff-candidate` | `baseline.json` and `rlimit_exceptions.txt` become the baseline file and per-entity param overrides. `--run` becomes `verus-lint verify`. |
| `tools/dead_fns.py` (also present) | `verus/dead-proof-code` | Its `ROOTS` list moves to `[roots]`; name merging across modules disappears. |

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
3. How are broadcast group members represented in the log (only `group_id`
   forms were seen)? Phase 1 checks this on vstd groups.

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
- License: MIT or Apache-2.0 (dual); the repository stays private for now.

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
| 1 | Workspace (`verus-lint` core, CLI, SDK in one crate to start); version table and check; streaming parser to `functions` and `uses`; DuckDB load; `extract` driving `./coral/verify` per crate; SQL runner with header parsing; `verus/fanin-open-spec` and `verus/fanin-reveal`. Ends with a run on Coral (llm-eq, read-only) whose top fan-in table is compared to `fanin.py --top 20`, with every difference explained. | 1.5 |
| 2 | Remaining static facts: `quantifiers`, `trusted`, `trait_impls`, module `broadcast use` scan; fixture crate and parser fixture tests; `verus/quantifier-auto-trigger`, `verus/trusted-inventory`, `verus/trait-spec-default`. | 1.5 |
| 3 | Roots config, `roots` view, `verus/dead-proof-code` in SQL and Rust (with SCC grouping); compare against `dead_fns.py` on Coral. | 1.0 |
| 4 | Rust SDK surface (`Facts`, `Graph`, `Rule`, `Findings`, `run`), `lints/` crate build and execution by the CLI, one example user rule. | 1.5 |
| 5 | Dynamic facts: `verify` command, report ingestion, friendly-name join, seeds; `verus/rlimit-*`, `verus/seed-instability`, `verus/hotspot-growth`, `verus/spinoff-candidate`; compare with `budget.py` on Coral. | 1.5 |
| 6 | Config levels, baseline file, set and metric ratchets, `--update-baseline`; text, JSON and SARIF output; exit statuses. | 1.5 |
| 7 | Caching by input hash with Parquet, parallel parse, performance targets measured on Coral; `verus/extraction-health`. | 1.0 |
| 8 | Second codebase (a public Verus project, for example a vstd-only example crate set or a published Verus verification project) to check genericity; docs; Coral `provenance_sites` rule as a worked example of a project rule. | 1.0 |

Total: about 10.5 agent hours.
