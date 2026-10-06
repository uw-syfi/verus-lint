-- id: verus/trait-spec-default
-- summary: Trait spec functions with a default body that implementations may override.
-- severity: note
-- schema: ^1.1
-- ratchet: set
-- An override changes what generic callers of the trait can rely on, so a default
-- body is only a stable fact for the implementations that do not override it.
SELECT d.path AS entity, d.file, d.line,
       count(DISTINCT i.fn_id) AS metric,
       (SELECT count(*) FROM trait_impls ti WHERE ti.trait_path = d.trait_path) AS impls,
       format('trait spec fn {} has a default body; {} of {} implementations override it',
              d.friendly, count(DISTINCT i.fn_id),
              (SELECT count(*) FROM trait_impls ti WHERE ti.trait_path = d.trait_path)) AS message
FROM functions d
LEFT JOIN functions i ON i.kind = 'trait_impl' AND i.trait_method = d.path
WHERE d.kind = 'trait_decl' AND d.mode = 'spec' AND d.has_default
GROUP BY d.path, d.file, d.line, d.trait_path, d.friendly
ORDER BY metric DESC, entity;
