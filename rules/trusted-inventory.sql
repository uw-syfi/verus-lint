-- id: verus/trusted-inventory
-- summary: Every assume, admit, external_body, external_fn, assume_specification and broadcast axiom.
-- severity: note
-- schema: ^1.1
-- ratchet: set
-- One finding per (item, kind); a set ratchet keeps the trusted surface from growing silently.
-- `#[verifier::external]` items are not in the VIR log and are not listed.
SELECT coalesce(f.path, t.text) || '#' || t.kind AS entity,
       min(t.file) AS file,
       min(t.line) AS line,
       count(*)    AS metric,
       t.kind      AS kind,
       format('{} x {} in {}', count(*), t.kind, coalesce(f.friendly, t.text)) AS message
FROM trusted t
LEFT JOIN functions f ON f.fn_id = t.fn_id
GROUP BY coalesce(f.path, t.text), t.kind, f.friendly, t.text
ORDER BY t.kind, entity;
