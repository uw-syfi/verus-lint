# Spike: Verus VIR log as the fact source

Toolchain: Verus 0.2026.07.18.3a4d30b (commit 3a4d30bcdc4571e7927af97be9c4664973083eda),
the pinned container used by Coral (`coral/verify`). Source: llm-eq at
`origin/claude/coral-prov-flip` (9dbd7524d). Host: 64 cores, shared, load about 10.

Flags: `cargo verus build -p <crate> --fwd-verus-args-to roots -- --log vir
--log impl-names --log-dir <dir> --time-expanded --output-json`. `--log vir`
writes `<dir>/crate.vir`; the file name does not include the crate name, so
every crate needs its own `--log-dir` (forwarding the flag to all crates of a
build into one directory would overwrite).

## Measurements

| Crate | Source lines | Verified fns | `crate.vir` | Verus total, log / no log | VIR phase, log / no log | Parse (prototype) |
| --- | --- | --- | --- | --- | --- | --- |
| coral-spec | 81,918 | 2,619 | 148 MB, 1.54 M lines | 62.6 s / 61.6 s | 16.6 s / 12.6 s | 2.8 to 3.5 s |
| coral-effects | 9,136 | 114 | 16.0 MB | 7.3 s (log) | n/a | 0.27 s |

- Logging costs about 4 s of VIR-phase time on coral-spec (writing 148 MB);
  end-to-end Verus time moved 1.8%, within the SMT-time noise of a shared host.
- `--log vir-option=no_type` shrinks the coral-effects log from 16.0 MB to
  12.1 MB (-24%) but drops the `self` parameter types needed for the JSON join
  (answer e). Keep types.
- The log holds the merged crate: the crate's own items plus the imported
  items it reaches (coral-effects log: 486 coral_effects functions, 255
  coral_spec, 222 vstd). Imported items are pruned and are not authoritative;
  each crate's own log is authoritative for its own functions.
- Prototype (`spike/vir-proto`, no dependencies, single thread): coral-spec
  yields 4,409 own functions (546 exec, 1,300 proof, 2,563 spec) and 64,229
  reference edges. Throughput is about 50 MB/s; reading the file is 0.13 s.

Edge counts on coral-spec by (section, kind, inside trigger):

```
38943 body/call/false        4005 body/call/true       368 body/reveal/false
 8819 ensure/call/false       511 ensure/call/true     189 decrease/call/false
 4332 require/call/false      232 require/call/true     24 hide/hide/false
 6470 */resolved_impl/*  (trait-method calls with a statically resolved impl)
```

## (a) Calls are resolved to full paths

Yes. Every call target is a `(Fun :path ...)` with the crate-qualified path.
Impl methods use Verus's internal impl names (`impl&%N`), not the type name.

```
(> Call :target (CallTarget Fun (CallTargetKind Static) (Fun :path coral_spec::ops::plugin_ok) () () ...)
  :args ((... (> Call :target (CallTarget Fun (CallTargetKind Static)
      (Fun :path coral_spec::ops::kinds::gelu_tanh_mul::gelu_tanh_mul_plugin) ...
```

Trait-method calls carry both the trait method and, when known, the resolved
impl: `(CallTargetKind DynamicResolved :resolved (Fun :path ...) ...)`.

## (b) Use context is distinguishable, except module-level `broadcast use`

Function fields are separate: `:require`, `:ensure`, `:returns`, `:decrease`,
`:decrease_by`, `:body`, and `:attrs ... :hidden` (`hide`). Inside an
expression, reveals are `Fuel` nodes whose third field is `is_broadcast_use`;
triggers are `Trigger` unary ops or `WithTriggers` wrappers:

```
(> Fuel (Fun :path coral_spec::core::plan::launch::run_bindings) 2 false)
(> Unary (UnaryOp Trigger (TriggerAnnotation Trigger None)) (... (> Call ... coral_spec::checker::aten::graph::atom_reads
(> WithTriggers :triggers (((... (> Call :target ... (Fun :path vstd::set::Set::contains)
```

Gap: a module-level `broadcast use` (all 14 in coral-spec, for example
`checker/teq_pattern.rs:21: broadcast use vstd::std_specs::hash::group_hash_axioms;`)
does not appear. The printer (`vir/src/printer.rs`, `write_krate`) writes
modules as ids only and drops `ModuleX::reveals`:

```
(module_id coral_spec::checker::teq_pattern)
(group_id vstd::std_specs::hash::group_hash_axioms)
```

