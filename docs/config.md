# Configuration reference

verus-lint reads `verus-lint.toml` from the workspace root (the directory given
by `--workspace`, default `.`). Use `--config FILE` for another path. Without a
file every setting has its default. Every section and key is optional.

The sections are `[extract]`, `[roots]`, `[rules]` (with `[rules.levels]` and
`[rules.params.<rule id>]`) and `[baseline]`. An unknown key inside one of
these sections is an error (exit status 2); an unknown top-level section is
ignored. Paths are relative to the workspace unless stated otherwise.

A complete file with every key set (the values are examples, not defaults):

```toml
[extract]
toolchain = ["./verify"]
crates = ["crates/**"]
exclude = ["my-cuda-sys"]

[roots]
patterns = ["theorem_*", "*::neg_*"]
public_api = false
pins = ["tools/pins/*.pin"]
pins_are_roots = false
name_files = ["tools/*.py", "!tools/baseline*"]

[rules]
dirs = ["lints/sql"]
rust = "lints"
rust_profile = "release"

[rules.levels]
"my/dead-proof-code" = "gate"
"my/big-proofs" = "note"

[rules.params."my/big-proofs"]
max_lines = 300

[baseline]
file = "verus-lint-baseline.json"
```

## `[extract]`

How `extract` (and `run`, and `verify`) call Verus.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `toolchain` | array of strings | `[]` | Command prefix placed before `cargo verus ...`. Use it for a container wrapper: with `["./verify"]` the tool runs `./verify cargo verus build ...` and `./verify verus --version --output-json`. Empty means `cargo` and `verus` from `PATH`. `--toolchain "./verify"` on the command line (split on spaces) replaces it. |
| `crates` | array of strings | `[]` (all verified members) | Which workspace members to extract: package names or manifest-directory globs (`crates/**`). Members are those with `[package.metadata.verus] verify = true`. `--crate NAME` (repeatable) replaces this list. `verify` ignores it and uses `--crate` only. |
| `exclude` | array of strings | `[]` | Members to skip, same patterns, for `verify = true` members Verus cannot build. `--exclude` adds to this list. Applies to `extract`, `run` and `verify`. |

```toml
[extract]
toolchain = ["./verify"]
crates = ["crates/**"]
exclude = ["my-cuda-sys"]
```

## `[roots]`

Functions that are live by definition, for dead-code rules. These settings fill
the `root_patterns`, `api_pins` and `root_names` tables and the `roots_*`
`meta` keys, and the `roots` view reads them. They are applied at `extract`
time, so changing them needs a new `extract` (use `extract --reuse-logs` to skip
running Verus). Every exec function, every implementation of a trait declared
outside the extracted crates, and every `#[verifier::type_invariant]` function
is a root without any configuration.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `patterns` | array of strings | `[]` | Globs for top theorems, negative controls and fixtures that nothing calls. `*` matches any run of characters, including `::` and `/`. A pattern without `::` matches the bare function name (`theorem_*`); one with `::` matches the VIR path or the friendly path (`*::neg_*`). Matching functions get reason `pattern`. |
| `public_api` | bool | `false` | Treat every `pub` function of the extracted crates as a root (reason `public_api`). |
| `pins` | array of strings | `[]` | Globs of API pin files. A pin file lists one function path or friendly name per line, ignoring blank lines and lines starting with `#`; a file with `## name` header lines uses those headers as entries instead. Entries are recorded in `api_pins`. Pinned functions are reported as public API, not rooted, unless `pins_are_roots` is set. |
| `pins_are_roots` | bool | `false` | Also make pinned functions roots (reason `pin`). Off by default: a pinned function nothing uses is unused API, and helpers only it calls are dead. |
| `name_files` | array of strings | `[]` | Globs of files outside the Rust sources that mention functions by name (scripts, lists, pin files). Every function whose name occurs as an identifier token in such a file is a root (reason `name_file`). A pattern starting with `!` excludes matching files. Matches are recorded in `root_names`. |

```toml
[roots]
patterns = ["theorem_*", "*::neg_*"]
pins = ["tools/pins/*.pin"]
pins_are_roots = false
name_files = ["tools/*.py", "!tools/baseline*"]
```

## `[rules]`

Where rules live and what they do.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `dirs` | array of strings | `[]` | Directories of SQL rule files (`*.sql`), relative to the workspace. `--rules DIR` (repeatable) adds more. No rules are built in. |
| `rust` | string | none | Directory of a Rust rules crate (it holds a `Cargo.toml`). `check` and `run` build it and start its binary with the same arguments; that binary also loads the SQL rules. |
| `rust_profile` | string | `"release"` | Cargo profile used to build the rules crate. `"dev"` and any custom profile name also work. |

### `[rules.levels]`

A table from rule id to level. Rules not listed run at `warn`.

| Level | Effect |
| --- | --- |
| `off` | The rule does not run. |
| `note` | Findings print as notes; the exit status never changes. |
| `warn` (default) | Findings print at the rule's severity; the exit status never changes. |
| `gate` | Findings the baseline does not cover make the run exit with status 1. |

```toml
[rules.levels]
"my/dead-proof-code" = "gate"
"my/big-proofs" = "note"
```

Any other value is a config error. Quote rule ids: they contain `/`.

### `[rules.params."<rule id>"]`

Numeric parameter values per rule, overriding the defaults the rule declares
(`-- params:` in SQL, `RuleMeta::param` in Rust). Values must be numbers
(integer or float); a string is a config error. Precedence, highest first:
`--param name=value` on the command line, this table, the rule's default.

```toml
[rules.params."my/big-proofs"]
max_lines = 300
```

A SQL rule reads it as `param('max_lines')`; a Rust rule as
`cx.params.get_u64("max_lines")`.

## `[baseline]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `file` | string | `"verus-lint-baseline.json"` | Baseline file. `--baseline FILE` on the command line replaces it. |

The baseline records what the project accepts today. `check --update-baseline`
writes it from the current findings of the rules that ran (only rules with a
`ratchet` header have entries), then exits 0. A rule's `ratchet` decides how a
finding is compared with it:

- `set`: covered when its `entity` is in the file.
- `metric, ratio = r, abs = a`: covered when the entity is in the file and the
  finding's `metric` is at most `max(r * base, base + a)`. A new entity is never
  covered. `ratio` defaults to 1 and `abs` to 0.

Entities in the file that a rule no longer reports are listed as fixed. They
stay until the next `--update-baseline`, so the baseline only shrinks on
purpose. Commit the file.

## Command-line settings that have no config key

`--workspace`, `--config`, `--out` (database and log directory, default
`.verus-lint`), `--target-dir` (cargo target directory for the Verus builds),
`--db`, `--only` (run rules whose id contains a text), `--top`, `--format`
(`text`, `json`, `sarif`) and `--output`. `verus-lint COMMAND --help` lists them
with defaults.
