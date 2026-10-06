-- id: verus/seed-instability
-- summary: Functions whose rlimit varies widely across solver seeds, or that fail under one.
-- severity: warning
-- schema: ^1.3
-- needs: dynamic
-- params: ratio = 3, floor = 100000
-- ratchet: set
-- Run `verify --seeds 1,2,3` first: the rule needs two or more runs of a function. A proof
-- whose cost depends on the seed breaks when unrelated edits move the solver's choices.
-- Functions below `floor` rlimit in every run are ignored (their ratio is noise).
SELECT w.friendly AS entity, f.file, f.line, w.max_rlimit AS metric,
       CASE WHEN NOT w.all_ok THEN 'error' ELSE 'warning' END AS severity,
       CASE WHEN NOT w.all_ok THEN format('fails under some seed ({} runs)', w.n_runs)
            ELSE format('rlimit {} to {} across {} runs ({}x)', w.min_rlimit, w.max_rlimit, w.n_runs,
                        round(w.max_rlimit / greatest(w.min_rlimit, 1.0), 1)) END AS message
FROM verify_worst w LEFT JOIN functions f USING (fn_id)
WHERE w.n_runs >= 2
  AND (NOT w.all_ok
       OR (w.max_rlimit >= param('floor') AND w.max_rlimit > param('ratio') * greatest(w.min_rlimit, 1)))
ORDER BY w.max_rlimit DESC;