Function-local `broadcast use` does appear (as `Fuel ... true`).
Quantifiers: 1,383 in coral-spec, 42 with no `#[trigger]` or `#![trigger]`
(Verus printed 12 "automatically chose triggers" notes; the gap is to be
classified: `#![auto]` and nested quantifiers).

## (c) Mode, opacity, visibility, attributes and spans are present

```
(@ "coral/crates/framework/coral-spec/src/ops/kinds/gelu_tanh_mul.rs:30:1: 30:38 (#0)" (Function
  :name (Fun :path coral_spec::ops::kinds::gelu_tanh_mul::lemma_gelu_tanh_mul_ok) :proxy None :kind (FunctionKind Static)
  :visibility (Visibility :restricted_to None)
  :body_visibility (BodyVisibility Visibility (Visibility :restricted_to coral_spec::ops::kinds::gelu_tanh_mul))
  :opaqueness (Opaqueness Opaque) :owning_module coral_spec::ops::kinds::gelu_tanh_mul :mode Proof ...
  :attrs (FunctionAttrs ... :hidden () :broadcast_forall false ... :spinoff_prover false ... :rlimit None ...
          :is_external_body false ...)
```

Spans are workspace-relative for the crate's own items and absolute for
vstd (`/cargo/git/checkouts/verus-.../3a4d30b/source/vstd/...`). Every
expression carries a span (`@@`), so each edge has a source location.
`assume`/`admit` are `(> AssertAssume :is_assume true ...)` (16 in coral-spec).
Not in the log: `#[verifier::external]` items (only `external_fn` ids) and
trait declarations (ids only).

## (d) Version: not in the log, available from the toolchain and the JSON

The log has no version header. Both `verus --version --output-json` and the
`--output-json` report of every run carry it:

```
"verus": {"profile": "release", "version": "0.2026.07.18.3a4d30b", "platform": {...},
          "toolchain": "1.96.0-x86_64-unknown-linux-gnu",
          "commit": "3a4d30bcdc4571e7927af97be9c4664973083eda"}
```

## (e) Joining rlimit and time by name works after a name mapping

The JSON names functions by their Rust-friendly path; the log uses impl ids:

```
{"function": "coral_spec::rewrite::controls::lemma_qkv_inserted_causal", "mode:": "proof",
 "time": 49, "time-micros": 49978, "rlimit": 147988, "success": true}
log: coral_effects::operations::impl&%22::put   JSON: coral_effects::operations::OperationsCap::put
```

Direct match: 1,960 of 2,006 coral-spec functions (97.7%). `--log impl-names`
maps trait impls to their self type:

```
coral_spec::checker::repr::impl&%1   ###   vstd::view::View   ###   coral_spec::checker::repr::ExecMask   ###   <span>
```

It does not cover inherent impls; for those the `self` parameter type, then
the return type, resolve most. Residual after all three: coral-effects 2 of 98
(`EncoderCap::new`, `CollectiveCap::new`); coral-spec 13 of 2,006 without the
impl-names log (trait impls, which that log covers). The JSON also gives
module session time and module rlimit (`smt-run-module-times`), and
`total-verify-module-times`.

## Additional findings

- `--no-verify --log vir` writes the same facts without SMT: coral-spec
  build 41.5 s versus 64 s for a full verify (cargo-reported, warm deps). The
  two logs hold the same 6,599 forms up to item order and internal
  `ReadKind` ids (147 forms differ only in those ids), so static extraction
  does not need a verification run, and facts must not key on log order or ids.
- Verus deletes the `--log-dir` directory if it exists and always names the
  file `crate.vir`; a multi-crate build needs one Verus invocation per crate
  (`-p <crate> --fwd-verus-args-to roots`, dependencies cached) or a driver
  wrapper that sets a per-crate directory.
- `cargo verus` already writes a bincode export per crate
  (`target/debug/deps/lib<crate>-<hash>.vir`, coral-spec 32 MB). It keeps
  modules with their `broadcast use` reveals, but is pruned for importers:
  only `pub` functions, no proof or exec bodies (`import_export.rs`,
  `export_crate`), and decoding it means linking the `vir` crate at the exact
  Verus commit (bincode is not self-describing). It cannot replace the log for
  dead-code analysis; it could supply module-level `broadcast use`.
- `vir` depends only on ordinary crates (air, im, indexmap, num-bigint, serde,
  sha2, sise), so linking it is possible, at the cost of one build per Verus
  commit.
