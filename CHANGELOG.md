# Changelog

## 0.2.0

- Support Verus 0.2026.08.30.b432e82 beside 0.2026.07.18.3a4d30b. The release
  is detected from `verus --version --output-json` (and from each report's
  `verus` section) and mapped to a log-format adapter; both releases share the
  `v2026_07` adapter. Unknown releases still exit with status 3, naming the
  detected release and the supported list. See `docs/verus-versions.md`.
- Internal: the normalized fact model (`src/facts.rs`) is separate from the
  parsers; parsers live in `src/adapters/<generation>/`, reached only through
  `adapters`. `strata.toml` enforces that layering with cargo-strata 0.4.0, and
  CI runs `cargo strata check`.
- API: `extract::load_crate_logs` takes the adapter (`adapters::newest()` or
  an entry of `adapters::SUPPORTED`); `verify::ingest` and
  `verify::select_crate_report` take normalized reports (`verify::normalize`).
  `version::SUPPORTED`, `version::check` and `version::Unsupported` moved to
  `adapters` (`SUPPORTED`, `select`, `Unsupported`).
- Fix: consts, methods of derived trait impls and the accessor of a field
  shared by enum variants were reported as `external_fn` trusted items. Verus
  lists them in `(external_fn ..)`, but they are verified or compiler-generated.
- Fix: integer rule parameters are substituted as `BIGINT`, so
  `examples/rules/rlimit-headroom.sql` no longer fails with an INT32 overflow
  (`80 * 30000000`) once a function passes `warn_pct`.
- Fixtures: per-release VIR logs under `tests/fixtures/fx/<version>/`, a real
  0.2026.08.30 report under `tests/fixtures/report/`, and
  `tests/fixtures/regen.sh VERSION IMAGE VSTD` to regenerate them.
- Docs: supported-release table, Docker wrapper with a persistent cargo
  registry, and how to add a Verus release.

## 0.1.1

- Docs (getting started, schema, SQL rules, API, config), schema drift test,
  agent skill.

## 0.1.0

- First release.
