-- id: verus/rlimit-headroom
-- summary: Functions that use a large share of the per-function rlimit budget.
-- severity: warning
-- schema: ^1.3
-- needs: dynamic
-- params: budget = 10000000, warn_pct = 50, fail_pct = 80
-- ratchet: set
-- Verus fails a function that exceeds its rlimit (10,000,000 by default, or the value of its
-- `#[verifier::rlimit]` attribute), so a function near the limit breaks on the next small
-- change. Rows at `fail_pct` or more are errors. Reads each crate's default run.
SELECT v.friendly AS entity, f.file, f.line, v.rlimit AS metric,
       CASE WHEN v.rlimit * 100 >= param('fail_pct') * param('budget') THEN 'error' ELSE 'warning' END AS severity,
       format('rlimit {} is {}% of the {} budget', v.rlimit,
              round(100.0 * v.rlimit / param('budget'), 1), CAST(param('budget') AS BIGINT)) AS message
FROM verify_latest v LEFT JOIN functions f USING (fn_id)
WHERE v.rlimit * 100 >= param('warn_pct') * param('budget')
ORDER BY v.rlimit DESC;
