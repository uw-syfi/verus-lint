-- id: verus/spinoff-candidate
-- summary: Expensive functions that do not run in their own solver session.
-- severity: warning
-- schema: ^1.3
-- needs: dynamic
-- params: threshold = 3000000
-- ratchet: set
-- `#[verifier::spinoff_prover]` gives a function a separate solver process: its context stays
-- small and it runs in parallel with the rest of its module. A function above `threshold`
-- rlimit without the attribute slows its whole module's session.
SELECT v.friendly AS entity, f.file, f.line, v.rlimit AS metric,
       format('rlimit {} without spinoff_prover', v.rlimit) AS message
FROM verify_latest v JOIN functions f USING (fn_id)
WHERE v.rlimit > param('threshold') AND NOT f.spinoff_prover
ORDER BY v.rlimit DESC;
