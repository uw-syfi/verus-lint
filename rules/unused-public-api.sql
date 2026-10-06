-- id: verus/unused-public-api
-- summary: Pinned public proof and spec functions that nothing in the workspace uses.
-- severity: note
-- schema: ^1.1
-- ratchet: set
-- Pin files list the public API; its items are not roots, so one that no root reaches
-- is unused by the workspace itself, though it may be meant for outside users.
SELECT DISTINCT f.path AS entity, f.file, f.line,
       f.body_lines AS metric,
       format('pinned {} fn {} is unreachable from every root', f.mode, f.friendly) AS message
FROM api_pins p
JOIN functions f ON f.fn_id = p.fn_id
WHERE f.mode IN ('proof', 'spec')
  AND f.path NOT IN (SELECT path FROM live_nodes)
ORDER BY f.file, f.line;
