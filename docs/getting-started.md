# Getting started

verus-lint reads the facts Verus already computes about your crate (functions,
what uses what, quantifiers, trusted items, verification cost), loads them into
a DuckDB database, and runs rules over it. The rules are yours: SQL files or a
small Rust crate in your repository. This page takes you from install to a rule
that fails CI.

Every command below was run end to end. The static-facts steps (install through
CI) ran against `tests/fixtures/crate` of this repository, a small Verus crate
named `fx` with 37 functions. The `verify` step ran against a three-function
crate that verifies, shown in that step. Verus ran in a Docker container behind
a `./verify` wrapper script, which is what the `toolchain` setting is for.

## 1. Install

```sh
cargo install verus-lint --version 0.1.1 --locked
verus-lint --version        # verus-lint 0.1.1
```

Needs Rust 1.88 or newer. The first build compiles a bundled DuckDB and takes a
few minutes.

You also need Verus with `cargo verus`. verus-lint accepts exactly one Verus
release, because the VIR log it reads (Verus's internal representation of your
code) has no stability promise:

| Verus release | Commit |
| --- | --- |
| `0.2026.07.18.3a4d30b` | `3a4d30bcdc4571e7927af97be9c4664973083eda` |

Before each extraction the tool runs `verus --version --output-json` and
compares the commit. Any other release exits with status 3:

```text
error: unsupported Verus 0.2026.01.01.abcdef0 (commit abcdef0000000000000000000000000000000000); supported: 0.2026.07.18.3a4d30b (3a4d30bcdc4571e7927af97be9c4664973083eda)
```

If your crate pins Verus as a git dependency (as the fixture does for `vstd`),
use that commit.

## 2. Prepare the workspace

The workspace is a Cargo workspace (or one crate) whose Verus crates carry this
in their `Cargo.toml`:

```toml
[package.metadata.verus]
verify = true
```

verus-lint extracts every such member. If Verus is not on your `PATH`, or lives
in a container, put a wrapper script next to your workspace. It receives the
command to run as arguments. A Docker version:

```sh
#!/usr/bin/env bash
# ./verify: run a command inside the container that has Verus, workspace mounted at the same path.
set -euo pipefail
exec docker run --rm --user "$(id -u):$(id -g)" -v "$PWD:$PWD" -w "$PWD" \
  <your-image-with-verus> "$@"
```

(The run used the same script with two more `-v` options for a persistent cargo
registry and target directory, so rebuilds are fast.)

Create `verus-lint.toml` in the workspace root:

```toml
[extract]
toolchain = ["./verify"]   # omit if `cargo verus` runs directly

[roots]
patterns = ["theorem_*"]   # functions that are live even though nothing calls them
```

Add `.verus-lint/` (the database and logs) to `.gitignore`. The full list of
keys is in [config.md](config.md).

## 3. Extract the facts

```sh
verus-lint extract
```

```text
extract fx: cargo clean -p fx
extract fx: running Verus --no-verify
extract fx: 37 functions, 27 uses
extracted 1 crates, 37 functions, 27 uses; logs 0.1 MB; verus 2.5s, parse+load 0.1s; .verus-lint/facts.duckdb
```

For each crate this runs

```sh
./verify cargo verus build -p fx --fwd-verus-args-to roots -- \
  --no-verify --log vir --log impl-names --log-dir .verus-lint/cache/fx/log
```

so the VIR log is `.verus-lint/cache/fx/log/crate.vir`. `--no-verify` skips the
SMT solver: on a large crate, 41.5 s instead of 64 s for a full verify, with
identical facts. The tool runs
`cargo clean -p <crate>` first, because cargo would otherwise treat the crate as
fresh and Verus would write no log. `extract --reuse-logs` parses logs from an
earlier run without calling Verus.

Look at the data:

```sh
verus-lint query "SELECT mode, count(*) AS n FROM functions GROUP BY mode ORDER BY mode"
```

```text
mode	n
exec	3
proof	12
spec	22
```

`query` runs one read-only SQL statement and prints tab-separated rows. The
tables and views are in [schema.md](schema.md); `functions`, `uses`, `roots`,
`quantifiers` and `trusted` cover most rules.

## 4. Add a SQL rule

A rule is one `.sql` file: a comment header and one `SELECT`. Save this as
`lints/sql/proof-needs-ensures.sql`:

```sql
-- id: my/proof-needs-ensures
-- summary: Proof functions with a body and no ensures clause.
-- severity: warning
-- ratchet: set
SELECT path AS entity, file, line,
       format('proof fn {} has no ensures clause', friendly) AS message
FROM functions
WHERE mode = 'proof' AND has_body AND n_ensures = 0 AND NOT generated;
```

