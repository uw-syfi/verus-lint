# Fact schema reference

`verus-lint extract` writes one DuckDB database (`.verus-lint/facts.duckdb`).
Every table and view below can be queried with `verus-lint query "SELECT ..."`,
from a SQL rule, or from a Rust rule through `Facts::query`. The schema version
is `meta.schema_version` (currently `1.3.0`); adding a table, column or enum
value is a minor bump, renaming or removing one is a major bump.

Conventions:

- A **path** is the VIR path Verus uses internally, for example
  `crate::module::impl&%2::f`. A **friendly name** is the Rust-style name Verus
  prints in verification reports (`crate::module::Type::f`). Paths are unique
  per function; friendly names are not always.
- `fn_id` is a database-assigned integer, stable only within one database. Use
  `path` for anything that must survive re-extraction (rule entities, baselines).
- Types are DuckDB types: `VARCHAR`, `BIGINT`, `INTEGER`, `DOUBLE`, `BOOLEAN`.
  Any column can be null unless it is a key; nullable columns are called out.
- Static facts come from the VIR log (`cargo verus build -- --no-verify --log
  vir --log impl-names`). Dynamic facts come from `verus-lint verify`, which
  runs `cargo verus build --time-expanded --output-json`. Dynamic tables are
  empty until `verify` has run.
- This file is checked against `src/schema.sql` by `tests/docs_schema.rs`: a
  table or column missing here, or listed here and absent from the schema,
  fails `cargo test`. Every `sql` block below is executed against the test
  fixture by the same test.

