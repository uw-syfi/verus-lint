CREATE TABLE meta (key VARCHAR PRIMARY KEY, value VARCHAR);
CREATE TABLE crates (crate VARCHAR PRIMARY KEY, manifest VARCHAR, log_bytes BIGINT, fn_count BIGINT, verified BOOLEAN);
CREATE TABLE modules (module VARCHAR PRIMARY KEY, crate VARCHAR, file VARCHAR);
CREATE TABLE functions (
    fn_id BIGINT PRIMARY KEY, path VARCHAR, friendly VARCHAR, crate VARCHAR, module VARCHAR, name VARCHAR,
    self_type VARCHAR, trait_path VARCHAR, mode VARCHAR, kind VARCHAR, item_kind VARCHAR,
    vis VARCHAR, body_vis VARCHAR, opaque BOOLEAN, reveal_vis VARCHAR, external_body BOOLEAN,
    broadcast_forall BOOLEAN, broadcast_forall_only BOOLEAN, rlimit_attr VARCHAR, spinoff_prover BOOLEAN,
    integer_ring BOOLEAN, bit_vector BOOLEAN, nonlinear BOOLEAN, has_body BOOLEAN,
    file VARCHAR, line INTEGER, end_line INTEGER, body_lines INTEGER, n_requires INTEGER, n_ensures INTEGER,
    has_default BOOLEAN, trait_method VARCHAR, type_invariant BOOLEAN, generated BOOLEAN
);
CREATE TABLE uses (
    caller_id BIGINT, callee_path VARCHAR, callee_id BIGINT, section VARCHAR, kind VARCHAR,
    in_trigger BOOLEAN, fuel VARCHAR, file VARCHAR, line INTEGER, col INTEGER
);
CREATE TABLE module_uses (module VARCHAR, callee_path VARCHAR, callee_id BIGINT, kind VARCHAR, file VARCHAR, line INTEGER);
-- Broadcast groups defined by an extracted crate ((group_id ..) forms in the log).
CREATE TABLE broadcast_groups (path VARCHAR PRIMARY KEY, crate VARCHAR);
-- Members of `broadcast group` items, from a source scan (the log has group ids only).
CREATE TABLE group_members (group_path VARCHAR, member_path VARCHAR, member_id BIGINT, file VARCHAR, line INTEGER);
CREATE TABLE quantifiers (fn_id BIGINT, quant VARCHAR, "trigger" VARCHAR, n_triggers INTEGER, section VARCHAR, file VARCHAR, line INTEGER);
CREATE TABLE trusted (fn_id BIGINT, kind VARCHAR, file VARCHAR, line INTEGER, text VARCHAR);
CREATE TABLE trait_impls (impl_path VARCHAR PRIMARY KEY, trait_path VARCHAR, self_type VARCHAR, crate VARCHAR, file VARCHAR, line INTEGER);
-- Extraction problems a rule can refuse to trust (unresolved names, skipped files).
CREATE TABLE warnings (crate VARCHAR, what VARCHAR, detail VARCHAR);

-- One row per Verus verification run of one crate (`verify`). `seed` is null when the run set no
-- solver seed.
CREATE TABLE runs (run_id BIGINT PRIMARY KEY, crate VARCHAR, seed INTEGER, verus_args VARCHAR, source_commit VARCHAR, started_at VARCHAR, wall_s DOUBLE);
-- Per-function verification cost from a run's report. `fn_id` is null when the report name matched
-- no extracted function, or more than one. `rlimit` is Verus's resource count, `time_us` the SMT time.
CREATE TABLE verify_fn (run_id BIGINT, fn_id BIGINT, friendly VARCHAR, crate VARCHAR, module VARCHAR, mode VARCHAR, rlimit BIGINT, time_us BIGINT, success BOOLEAN, seed INTEGER);
-- Per-module cost. `rlimit` and `smt_time_ms` sum the module's own functions' queries;
-- `session_time_ms` is the longest solver session (the main one or a spinoff) of the module.
CREATE TABLE verify_module (run_id BIGINT, module VARCHAR, crate VARCHAR, rlimit BIGINT, smt_time_ms BIGINT, session_time_ms BIGINT);

-- The run a rule should read for each crate: the one without a seed if there is one, else the
-- lowest seed; the newest of those.
CREATE VIEW verify_default_runs AS
SELECT run_id FROM (
    SELECT run_id, row_number() OVER (PARTITION BY crate ORDER BY (seed IS NOT NULL), seed, run_id DESC) AS rn
    FROM runs) WHERE rn = 1;
CREATE VIEW verify_latest AS
SELECT v.* FROM verify_fn v JOIN verify_default_runs USING (run_id);
CREATE VIEW verify_module_latest AS
SELECT v.* FROM verify_module v JOIN verify_default_runs USING (run_id);
-- Cost of each function across all runs of its crate (all seeds).
CREATE VIEW verify_worst AS
SELECT crate, friendly, any_value(fn_id) AS fn_id, count(DISTINCT run_id) AS n_runs,
       max(rlimit) AS max_rlimit, min(rlimit) AS min_rlimit, bool_and(success) AS all_ok
