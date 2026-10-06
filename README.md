# verus-lint

Lint and analysis tool for Verus codebases (work in progress, private).

It runs Verus's front end on each verified crate of a Cargo workspace, reads the
VIR log (`--log vir`), loads functions and their uses into an embedded DuckDB
database, and runs rules against it. The rules are yours: SQL files or a small
Rust crate in your repository. The tool ships mechanism only; `examples/` holds
sample rules to copy. `DESIGN.md` has the schema and the reasoning.

## Install

```sh
cargo install --git https://github.com/uw-syfi/verus-lint
```

Needs Rust 1.88 or newer and a Verus toolchain that provides `cargo verus`
(a container wrapper works; see `toolchain` below). Only the Verus releases in
`src/version.rs` are accepted; any other exits with status 3. The supported
release is 0.2026.07.18.3a4d30b.

## Quickstart

In a workspace whose Verus crates set `[package.metadata.verus] verify = true`:

```sh
verus-lint run --rules lints/sql
```

`run` extracts facts (one `cargo verus build -- --no-verify --log vir` per
crate) into `.verus-lint/facts.duckdb`, then runs the rules in `lints/sql`.
To see the available data without writing a rule:

```sh
verus-lint query "SELECT mode, count(*) FROM functions GROUP BY mode"
verus-lint query "SELECT * FROM roots LIMIT 5"
```

Put the settings in `verus-lint.toml` at the workspace root so the command line
stays short:

```toml
[extract]
toolchain = ["./coral/verify"]        # command prefix for `cargo verus`
crates = ["coral/crates/**"]          # package names or manifest-directory globs
exclude = ["sea-lion-cuda-sys"]       # verified members Verus cannot build

[roots]                               # live by definition, for dead-code rules
patterns = ["theorem_*", "neg_*"]     # bare name globs; with `::` they match paths
name_files = ["tools/*.py", "!tools/baseline*"]  # names mentioned by other tooling
pins = ["tools/pins/*.pin"]           # public API pins (reported, not roots)
pins_are_roots = false                # true: pinned functions are roots too
public_api = false                    # true: every `pub` function is a root

[rules]
dirs = ["lints/sql"]                  # SQL rule directories
rust = "lints"                        # Rust rules crate (see below)

[rules.levels]                        # off, note, warn (default) or gate
"my/dead-proof-code" = "gate"

[rules.params."my/fanin-open-spec"]   # numeric parameters, per rule
min_fns = 30

[baseline]
file = "verus-lint-baseline.json"     # the default
```

## Dynamic facts: `verify`

