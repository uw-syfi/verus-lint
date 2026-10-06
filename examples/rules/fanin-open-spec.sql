-- id: verus/fanin-open-spec
-- summary: Open spec functions named by many functions outside their module.
-- severity: note
-- schema: ^1.0
-- params: min_fns = 20
-- ratchet: metric, ratio = 1.2, abs = 5
-- Verus unfolds an open spec body wherever the function is named, so each
-- naming function outside the defining module depends on the body.
SELECT d.friendly                            AS entity,
       d.file, d.line,
       count(DISTINCT c.fn_id)               AS metric,
       count(DISTINCT c.module)              AS mods,
       count(DISTINCT c.crate)               AS crates,
       d.body_lines                          AS body,
       format('{} functions in {} modules name this open definition',
              count(DISTINCT c.fn_id), count(DISTINCT c.module)) AS message
FROM open_spec d
JOIN edges u     ON u.callee_id = d.fn_id AND u.kind IN ('call', 'resolved_impl', 'fn_value')
JOIN functions c ON c.fn_id = u.caller_id
WHERE c.module <> d.module
GROUP BY d.friendly, d.file, d.line, d.body_lines
HAVING count(DISTINCT c.fn_id) >= param('min_fns')
ORDER BY metric DESC;
