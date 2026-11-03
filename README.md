# verus-lint

Lint and analysis for [Verus](https://github.com/verus-lang/verus) codebases.

verus-lint runs Verus's front end on each verified crate of a Cargo workspace,
reads the VIR log (`--log vir`), loads functions, uses, quantifiers, trusted
items and, optionally, per-function verification cost into an embedded DuckDB
database, and runs rules against it. The rules are yours: SQL files or a small
Rust crate in your repository. The tool ships mechanism only, no rules;
`examples/` has sample rules to copy. Findings can gate CI through ratcheting
baselines, and reports come out as text, JSON or SARIF.

## Quick start

```sh
cargo install verus-lint --version 0.2.0 --locked   # Rust 1.88+; Verus releases below
verus-lint extract                                  # crates need [package.metadata.verus] verify = true
verus-lint query "SELECT mode, count(*) FROM functions GROUP BY mode"
verus-lint check --rules lints/sql                  # run your SQL rules
```

Supported Verus releases (each maps to a log-format adapter; see
[docs/verus-versions.md](docs/verus-versions.md)):

| Verus release | Commit |
| --- | --- |
| `0.2026.07.18.3a4d30b` | `3a4d30bcdc4571e7927af97be9c4664973083eda` |
| `0.2026.08.30.b432e82` | `b432e82fed7e05090fd53b5e5fc39020f725aabe` |

Verus in a container: set `[extract] toolchain = ["./verify"]` to a wrapper
script that runs its arguments in the image with the workspace mounted at the
same path ([docs/getting-started.md](docs/getting-started.md#2-prepare-the-workspace)).

## Documentation

| Page | Contents |
| --- | --- |
| [docs/getting-started.md](docs/getting-started.md) | Install, first run, SQL and Rust rules, baselines, SARIF, CI |
| [docs/schema.md](docs/schema.md) | Every table and view: columns, types, what fills them, example queries |
| [docs/sql-rules.md](docs/sql-rules.md) | SQL rule header and result columns |
| [docs/api.md](docs/api.md) | Rust SDK for rules (`Rule`, `Facts`, `Graph`, `Finding`, `run`) |
| [docs/config.md](docs/config.md) | Every `verus-lint.toml` key |
| [docs/verus-versions.md](docs/verus-versions.md) | Supported Verus releases, adapters, adding a release |
| [DESIGN.md](DESIGN.md) | Design notes and measurements |
| [examples/](examples/README.md) | Sample SQL rules and a Rust rules crate |

## For AI coding agents

`.agents/skills/verus-lint/SKILL.md` is an agent skill: when to use verus-lint,
where the docs are, and how to write, test and baseline rules. Claude Code finds
it through the `.claude/skills` symlink; other agents can read it from
`.agents/skills/`.

## Development

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo install cargo-strata --version 0.4.0 --locked && cargo strata check
```

`cargo strata check` enforces the module layering in `strata.toml`: only the
dispatch module `adapters` names a Verus-release adapter, adapters do not
reference each other, and the database, rules and reporting modules see only
the normalized facts. The fixture crates under `tests/fixtures/crate` and
`tests/fixtures/report/crate` are built with a real Verus, one log per
supported release; see `tests/fixtures/regen.sh` and
[docs/verus-versions.md](docs/verus-versions.md). `tests/docs_schema.rs` fails
when `docs/schema.md` and `src/schema.sql` disagree. Licensed under MIT.
