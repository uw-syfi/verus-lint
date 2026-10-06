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
    has_default BOOLEAN, trait_method VARCHAR, type_invariant BOOLEAN
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
