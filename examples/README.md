# Example lints

verus-lint ships no rules. The SQL files in `rules/` are examples to copy into
your own repository and adapt; the tool never loads them unless you point it at
a directory (`verus-lint check --rules DIR`, or `[rules] dirs` in
`verus-lint.toml`). Keep your rules in your own tree and do not reference these.

| File | Shows |
| --- | --- |
| `dead-proof-code.sql` | Reachability from `roots` over `live_nodes`, grouped by `dead_scc` |
| `unused-public-api.sql` | The pinned-API category (`api_pins`) |
| `quantifier-auto-trigger.sql` | Reading `quantifiers` |
| `trusted-inventory.sql` | Reading `trusted` with a set ratchet |
| `trait-spec-default.sql` | Joining `trait_impls` and `functions.trait_method` |
| `fanin-open-spec.sql`, `fanin-reveal.sql` | Fan-in over `edges` with a metric ratchet |

The mechanisms these rules use (the `roots`, `graph_edges` and `live_nodes`
views, the `dead_scc` table, root patterns and pins in `[roots]`) are part of
the tool. An example Rust rules crate comes with the SDK (phase 4).