`entity` and `message` are required result columns. The entity is the stable key
of a finding (a function path, never a line number), because baselines match on
it. `file` and `line` are optional locations; any other column becomes a
property in JSON and SARIF output.

```sh
verus-lint check --rules lints/sql
```

```text
src/lib.rs:29: warning my/proof-needs-ensures: proof fn fx::t::lemma_assume has no ensures clause [fx::t::lemma_assume]
src/lib.rs:30: warning my/proof-needs-ensures: proof fn fx::t::lemma_admit has no ensures clause [fx::t::lemma_admit]
...
my/proof-needs-ensures [warn]: 8 new, 0 baselined, 0 fixed: Proof functions with a body and no ensures clause.
```

(Six more finding lines are elided.) `check` reads the database and takes
seconds; `extract` is the slow step. To avoid repeating the flag, list the
directory in `verus-lint.toml`:

```toml
[rules]
dirs = ["lints/sql"]
```

Use SQL for questions about sets of functions: counts, joins, filters, fan-in.
Header keys and result columns are in [sql-rules.md](sql-rules.md);
`examples/rules/` has thirteen complete rules to copy.

## 5. Add a Rust rule

Use Rust when the question is about the graph or needs per-function logic.
This rule reports proof functions that call each other in a cycle, which SQL
expresses badly. Create a small crate, by convention `lints/`:

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

[workspace]   # the crate sits inside another directory tree
```

```rust
// lints/src/main.rs
use verus_lint::rules::Finding;
use verus_lint::sdk::{Cx, Findings, Mode, Ratchet, Rule, RuleMeta, Severity, UseKind};

/// Proof functions that call each other in a cycle.
struct ProofCycles;

impl Rule for ProofCycles {
    fn meta(&self) -> RuleMeta {
        RuleMeta::new("my/proof-cycles", "Proof functions that form a call cycle.")
            .severity(Severity::Warning)
            .param("min_size", 2)
            .ratchet(Ratchet::Set)
    }

    fn check(&self, cx: &Cx, out: &mut Findings) -> anyhow::Result<()> {
        let min = cx.params.get_u64("min_size")?;
        let graph = cx.facts.graph(|u| u.kind == UseKind::Call);
        for scc in graph.sccs() {
            if (scc.len() as u64) < min {
                continue;
            }
            let fns: Vec<_> = scc.iter().map(|&id| cx.facts.function(id)).collect();
            if fns.iter().all(|f| f.mode == Mode::Proof) {
                // Entity: the smallest path in the cycle, so it is stable across edits.
                let first = fns.iter().min_by_key(|f| &f.path).unwrap();
                let names: Vec<_> = fns.iter().map(|f| f.name.as_str()).collect();
                out.push(Finding::at(
                    first,
                    format!("{} proof functions call each other: {}", fns.len(), names.join(", ")),
                ));
            }
        }
        Ok(())
    }
}

fn main() -> std::process::ExitCode {
    verus_lint::run(&[&ProofCycles])
}
```

Name the crate in the config:

```toml
[rules]
dirs = ["lints/sql"]
rust = "lints"
```

```sh
verus-lint check
```

```text
building rules crate .../lints/Cargo.toml
    Finished `release` profile [optimized] target(s) in 0.45s
...
src/lib.rs:80: warning my/proof-cycles: 2 proof functions call each other: lemma_dead_a, lemma_dead_b [fx::live::lemma_dead_a]

my/proof-needs-ensures [warn]: 8 new, 0 baselined, 0 fixed: Proof functions with a body and no ensures clause.
my/proof-cycles [warn]: 1 new, 0 baselined, 0 fixed: Proof functions that form a call cycle.
```

`check` and `run` build the rules crate (`cargo build --release`) and start its
binary with the same arguments. That binary loads the SQL rules too, so both
kinds of rule share levels, baselines and output. The SDK is described in
[api.md](api.md).

## 6. Baselines and levels: the ratchet

New rules usually find existing problems. A baseline records what the project
accepts today so that only new findings fail the build. First choose which
rules may fail the build, with levels:

```toml
[rules.levels]
"my/proof-needs-ensures" = "gate"
"my/proof-cycles" = "gate"
```

`gate` findings the baseline does not cover make `check` exit with status 1;
`warn` (the default) and `note` print but never fail. Now:

```sh
verus-lint check                     # exit 1: 9 gated findings, none accepted yet
verus-lint check --update-baseline   # writes verus-lint-baseline.json, exit 0
verus-lint check                     # exit 0: "0 new, 8 baselined", "0 new, 1 baselined"
```

Commit `verus-lint-baseline.json`. The ratchet header in each rule decides how a
finding is matched: `set` covers a finding whose entity is in the file;
`metric, ratio = r, abs = a` covers it while its metric stays within
`max(r * base, base + a)`. Entities the rule no longer reports are listed as
fixed, but stay in the file until you update it, so the baseline only shrinks
on purpose. Details: [config.md](config.md#baseline).

To see the gate work, add a proof function with no `ensures` to the crate and
run `verus-lint run` (extract, then check):

```text
extracted 1 crates, 38 functions, 28 uses; ...
src/lib.rs:79: warning my/proof-needs-ensures: proof fn fx::live::lemma_new has no ensures clause [fx::live::lemma_new]

