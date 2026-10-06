-- id: verus/hotspot-growth
-- summary: A function's rlimit grew past its baseline.
-- severity: note
-- schema: ^1.3
-- needs: dynamic
-- params: floor = 100000
-- ratchet: metric, ratio = 1.3, abs = 200000
-- Metric ratchet: the baseline records each reported function's rlimit, and a later run is
-- covered while it stays within max(1.3 * base, base + 200000). Only functions at or above
-- `floor` are reported, so the baseline holds the expensive functions and a new function is
-- uncovered once it crosses `floor`. Gate it with `[rules.levels] "verus/hotspot-growth" = "gate"`.
SELECT v.friendly AS entity, f.file, f.line, v.rlimit AS metric,
       format('rlimit {}', v.rlimit) AS message
FROM verify_latest v LEFT JOIN functions f USING (fn_id)
WHERE v.rlimit >= param('floor')
ORDER BY v.rlimit DESC;