Contents: [Metadata](#metadata) | [Functions and their uses](#functions-and-their-uses)
| [Proof-engineering facts](#proof-engineering-facts) | [Reachability](#reachability)
| [Dynamic facts](#dynamic-facts-verify) | [Extraction health](#extraction-health)

## Metadata

### `meta`

Key `key`. Free-form key/value pairs describing the extraction.

| Column | Type | Meaning |
| --- | --- | --- |
| `key` | VARCHAR | Key name (see below). |
| `value` | VARCHAR | Value as text. |

Keys: `schema_version`, `verus_version`, `verus_commit`, `rust_toolchain`,
`tool_version` (verus-lint's own version), `source_commit` (git HEAD of the
workspace, empty outside git), `source_dirty` (`true` when the working tree has
uncommitted changes), `extracted_at` (UTC, ISO 8601), `roots_public_api` and
`roots_pins` (the `[roots]` switches, read by the `roots` view), and after
`verify`: `verify_rows`, `unjoined_verify_rows`, `ambiguous_verify_rows`.

Populated by: `extract` and `verify` themselves, not by any Verus construct.

```sql
SELECT key, value FROM meta ORDER BY key;
```

### `crates`

Key `crate`. One row per extracted crate.

| Column | Type | Meaning |
| --- | --- | --- |
| `crate` | VARCHAR | Crate identifier (package name with `-` replaced by `_`). |
| `manifest` | VARCHAR | Path of the crate's `Cargo.toml`, relative to the workspace. |
| `log_bytes` | BIGINT | Size of the crate's VIR log. |
| `fn_count` | BIGINT | Number of `functions` rows from this crate. |
| `verified` | BOOLEAN | `true` once dynamic facts were loaded for the crate. |

Populated by: one row per workspace member with `[package.metadata.verus]
verify = true`; `verified` is set by `verify`.

```sql
SELECT crate, fn_count, verified FROM crates ORDER BY fn_count DESC;
```

### `modules`

Key `module`. Modules of the extracted crates.

| Column | Type | Meaning |
| --- | --- | --- |
| `module` | VARCHAR | Module path (`crate::a::b`). |
| `crate` | VARCHAR | Owning crate. |
| `file` | VARCHAR | A source file of the module, relative to the workspace. |

Populated by: the `(module_id ...)` forms in the VIR log; the file is that of
the module's first function.

```sql
SELECT module, file FROM modules ORDER BY module;
```

## Functions and their uses

### `functions`

Key `fn_id`. One row per function-like item of the extracted crates: `fn`
(exec, proof, spec), trait method declarations and implementations, and `const`
items.

| Column | Type | Meaning |
| --- | --- | --- |
| `fn_id` | BIGINT | Database id. |
| `path` | VARCHAR | VIR path, unique. |
| `friendly` | VARCHAR | Rust-style path as printed in Verus reports. |
| `crate` | VARCHAR | Crate identifier. |
| `module` | VARCHAR | Module path (`:owning_module`). |
| `name` | VARCHAR | Bare function name. |
| `self_type` | VARCHAR | For trait implementation methods, the implementing type (from `--log impl-names`); empty for other functions. |
| `trait_path` | VARCHAR | For trait declarations and implementations, the trait path; null for free functions and inherent methods. |
| `mode` | VARCHAR | `exec`, `proof` or `spec` (`:mode`). |
| `kind` | VARCHAR | `static` (free or inherent), `trait_decl` (declaration in a trait), `trait_impl` (implementation of a trait from an extracted crate), `foreign_trait_impl` (implementation of a trait from outside the extracted crates). |
| `item_kind` | VARCHAR | `function`, `const` or `static`. |
| `vis` | VARCHAR | `pub`, or the module the function is visible in. |
| `body_vis` | VARCHAR | Where the body is visible: `pub`, a module path (`open(in m)`, `open(crate)` and `closed` all record a restricted module), or `none` when there is no visible body. |
| `opaque` | BOOLEAN | Verus's `:opaqueness`: `true` when the body is hidden unless revealed. Verus marks proof and exec functions opaque, so filter on `mode = 'spec'` or use `open_spec`. |
| `reveal_vis` | VARCHAR | For a non-opaque function, the visibility the body is revealed at; null when opaque. |
| `external_body` | BOOLEAN | Marked `#[verifier::external_body]`. |
| `broadcast_forall` | BOOLEAN | A `broadcast proof fn`. |
| `broadcast_forall_only` | BOOLEAN | Verus's `broadcast_forall_only` function attribute, recorded as is. |
| `rlimit_attr` | VARCHAR | `#[verifier::rlimit(n)]` value as text (`inf` allowed); null when absent. |
| `spinoff_prover` | BOOLEAN | Marked `#[verifier::spinoff_prover]`. |
| `integer_ring` | BOOLEAN | Marked `#[verifier::integer_ring]`. |
| `bit_vector` | BOOLEAN | Marked `#[verifier::bit_vector]`. |
| `nonlinear` | BOOLEAN | Marked `#[verifier::nonlinear]`. |
| `has_body` | BOOLEAN | The log contains a body. |
| `file` | VARCHAR | Source file, relative to the workspace. |
| `line` | INTEGER | First line of the function header. |
| `end_line` | INTEGER | Last line of the body (the header form's own span covers only its first line, so this comes from the body expression). |
| `body_lines` | INTEGER | `end_line - line + 1`. |
| `n_requires` | INTEGER | Number of `requires` expressions. |
| `n_ensures` | INTEGER | Number of `ensures` expressions. |
| `has_default` | BOOLEAN | A trait method declaration that carries a default body. |
| `trait_method` | VARCHAR | For a `trait_impl` method, the path of the declaration it implements. |
| `type_invariant` | BOOLEAN | Marked `#[verifier::type_invariant]`; Verus uses these implicitly. |
| `generated` | BOOLEAN | Compiler-made datatype field accessor (`arrow_*`); there is no source item to edit. |

Populated by: each `(@ "span" (Function ...))` form of the log. `file` and
`line` come from the span; `mode`, `kind`, `opaque`, `body_vis`, the attribute
flags and `n_requires`/`n_ensures` from the corresponding `Function` fields.
Imported items (for example vstd's) appear in the log only as pruned copies and
are dropped, so only the crate's own functions have rows.

```sql
SELECT mode, count(*) AS n, sum(body_lines) AS lines
FROM functions WHERE NOT generated GROUP BY mode ORDER BY mode;
```

### `uses`

No key (a multiset). One row per reference from a function to another item,
with the place where it occurs.

| Column | Type | Meaning |
| --- | --- | --- |
| `caller_id` | BIGINT | `functions.fn_id` of the referencing function. |
| `callee_path` | VARCHAR | Path of the referenced item as written in the log. |
| `callee_id` | BIGINT | `functions.fn_id` of the callee; null when it is outside the extracted crates (vstd, other workspaces) or is a broadcast group. |
| `section` | VARCHAR | Part of the caller: `require`, `ensure`, `returns`, `decrease`, `decrease_by`, `body`, or `hide` (the function's `hidden` list). |
| `kind` | VARCHAR | `call`, `reveal` (a `reveal` of an opaque definition), `broadcast_use` (function-local `broadcast use`), `fn_value` (a function used as a value), `resolved_impl` (a trait method call resolved to one implementation), `hide`, `other` (any other path mention, for example a constant). |
| `in_trigger` | BOOLEAN | The reference sits inside a quantifier trigger. |
| `fuel` | VARCHAR | For `reveal` and `broadcast_use`, the fuel value Verus records (for example `1`); null otherwise. |
| `file` | VARCHAR | Source file. |
| `line` | INTEGER | Source line. |
| `col` | INTEGER | Source column. |

Populated by: walking each function's expression trees. A `Call` node gives
`kind = 'call'`; a `Fuel` node gives `reveal`, or `broadcast_use` when its
broadcast flag is set; `ExecFnByName` gives `fn_value`; `CallTargetKind` gives
`resolved_impl`; arguments of a `#![trigger ...]` or `#[trigger]` set
`in_trigger`. `callee_id` is filled after all crates are loaded, by exact path
match. Type mentions are not uses.

```sql
SELECT f.path AS caller, u.callee_path, u.section, u.kind, u.file, u.line
FROM uses u JOIN functions f ON f.fn_id = u.caller_id
WHERE u.kind = 'call' AND u.in_trigger ORDER BY f.path LIMIT 10;
```

### `module_uses`

No key. Module-level `broadcast use` items.

| Column | Type | Meaning |
| --- | --- | --- |
| `module` | VARCHAR | Module containing the `broadcast use`. |
| `callee_path` | VARCHAR | Lemma or group named, as resolved (`crate::` expanded). |
| `callee_id` | BIGINT | Function id when the name resolves to an extracted function; null for groups and outside items. |
| `kind` | VARCHAR | Always `broadcast_use`. |
| `file` | VARCHAR | Source file. |
| `line` | INTEGER | Source line. |

Populated by: a source scan, because Verus's log prints module ids only. The
scan covers the files that hold the crate's functions, skips comments and
strings, drops items inside a known function span (those are in `uses`), and
expands brace and comma lists. Names that cannot be resolved produce an
`unresolved_broadcast_use` row in `warnings`.

```sql
SELECT module, callee_path, file, line FROM module_uses ORDER BY file, line;
```

### `broadcast_groups`

Key `path`. Broadcast groups defined by an extracted crate.

| Column | Type | Meaning |
| --- | --- | --- |
| `path` | VARCHAR | Group path. |
| `crate` | VARCHAR | Defining crate. |

Populated by: `(group_id ...)` forms in the log. Groups of imported crates are
not listed.

```sql
SELECT path FROM broadcast_groups ORDER BY path;
```

### `group_members`

No key. Members of `broadcast group` items.

| Column | Type | Meaning |
| --- | --- | --- |
| `group_path` | VARCHAR | Group path. |
| `member_path` | VARCHAR | Member as resolved. |
| `member_id` | BIGINT | Function id of the member; null for items outside the extracted crates. |
| `file` | VARCHAR | Source file. |
| `line` | INTEGER | Line of the `broadcast group` item. |

Populated by: a source scan, because the log carries group ids without member
lists. Needed for reachability: a live group makes its members live.

```sql
SELECT group_path, member_path, member_id FROM group_members ORDER BY 1, 2;
```

### `trait_impls`

Key `impl_path`. One row per trait implementation.

| Column | Type | Meaning |
| --- | --- | --- |
| `impl_path` | VARCHAR | Implementation path (`crate::m::impl&%N`). |
| `trait_path` | VARCHAR | Trait implemented. |
| `self_type` | VARCHAR | Implementing type. |
| `crate` | VARCHAR | Crate holding the implementation. |
| `file` | VARCHAR | Source file. |
| `line` | INTEGER | Source line. |

Populated by: the `--log impl-names` output (`crate.impl_names`).

```sql
SELECT trait_path, count(*) AS impls FROM trait_impls GROUP BY 1 ORDER BY 2 DESC;
```

## Proof-engineering facts

### `quantifiers`

No key. One row per quantifier in a function's specs or proofs.

| Column | Type | Meaning |
| --- | --- | --- |
| `fn_id` | BIGINT | Function containing the quantifier. |
| `quant` | VARCHAR | `forall`, `exists` or `choose`. |
| `trigger` | VARCHAR | `explicit` (a trigger is written), `auto_annotation` (`#![auto]`), `none` (Verus picks triggers). |
| `n_triggers` | INTEGER | Number of explicit trigger groups (0 unless `explicit`). |
| `section` | VARCHAR | Same values as `uses.section`. |
| `file` | VARCHAR | Source file. |
| `line` | INTEGER | Source line. |

Populated by: `Quant` and `Choose` nodes, classified by the `WithTriggers`
wrapper inside them. Nested quantifiers are classified on their own.

```sql
SELECT f.path, q.quant, q.trigger, q.line
FROM quantifiers q JOIN functions f USING (fn_id) WHERE q.trigger = 'none';
```

### `trusted`

No key. The trusted surface: places where the proof relies on something Verus
does not check.

| Column | Type | Meaning |
| --- | --- | --- |
| `fn_id` | BIGINT | Function concerned; null for `external_type` rows. |
| `kind` | VARCHAR | `assume`, `admit`, `external_body`, `external_fn`, `external_type`, `assume_specification`, `broadcast_axiom`. |
| `file` | VARCHAR | Source file. |
| `line` | INTEGER | Source line. |
| `text` | VARCHAR | For `external_fn` and `external_type`, the item path; empty otherwise. |

Populated by: `assume(...)` calls (`AssertAssume` with `is_assume`; `admit()`
is recorded as `admit`, an assume of `false`); `#[verifier::external_body]`
(`external_body`, or `broadcast_axiom` on a broadcast function); an
`assume_specification` proxy; and `(external_fn ...)` and `(external_type ...)`
forms. Items marked `#[verifier::external]` are not in the log.

```sql
SELECT kind, count(*) AS n FROM trusted GROUP BY kind ORDER BY n DESC;
```

## Reachability

These tables and views support dead-code rules. They are derived from the
`[roots]` config section (see [config.md](config.md)).

### `root_patterns`

No key. One row per `[roots] patterns` entry.

| Column | Type | Meaning |
| --- | --- | --- |
| `pattern` | VARCHAR | The glob as configured. |
| `like_pattern` | VARCHAR | The glob as a SQL `LIKE` pattern, `\` as the escape character. |
| `by_name` | BOOLEAN | `true` when the pattern has no `::` and so matches the bare function name; otherwise it matches `path` or `friendly`. |

Populated by: `extract`, from the config.

```sql
SELECT pattern, by_name FROM root_patterns;
```

### `api_pins`

No key. Entries of the `[roots] pins` files and the functions they name.

| Column | Type | Meaning |
| --- | --- | --- |
| `entry` | VARCHAR | The pin entry (a function path or friendly name). |
| `file` | VARCHAR | Pin file, relative to the workspace. |
| `line` | INTEGER | Line in the pin file. |
| `fn_id` | BIGINT | Matched function; null when nothing matched. |

Populated by: `extract`. An entry matches a path or friendly name exactly, then
a path with that suffix, then a `pub` static function of that name.

```sql
SELECT entry, fn_id IS NOT NULL AS matched FROM api_pins;
```

### `root_names`

No key. Function names mentioned by files outside the Rust sources.

| Column | Type | Meaning |
| --- | --- | --- |
| `name` | VARCHAR | Function name (only names of existing functions). |
| `file` | VARCHAR | The first file that mentions it. |

Populated by: `extract`, scanning the `[roots] name_files` globs for identifier
tokens.

```sql
SELECT name, file FROM root_names ORDER BY name;
```

### `dead_scc`

Key `fn_id`. Strongly connected components of the dead subgraph.

| Column | Type | Meaning |
| --- | --- | --- |
| `fn_id` | BIGINT | A dead function. |
| `scc_id` | BIGINT | Smallest `fn_id` of the component. |
| `scc_size` | INTEGER | Number of functions in the component; above 1 for a dead cycle. |

Populated by: `extract`, from the functions not in `live_nodes`.

```sql
SELECT scc_id, scc_size FROM dead_scc WHERE scc_size > 1 GROUP BY ALL;
```

### `edges` (view)

Resolved uses, one row per caller, callee and section.

| Column | Type | Meaning |
| --- | --- | --- |
| `caller_id` | BIGINT | Referencing function. |
| `callee_id` | BIGINT | Referenced function (never null here). |
| `section` | VARCHAR | As in `uses`. |
| `kind` | VARCHAR | As in `uses`. |

Defined as: `SELECT DISTINCT` of `uses` rows with a non-null `callee_id`.

```sql
SELECT callee_id, count(DISTINCT caller_id) AS callers
FROM edges GROUP BY 1 ORDER BY 2 DESC LIMIT 5;
```

### `open_spec` (view)

Same columns as `functions` (see above). Spec functions whose body is visible
beyond their own module: `mode = 'spec'`, `item_kind = 'function'`, not opaque,
`body_vis` neither `none` nor the function's own module. This covers `pub
open`, `open(crate)` and `open(in ancestor)`.

```sql
SELECT path, body_lines FROM open_spec ORDER BY body_lines DESC LIMIT 5;
```

### `roots` (view)

Functions that are live by definition.

| Column | Type | Meaning |
| --- | --- | --- |
| `fn_id` | BIGINT | The root function. |
| `reason` | VARCHAR | Why: `exec` (every exec function), `pattern` (matches a `[roots] patterns` entry), `name_file` (named in a `name_files` file), `foreign_trait_impl` (implements a trait declared outside the extracted crates, or whose declaration is not extracted), `type_invariant`, `pin` (pinned, only when `pins_are_roots`), `public_api` (every `pub` function, only when `public_api`). |

A function can appear more than once, with different reasons.

```sql
SELECT reason, count(*) FROM roots GROUP BY reason ORDER BY reason;
```

### `graph_edges` (view)

Dependency graph over path nodes.

| Column | Type | Meaning |
| --- | --- | --- |
| `src` | VARCHAR | Source node: a function path, a group path or `mod:<module>`. |
| `dst` | VARCHAR | Destination node. |

Edges: every `uses` row (caller path to `callee_path`); function to its
`mod:` module node; `mod:` node to each module-level `broadcast use` callee;
group to each member; trait method declaration to every implementation of it.

```sql
SELECT src, dst FROM graph_edges WHERE src LIKE 'mod:%' LIMIT 5;
```

### `live_nodes` (view)

| Column | Type | Meaning |
| --- | --- | --- |
| `path` | VARCHAR | A node reachable from the roots over `graph_edges`. Join to `functions.path` to get functions; the rest are `mod:` nodes and groups. |

```sql
SELECT count(*) AS live_functions
FROM functions f JOIN live_nodes l ON l.path = f.path;
```

## Dynamic facts (`verify`)

`verus-lint verify [--seeds 1,2,3]` runs one verification per crate and seed
and loads the reports. A report row is joined to `functions` by friendly name,
then by trait implementation, inherent impl type and the module as tie-break;
rows that match no single function keep a null `fn_id` and are counted in
`meta.unjoined_verify_rows` and `meta.ambiguous_verify_rows`. A report from a
Verus other than the one that produced the facts is refused.

### `runs`

Key `run_id`. One row per verification run of one crate.

| Column | Type | Meaning |
| --- | --- | --- |
| `run_id` | BIGINT | Run id. |
| `crate` | VARCHAR | Crate verified. |
| `seed` | INTEGER | Solver seed (`smt.random_seed`); null when the run set none. |
| `verus_args` | VARCHAR | Command line of the run. |
| `source_commit` | VARCHAR | Workspace commit. |
| `started_at` | VARCHAR | Start time (UTC, ISO 8601). |
| `wall_s` | DOUBLE | Wall-clock seconds. |

```sql
SELECT run_id, crate, seed, wall_s FROM runs ORDER BY run_id;
```

### `verify_fn`

No key. Per-function verification cost.

| Column | Type | Meaning |
| --- | --- | --- |
| `run_id` | BIGINT | Run the row belongs to. |
| `fn_id` | BIGINT | Joined function; null when the report name matched no single function. |
| `friendly` | VARCHAR | Function name as the report prints it. |
| `crate` | VARCHAR | Crate. |
| `module` | VARCHAR | Module the function was verified under. |
| `mode` | VARCHAR | `exec`, `proof` or `spec`. |
| `rlimit` | BIGINT | Verus's resource count, the deterministic cost measure. |
| `time_us` | BIGINT | SMT time in microseconds (varies between runs). |
| `success` | BOOLEAN | The function verified. |
| `seed` | INTEGER | Seed of the run; null when none. |

Populated by: the `function-breakdown` entries of the report's per-module SMT
times.

```sql
SELECT friendly, rlimit, time_us FROM verify_fn ORDER BY rlimit DESC LIMIT 10;
```

### `verify_module`

No key. Per-module cost.

| Column | Type | Meaning |
| --- | --- | --- |
| `run_id` | BIGINT | Run. |
| `module` | VARCHAR | Module path. |
| `crate` | VARCHAR | Crate. |
| `rlimit` | BIGINT | Sum over the module's own functions' queries. |
| `smt_time_ms` | BIGINT | Sum of their SMT time. |
| `session_time_ms` | BIGINT | Longest solver session of the module (the main one or a spinoff). |

```sql
SELECT module, rlimit, session_time_ms FROM verify_module ORDER BY rlimit DESC;
```

### `verify_default_runs` (view)

| Column | Type | Meaning |
| --- | --- | --- |
| `run_id` | BIGINT | For each crate, the run a rule should read: the one without a seed if there is one, else the lowest seed; the newest of those. |

```sql
SELECT run_id FROM verify_default_runs;
```

### `verify_latest` (view)

Same columns as `verify_fn`, restricted to each crate's default run
(`verify_default_runs`). Use this for rules that read one run.

```sql
SELECT friendly, rlimit FROM verify_latest ORDER BY rlimit DESC LIMIT 5;
```

### `verify_module_latest` (view)

Same columns as `verify_module`, restricted to each crate's default run.

```sql
SELECT module, rlimit FROM verify_module_latest ORDER BY rlimit DESC LIMIT 5;
```

### `verify_worst` (view)

Cost of each function across all runs of its crate (all seeds).

| Column | Type | Meaning |
| --- | --- | --- |
| `crate` | VARCHAR | Crate. |
| `friendly` | VARCHAR | Function name as the report prints it. |
| `fn_id` | BIGINT | Joined function (one of them, if the name is shared). |
| `n_runs` | BIGINT | Distinct runs that include the function. |
| `max_rlimit` | BIGINT | Largest `rlimit` over the runs. |
| `min_rlimit` | BIGINT | Smallest `rlimit` over the runs. |
| `all_ok` | BOOLEAN | The function verified in every run. |

A large gap between `max_rlimit` and `min_rlimit` marks a proof that depends
on the solver seed.

```sql
SELECT friendly, max_rlimit, min_rlimit, n_runs
FROM verify_worst ORDER BY max_rlimit - min_rlimit DESC LIMIT 5;
```

## Extraction health

### `warnings`

No key. Problems a rule can refuse to trust.

| Column | Type | Meaning |
| --- | --- | --- |
| `crate` | VARCHAR | Crate concerned. |
| `what` | VARCHAR | Kind: `feature_gated_item` (a `#[cfg(... feature ...)]` attribute; code behind it is absent from the log and so are its references), `unresolved_broadcast_use`, `module_use_in_function_less_file`, `scan_unreadable_file`, `ambiguous_broadcast_group`. |
| `detail` | VARCHAR | Usually `file:line: text`. |

Populated by: the source scan that fills `module_uses` and `group_members`.

```sql
SELECT what, count(*) FROM warnings GROUP BY what;
```
