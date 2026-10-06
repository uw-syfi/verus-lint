-- id: verus/fanin-reveal
-- summary: Closed or opaque spec functions revealed by many functions outside their module.
-- severity: note
-- schema: ^1.0
-- params: min_fns = 10
-- ratchet: metric, ratio = 1.2, abs = 5
-- A reveal (reveal, reveal_with_fuel or function-local broadcast use) outside
-- the module is where the body of a closed or opaque definition is needed.
SELECT d.friendly                            AS entity,
       d.file, d.line,
       count(DISTINCT c.fn_id)               AS metric,
       count(DISTINCT c.module)              AS mods,
       count(DISTINCT c.crate)               AS crates,
       d.body_lines                          AS body,
       format('{} functions in {} modules reveal this {} definition',
              count(DISTINCT c.fn_id), count(DISTINCT c.module),
              CASE WHEN d.opaque THEN 'opaque' ELSE 'closed' END) AS message
FROM functions d
JOIN edges u     ON u.callee_id = d.fn_id AND u.kind IN ('reveal', 'broadcast_use')
JOIN functions c ON c.fn_id = u.caller_id
WHERE d.mode = 'spec' AND d.item_kind = 'function' AND (d.opaque OR d.body_vis IN ('none', d.module))
  AND c.module <> d.module
GROUP BY d.friendly, d.file, d.line, d.body_lines, d.opaque
HAVING count(DISTINCT c.fn_id) >= param('min_fns')
ORDER BY metric DESC;
