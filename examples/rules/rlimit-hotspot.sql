-- id: verus/rlimit-hotspot
-- summary: Functions and modules whose verification cost is above a ceiling.
-- severity: warning
-- schema: ^1.3
-- needs: dynamic
-- params: fn_ceiling = 1000000, module_rlimit = 40000000, module_session_ms = 8000
-- ratchet: set
-- A function over `fn_ceiling` rlimit is a candidate to split. Verus checks the functions of a
-- module in one solver session, and modules in parallel, so a module's own session time bounds
-- wall time however many cores are free; a module over `module_rlimit` (summed over its
-- functions) or `module_session_ms` (its longest session) is reported as `module:<name>`.
SELECT v.friendly AS entity, f.file, f.line, v.rlimit AS metric,
       format('rlimit {} is over the {} function ceiling', v.rlimit, CAST(param('fn_ceiling') AS BIGINT)) AS message
FROM verify_latest v LEFT JOIN functions f USING (fn_id)
WHERE v.rlimit > param('fn_ceiling')
UNION ALL
SELECT 'module:' || m.module, NULL, NULL, m.rlimit,
       format('module rlimit {} and longest session {} ms (limits {} and {})', m.rlimit, m.session_time_ms,
              CAST(param('module_rlimit') AS BIGINT), CAST(param('module_session_ms') AS BIGINT))
FROM verify_module_latest m
WHERE m.rlimit > param('module_rlimit') OR m.session_time_ms > param('module_session_ms')
ORDER BY metric DESC;
