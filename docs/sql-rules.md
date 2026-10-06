# SQL rule format

A SQL rule is one `.sql` file: a comment header and one `SELECT` (or `WITH ...
SELECT`) over the tables in [schema.md](schema.md). Rules live in a directory
listed under `[rules] dirs` (or passed with `--rules DIR`). The runner opens the
database read-only.

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

## Header

Each header line is `-- key: value`, one line per key, with no trailing comment.
Other comment lines in the file are ignored.

| Key | Required | Meaning |
| --- | --- | --- |
| `id` | yes | Unique id, `namespace/name`. Levels, parameters and baselines are keyed by it. |
| `summary` | yes | One line, shown in the report. |
| `severity` | no | `note`, `warning` (default) or `error`. What output shows; whether findings fail the run is the level in the config. |
| `params` | no | Numeric parameters with defaults: `name = value, name2 = value`. Read in SQL as `param('name')`. |
| `ratchet` | no | `set`, or `metric, ratio = r, abs = a`. How findings are compared with the baseline. Without it the rule has no baseline entries and every finding counts. |
| `needs` | no | `dynamic`: the rule reads `verify_*` tables and is skipped with a note until `verus-lint verify` has run. |
| `schema` | no | Schema version range the rule was written against, for example `^1.1`. Informational. |

## Result columns

| Column | Required | Meaning |
| --- | --- | --- |
| `entity` | yes | Stable key of the finding, usually a function path. Never a line number or message text: the baseline and SARIF matching use it. |
| `message` | yes | One line. |
| `file`, `line` | no | Location for text and SARIF output. |
| `metric` | no | Number compared by metric ratchets. |
| `severity` | no | Per-row override of the header severity. |
| any other | no | Kept as a property of the finding in JSON and SARIF. |

## Parameters

`param('name')` is replaced by the numeric value before the query runs. The value
is the header default, overridden by `[rules.params."<id>"]` in the config,
overridden by `--param name=value`.

## Reachability helpers

`roots`, `graph_edges`, `live_nodes` and `dead_scc` support dead-code rules; see
the Reachability section of [schema.md](schema.md). `examples/rules/dead-proof-code.sql`
is a complete rule using them.
