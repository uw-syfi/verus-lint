-- id: verus/extraction-health
-- summary: Facts the extraction could not see or resolve; read these before trusting a count.
-- severity: note
-- ratchet: set
SELECT crate || ': ' || what || ': ' || detail AS entity,
       CASE what
         WHEN 'feature_gated_item' THEN 'item behind a cargo feature; absent from the log unless the build enabled it, so references from it are invisible'
         WHEN 'unresolved_broadcast_use' THEN 'module-level broadcast use that names no function or group of the crate'
         WHEN 'module_use_in_function_less_file' THEN 'module-level broadcast use in a file without functions; its module is unknown'
         ELSE what
       END AS message,
       split_part(detail, ':', 1) AS file,
       try_cast(split_part(detail, ':', 2) AS INTEGER) AS line,
       what AS kind
FROM warnings
ORDER BY crate, what, detail;