FROM verify_fn GROUP BY crate, friendly;

-- Resolved uses, one per caller, callee and section. A statically resolved
-- trait-method call and a function value count as uses of the callee.
CREATE VIEW edges AS
SELECT DISTINCT caller_id, callee_id, section, kind FROM uses WHERE callee_id IS NOT NULL;

-- An open spec function: not opaque, and its body is visible beyond its own
-- module (`pub open`, or `open(in ancestor)` / `open(crate)`, which Verus
-- records as a body visibility restricted to an ancestor module).
CREATE VIEW open_spec AS
SELECT * FROM functions
WHERE mode = 'spec' AND item_kind = 'function' AND NOT opaque
  AND body_vis <> 'none' AND body_vis <> module;

-- Dead-code analysis inputs, stored by `extract` from the config. `like_pattern` is the glob
-- converted to SQL LIKE with `\` as the escape character. A pattern without `::` matches the
-- function's own name (`by_name`); one with `::` matches its path or friendly path.
CREATE TABLE root_patterns (pattern VARCHAR, like_pattern VARCHAR, by_name BOOLEAN);
-- API pin entries and the functions they name (fn_id null when nothing matched).
CREATE TABLE api_pins (entry VARCHAR, file VARCHAR, line INTEGER, fn_id BIGINT);
-- Function names found in the files listed under `[roots] name_files` (only names of functions
-- that exist), with the first file that mentions each.
CREATE TABLE root_names (name VARCHAR, file VARCHAR);
-- Strongly connected components of the dead subgraph (filled by `extract`); scc_id is the
-- smallest fn_id of the component.
CREATE TABLE dead_scc (fn_id BIGINT, scc_id BIGINT, scc_size INTEGER);

-- Functions that are live by definition: exec functions, functions matching a root pattern,
-- implementations of traits declared outside the extracted crates (callers may dispatch to
-- them generically), functions named in a `name_files` file, type invariants, and, when the config asks for it, every pub function.
-- API pin entries are not roots unless `[roots] pins_are_roots` is set. The log has no `#[cfg(test)]` items, so tests need no entry.
CREATE VIEW roots AS
SELECT fn_id, 'exec' AS reason FROM functions WHERE mode = 'exec'
UNION ALL
SELECT f.fn_id, 'pattern' FROM functions f JOIN root_patterns p
  ON CASE WHEN p.by_name THEN f.name LIKE p.like_pattern ESCAPE '\'
          ELSE f.path LIKE p.like_pattern ESCAPE '\' OR f.friendly LIKE p.like_pattern ESCAPE '\' END
UNION ALL
SELECT fn_id, 'name_file' FROM functions WHERE name IN (SELECT name FROM root_names)
UNION ALL
SELECT fn_id, 'foreign_trait_impl' FROM functions
WHERE kind IN ('trait_impl', 'foreign_trait_impl')
  AND (trait_method IS NULL OR trait_method NOT IN (SELECT path FROM functions WHERE kind = 'trait_decl'))
UNION ALL
SELECT fn_id, 'type_invariant' FROM functions WHERE type_invariant
UNION ALL
SELECT fn_id, 'pin' FROM api_pins
WHERE fn_id IS NOT NULL AND (SELECT coalesce(max(value), 'false') FROM meta WHERE key = 'roots_pins') = 'true'
UNION ALL
SELECT fn_id, 'public_api' FROM functions
WHERE vis = 'pub' AND (SELECT coalesce(max(value), 'false') FROM meta WHERE key = 'roots_public_api') = 'true';

-- Use graph over path nodes. A function node leads to its module node, which leads to the
-- module's `broadcast use` items; a group leads to its members; a trait method declaration
-- leads to every implementation of it.
CREATE VIEW graph_edges AS
SELECT f.path AS src, u.callee_path AS dst FROM uses u JOIN functions f ON f.fn_id = u.caller_id
UNION ALL SELECT path, 'mod:' || module FROM functions
UNION ALL SELECT 'mod:' || module, callee_path FROM module_uses
UNION ALL SELECT group_path, member_path FROM group_members
UNION ALL SELECT d.path, i.path FROM functions d JOIN functions i ON i.trait_method = d.path;

-- Nodes (function and group paths, `mod:` module nodes) reachable from the roots.
CREATE VIEW live_nodes AS
WITH RECURSIVE live(path) AS (
    SELECT f.path FROM roots r JOIN functions f USING (fn_id)
  UNION
    SELECT g.dst FROM live JOIN graph_edges g ON g.src = live.path
)
SELECT path FROM live;
