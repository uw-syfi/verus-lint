---
name: verus-lint
description: Write, run and maintain lint rules for Verus codebases with verus-lint (SQL or Rust rules over facts from Verus's VIR log). Use when asked to find dead proof code, unused public API, quantifiers without triggers, trusted assumptions, expensive proofs or other structural properties of a Verus project, to add a project-specific check, or to set up a baseline or CI gate for such checks.
---

# verus-lint

verus-lint extracts facts about a Verus workspace (functions, what uses what,
quantifiers, trusted items, per-function verification cost) into a DuckDB
database and runs rules over it. It ships no rules: the rules are SQL files or a
small Rust crate in the user's own repository.

Use it when the question is structural and answerable from those facts. It is
not a Rust linter (use clippy) and it does not prove anything (use Verus).

## Docs

Read these instead of guessing. They are in `docs/` of the verus-lint repository
(also on GitHub: uw-syfi/verus-lint).

- `docs/getting-started.md`: install, extract, first rule, baselines, SARIF, CI.
- `docs/schema.md`: every table and view with columns, types, which Verus
  construct fills it, and an example query. Start here when writing a rule.
- `docs/api.md`: the Rust SDK (`Rule`, `Facts`, `Graph`, `Finding`, `run`).
- `docs/config.md`: every `verus-lint.toml` key.
- `examples/rules/*.sql` and `examples/rust-rules/`: complete rules to copy from.
  Copy them into the user's repository; do not point the config at the examples.

## How to work

1. Check the setup. `verus-lint --version`; the workspace needs `verus-lint.toml`
   and crates marked `[package.metadata.verus] verify = true`. Only one Verus
   release is accepted (see `docs/getting-started.md`); exit status 3 means the
   wrong one.
2. Extract once (`verus-lint extract`, minutes on a large workspace), then
   iterate with `verus-lint check`, which takes seconds. Re-extract only after
   the source or the `[roots]` config changes.
3. Query the facts before writing a rule. `verus-lint query "SELECT ..."` against
   the tables in `docs/schema.md` tells you what the data looks like on this
   codebase and what a rule would report. Read `warnings` too: code behind
   disabled `cfg(feature)` is missing from the facts, and a function used only
   there looks dead.
4. Pick the tool. SQL for set questions (counts, filters, joins, fan-in). The Rust
   SDK for graph questions (cycles, reachability with a custom edge filter) and
   per-function logic. Put the entity of each finding as a stable key (a function
   path, not a line number).
5. Test every rule on a fixture: a small crate or log with one positive case the
   rule must report and one negative case it must not. A rule that reports
   nothing on real code proves nothing until it reports on a known bad case.
6. Keep rules in the user's own repository (for example `lints/sql/` and
   `lints/`), never in verus-lint's `examples/`.
7. Set levels in `verus-lint.toml`: new rules start at `warn` or `note`; move to
   `gate` only after the baseline records the current findings.
8. Use baselines as ratchets that only shrink. `verus-lint check
   --update-baseline` after fixing findings, never to hide a new finding you have
   not looked at. Every rule that gates should have a `ratchet` header (`set` for
   findings, `metric, ratio = r, abs = a` for numbers).
9. Read results from `--format json` or `--format sarif --output FILE`, not by
   parsing the text output. Exit status: 0 clean, 1 gated findings not covered
   by the baseline, 2 error, 3 unsupported Verus.

## Pitfalls

- `opaque` is true for proof and exec functions in Verus's data. For spec
  functions use `mode = 'spec'` or the `open_spec` view.
- `uses.callee_id` is null for callees outside the extracted crates (vstd, other
  workspaces) and for broadcast groups. Join on `callee_path` when that matters.
- Dead-code rules depend on `[roots]`: patterns, `name_files` and pins decide
  what is live by definition. Look at `SELECT reason, count(*) FROM roots GROUP
  BY reason` before trusting a dead-code result.
- `Facts::query` returns text. Parse numbers yourself, or use the typed rows.
