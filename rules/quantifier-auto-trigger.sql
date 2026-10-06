-- id: verus/quantifier-auto-trigger
-- summary: Quantifiers with no explicit trigger; Verus picks triggers automatically.
-- severity: warning
-- schema: ^1.1
-- ratchet: set
-- A quantifier counts as explicit when it carries `#[trigger]` or `#![trigger ..]`.
-- `#![auto]` and `#![all_triggers]` are an explicit request and are not reported.
SELECT f.path                     AS entity,
       f.file,
       min(q.line)                AS line,
       count(*)                   AS metric,
       format('{} quantifier(s) with no explicit trigger in {} fn {}',
              count(*), f.mode, f.friendly) AS message
FROM quantifiers q
JOIN functions f ON f.fn_id = q.fn_id
WHERE q."trigger" = 'none'
GROUP BY f.path, f.file, f.mode, f.friendly
ORDER BY metric DESC, entity;