my/proof-needs-ensures [gate]: 1 new, 8 baselined, 0 fixed: Proof functions with a body and no ensures clause.
my/proof-cycles [gate]: 0 new, 1 baselined, 0 fixed: Proof functions that form a call cycle.
FAILED: 1 gated finding(s) not covered by the baseline
```

The exit status was 1.

## 7. Verification cost (dynamic facts)

`verify` runs Verus's verifier per crate and loads per-function cost. It needs
a crate that verifies; the fixture has a deliberately recursive proof without a
`decreases` clause, so this step used a small crate named `tiny` instead:

```rust
use vstd::prelude::*;

verus! {

pub open spec fn double(x: int) -> int { 2 * x }

pub proof fn lemma_double_add(a: int, b: int)
    ensures double(a) + double(b) == double(a + b)
{}

pub fn add_one(x: u32) -> (r: u32)
    requires x < 100
    ensures r == x + 1
{
    x + 1
}

} // verus!
```

```sh
verus-lint extract
verus-lint verify --seeds 1,2
verus-lint query "SELECT friendly, rlimit, time_us, success, seed FROM verify_fn ORDER BY 1, 5"
```

```text
verified 2 runs, 4 function rows (0 unjoined); verus 4.9s
friendly	rlimit	time_us	success	seed
tiny::add_one	736	459	true	1
tiny::add_one	730	490	true	2
tiny::lemma_double_add	1408	836	true	1
tiny::lemma_double_add	1408	794	true	2
```

`rlimit` is Verus's resource count, a deterministic cost; `time_us` is SMT time
and varies. Running with several seeds shows which proofs depend on the solver's
random seed (`verify_worst`). Rules that read these tables carry the header
`-- needs: dynamic` and are skipped with a note until a `verify` run exists.
`verify` modifies the database `extract` made, so run `extract` first.

## 8. Machine-readable output and CI

Read results from JSON or SARIF, not from the text format:

```sh
verus-lint check --format sarif --output verus-lint.sarif
```

The SARIF file (2.1.0) has one run, the rules as descriptors, and each finding's
entity in `partialFingerprints.entity`, so code-scanning tools follow findings
across line moves. `--format json` gives `meta`, `rules`, `findings` (each with a
`baselined` flag), `skipped` and `gate_failures`.

| Exit status | Meaning |
| --- | --- |
| 0 | no gated finding outside the baseline (or `--update-baseline`) |
| 1 | at least one gated finding the baseline does not cover |
| 2 | rule, config or runtime error (bad SQL, bad config, failed build) |
| 3 | unsupported Verus version |

A GitHub Actions job (template; the commands in it are the ones above):

```yaml
- uses: actions/cache@v4
  with:
    path: .verus-lint
    key: verus-lint-${{ hashFiles('**/*.rs', '**/Cargo.lock') }}
- run: verus-lint run --format sarif --output verus-lint.sarif   # exits 1 on new gated findings
- if: always()
  uses: github/codeql-action/upload-sarif@v3
  with:
    sarif_file: verus-lint.sarif
```

Extraction runs Verus's front end and takes minutes on a large workspace (about
4 minutes for 12 crates and 10,000 functions), so cache `.verus-lint/` or run
extraction in the job that already builds Verus. After deliberately accepting a
change, run `verus-lint check --update-baseline` and commit the baseline.

## Next

- [schema.md](schema.md): every table, view and column.
- [sql-rules.md](sql-rules.md): the SQL rule header and result columns.
- [config.md](config.md): every config key.
- [api.md](api.md): the Rust SDK.
- `examples/`: complete SQL and Rust rules to copy into your own repository.
- AI coding agents: the skill in `.agents/skills/verus-lint/SKILL.md`.
