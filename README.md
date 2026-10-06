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
cargo install verus-lint --version 0.1.1 --locked   # Rust 1.88+; needs Verus 0.2026.07.18.3a4d30b
verus-lint extract                                  # crates need [package.metadata.verus] verify = true
verus-lint query "SELECT mode, count(*) FROM functions GROUP BY mode"
verus-lint check --rules lints/sql                  # run your SQL rules
```

## Documentation

| Page | Contents |
| --- | --- |
| [docs/getting-started.md](docs/getting-started.md) | Install, first run, SQL and Rust rules, baselines, SARIF, CI |
| [docs/schema.md](docs/schema.md) | Every table and view: columns, types, what fills them, example queries |
| [docs/sql-rules.md](docs/sql-rules.md) | SQL rule header and result columns |
| [docs/api.md](docs/api.md) | Rust SDK for rules (`Rule`, `Facts`, `Graph`, `Finding`, `run`) |
| [docs/config.md](docs/config.md) | Every `verus-lint.toml` key |
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
```

The fixture crate under `tests/fixtures/crate` is built with a real Verus; see
`tests/fixtures/regen.sh`. `tests/docs_schema.rs` fails when `docs/schema.md`
and `src/schema.sql` disagree. Licensed under MIT.