`verus-lint verify [--seeds 1,2,3]` runs `cargo verus build --time-expanded
--output-json` per crate (and per seed, via `smt.random_seed`), cleans the
crate first (a fresh crate prints no report), and loads `runs`, `verify_fn`
and `verify_module`. Rows join to `functions` by friendly name, then path, with
the module as tie-break; unjoined rows keep a null `fn_id` and are counted in
`meta.unjoined_verify_rows`. A report from a different Verus than the facts is
refused. `verify --report FILE` ingests an existing report, `--reuse-reports`
re-ingests saved ones. Views: `verify_latest` (each crate's default run),
`verify_worst` (maximum across seeds), `verify_module_latest`. Rules that read
them carry the header `needs: dynamic` and are skipped with a note until a run
exists.

`extract` and `check` are also separate commands (`extract` is the slow one;
`check` reads the database and takes seconds). `extract --reuse-logs` re-parses
logs from an earlier run without calling Verus.

## What the facts miss

The VIR log describes one build. Code behind a `cfg(feature = ...)` that the
extraction build does not enable, `#[cfg(test)]` code, and crates outside the
extraction are absent, and so are their references to other functions. A
function used only from such code looks dead. Each feature gate found in the
sources is recorded in the `warnings` table (`what = 'feature_gated_item'`);
`examples/rules/extraction-health.sql` lists them. Names that tooling outside
Rust refers to can be rooted with `[roots] name_files`.

## Writing SQL rules

A rule is one `.sql` file: a comment header and one `SELECT`.

```sql
-- id: my/big-proofs
-- summary: Proof functions with long bodies.
-- severity: warning
-- params: max_lines = 200
-- ratchet: metric, ratio = 1.1, abs = 10
SELECT path AS entity, file, line, body_lines AS metric,
       format('{} lines', body_lines) AS message
FROM functions
WHERE mode = 'proof' AND body_lines > param('max_lines');
```

Header keys: `id` and `summary` (required), `severity` (note, warning or error;
warning by default), `params`, `ratchet` (`set` or `metric, ratio = r, abs = a`),
`needs: dynamic` and `schema`. Each is one line, with no trailing comment.

Required result columns: `entity` (a stable key such as a function path, never
a line number; the baseline uses it) and `message`. Optional: `file`, `line`,
`metric` (compared by metric ratchets), `severity` (per row); any other column
becomes a property in JSON and SARIF. `param('name')` is replaced by the
numeric value (header default, then config, then `--param name=value`). The
tables and views are listed in `DESIGN.md` section 2; `roots`, `graph_edges`,
`live_nodes` and `dead_scc` support reachability rules. See `examples/rules/`.

## Writing Rust rules

For checks that SQL expresses badly (graphs, per-function logic), write a small
crate, by convention `lints/`, and name it in `[rules] rust`:

```toml
[package]
name = "my-lints"
edition = "2024"
[dependencies]
verus-lint = { git = "https://github.com/uw-syfi/verus-lint", rev = "..." }
[workspace]            # if the crate sits inside another Cargo workspace
```

```rust
use verus_lint::sdk::{Cx, Findings, Rule, RuleMeta};
use verus_lint::rules::Finding;

struct BigProofs;
impl Rule for BigProofs {
    fn meta(&self) -> RuleMeta {
        RuleMeta::new("my/big-proofs", "Proof functions with long bodies.").param("max_lines", 200)
    }
    fn check(&self, cx: &Cx, out: &mut Findings) -> anyhow::Result<()> {
        let max = cx.params.get_u64("max_lines")?;
        for f in cx.facts.functions() {
            if u64::from(f.body_lines) > max {
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

`verus_lint::run` is the whole command line, so the crate's binary accepts the
same arguments. `verus-lint check` and `run` build the crate (`cargo build
--release`; set `rust_profile = "dev"` to use another profile) and start its
binary with the same arguments, so SQL rules, Rust rules, levels, baselines and
output all go through one report. `Facts` gives typed `functions` and `uses`,
`roots`, `live` (reachable over the full dependency graph), `graph(keep)` with
`reachable`, `sccs`, `callers` and `callees`, and read-only `query(sql)` for the
rest. A complete example is in `examples/rust-rules/`.

## Baselines and levels

A rule's level decides what its findings do:

| Level | Effect |
| --- | --- |
| `off` | not run |
| `note` | printed as notes; never changes the exit status |
| `warn` (default) | printed at the rule's severity; never changes the exit status |
| `gate` | findings the baseline does not cover make the run exit 1 |

The baseline file records what a project accepts today. `check
--update-baseline` writes it from the current findings of the rules that ran
(only rules with a `ratchet` header have entries); commit the file. Then:

- `ratchet: set`: a finding is covered when its `entity` is in the file's set.
- `ratchet: metric, ratio = r, abs = a`: covered when the entity is in the file
  and its `metric` is at most `max(r * base, base + a)`. A new entity is never
  covered.
- Entities in the file that a rule no longer reports are listed as fixed. They
  stay in the file until you update it, so the baseline only shrinks on purpose.

## Output and exit status

`--format text` (default) prints `file:line: severity rule: message [entity]`
for findings not covered by the baseline (at most `--top N` per rule, 0 for
all) and a summary per rule. `--format json` writes `meta`, `rules`,
`findings` (each with `baselined`), `skipped` and `gate_failures` in a stable
field order. `--format sarif` writes SARIF 2.1.0 with one run, the rules as
descriptors and each finding's entity as `partialFingerprints.entity` so
code-scanning UIs track findings across line moves. `--output FILE` writes the
report to a file instead of stdout.

| Exit status | Meaning |
| --- | --- |
| 0 | no gated finding outside the baseline (or `--update-baseline`) |
| 1 | at least one gated finding the baseline does not cover |
| 2 | rule, config or runtime error (bad SQL, bad config, failed build) |
| 3 | unsupported Verus version |

## CI usage

Extraction runs Verus's front end and takes minutes on a large workspace
(4 minutes for 12 crates and 10,000 functions), so cache `.verus-lint/` keyed
on the sources or run extraction in the job that already builds Verus. A
typical step:

```sh
verus-lint run --format sarif --output verus-lint.sarif   # exits 1 on new gated findings
```

Upload `verus-lint.sarif` with GitHub's `upload-sarif` action to get
annotations. After deliberately accepting a change, run
`verus-lint check --update-baseline` and commit the baseline file.

## Development

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The fixture crate under `tests/fixtures/crate` is built with a real Verus; see
`tests/fixtures/regen.sh`. Licensed under MIT.
