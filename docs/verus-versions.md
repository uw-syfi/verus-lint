# Verus versions

Verus's VIR log and `--output-json` report have no stability promise and no
format version, so verus-lint accepts only the Verus releases it has been
tested against. Several releases are supported side by side.

## Supported releases

| Verus release | Commit | Adapter |
| --- | --- | --- |
| `0.2026.07.18.3a4d30b` | `3a4d30bcdc4571e7927af97be9c4664973083eda` | `v2026_07` |
| `0.2026.08.30.b432e82` | `b432e82fed7e05090fd53b5e5fc39020f725aabe` | `v2026_07` |

`verus-lint extract` runs `verus --version --output-json` (through the
`[extract] toolchain` wrapper, if set) and looks the commit up in this table.
`verify` and `ingest` read the `verus` section of each report the same way. Any
other release exits with status 3 and names itself and the supported list:

```text
error: unsupported Verus 0.2026.01.01.abcdef0 (commit abcdef0000000000000000000000000000000000); supported: 0.2026.07.18.3a4d30b (3a4d30bcdc4571e7927af97be9c4664973083eda), 0.2026.08.30.b432e82 (b432e82fed7e05090fd53b5e5fc39020f725aabe)
```

A database records the release it was extracted with (`meta.verus_version`,
`meta.verus_commit`), and `verify` refuses a report from another release.

## How releases are kept apart

- `src/version.rs` detects the release (from `verus --version` or a report).
- `src/adapters/mod.rs` is the one dispatch point: the table above, as
  `adapters::SUPPORTED`, maps each release to an adapter.
- An adapter (`src/adapters/<name>/`) parses one format generation: the VIR
  log, the `--log impl-names` output and the report. Consecutive releases that
  print the same format share an adapter. The module comment of each adapter
  lists what its format looks like and how it differs from the previous one.
- An adapter produces the normalized rows of `src/facts.rs`. The database,
  schema, rules, baselines and reports see only those rows, so a rule never
  depends on the Verus release.
- `strata.toml`, checked by `cargo strata check` in CI, enforces this: only
  `adapters` names an adapter, adapters never reference each other and read
  only the fact model and parsing helpers, and the database, rules and
  reporting modules never reference `adapters` or the parsing helpers.

A fact that a release cannot provide is stored as NULL, and
[schema.md](schema.md) says so for the column. No column is release-dependent
today: both supported releases provide every fact.

Differences between the two supported releases that show up in the facts,
because the source meaning changed rather than the format:

- 0.2026.08.30 gives each compiler-generated field accessor (`arrow_*`,
  `functions.generated`) one `requires` clause, so `n_requires` is 1 there
  and 0 under 0.2026.07.18.

## Adding a Verus release

1. Get a Docker image with the release's `verus`, `cargo-verus` and Rust
   toolchain on `PATH`.
2. Generate its fixtures:

   ```sh
   tests/fixtures/regen.sh <version> <image> '<vstd dependency as TOML>'
   # for example
   tests/fixtures/regen.sh 0.2026.08.30.b432e82 my-verus:0.2026.08.30 '"=0.0.0-2026-08-30-0159"'
   ```

   This writes `tests/fixtures/fx/<version>/` (VIR log and impl names of
   `tests/fixtures/crate`) and `tests/fixtures/report/<version>.json` (a report
   of `tests/fixtures/report/crate`).
3. Add the release to `FIXTURES` in `tests/fixture_crate.rs` and to
   `adapters::SUPPORTED`, first with the newest existing adapter.
4. Run `cargo test`. If the fixture tests pass, the format did not change in
   any way the parser reads: keep the shared adapter and note the release in
   its module comment. If they fail, or the parser rejects the log, copy the
   newest adapter to `src/adapters/<new name>/`, change the copy until the
   tests pass, and point the new release at it. Do not branch on the release
   inside an adapter, and do not make one adapter call another.
5. If the release cannot provide a fact, leave it NULL in that adapter's rows
   and say so in [schema.md](schema.md) and in the list above.
6. Update the table on this page, the README, `CHANGELOG.md`, then run
   `cargo strata check`, `cargo clippy --workspace --all-targets -- -D warnings`
   and `cargo test --workspace`.
