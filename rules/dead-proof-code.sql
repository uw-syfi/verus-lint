-- id: verus/dead-proof-code
-- summary: Proof and spec functions unreachable from every root.
-- severity: warning
-- schema: ^1.1
-- ratchet: set
-- Reachability is over resolved uses (all sections, triggers and reveals included), module
-- and group `broadcast use`, group membership and trait-method dispatch; see the `roots` and
-- `live_nodes` views. A function in a dead cycle shares `scc` with the rest of the cycle.
-- Items named in an API pin file are reported by verus/unused-public-api instead.
SELECT f.path AS entity, f.file, f.line,
       coalesce(s.scc_id, f.fn_id) AS scc,
       coalesce(s.scc_size, 1)     AS scc_size,
       f.body_lines                AS metric,
       CASE WHEN coalesce(s.scc_size, 1) > 1
            THEN format('{} fn {} is unreachable from every root (dead cycle of {} functions)',
                        f.mode, f.friendly, s.scc_size)
            ELSE format('{} fn {} is unreachable from every root', f.mode, f.friendly) END AS message
FROM functions f
LEFT JOIN dead_scc s ON s.fn_id = f.fn_id
WHERE f.mode IN ('proof', 'spec')
  AND f.path NOT IN (SELECT path FROM live_nodes)
  AND f.fn_id NOT IN (SELECT fn_id FROM api_pins WHERE fn_id IS NOT NULL)
ORDER BY f.file, f.line;
