//! Parser and extraction tests against a real Verus log of `tests/fixtures/crate`.

use std::path::Path;
use verus_lint::db::Db;
use verus_lint::extract::load_crate_logs;

fn fixture_db() -> Db {
    let mut db = Db::in_memory().unwrap();
    let ws = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/crate");
    let vir = include_str!("fixtures/fx/crate.vir");
    let imp = include_str!("fixtures/fx/crate.impl_names");
    load_crate_logs(&mut db, &ws, "fx", "Cargo.toml", vir, imp).unwrap();
    db.resolve().unwrap();
    let roots = verus_lint::config::RootsCfg {
        patterns: vec!["theorem_*".into()],
        public_api: false,
        pins: vec!["pins/*.pin".into()],
    };
    verus_lint::analysis::store_roots(&db, &ws, &roots).unwrap();
    verus_lint::analysis::store_dead_sccs(&db).unwrap();
    db
}

fn rows(db: &Db, sql: &str) -> Vec<String> {
    db.query_rows(sql)
        .unwrap()
        .into_iter()
        .skip(1)
        .map(|r| r.join(" "))
        .collect()
}

#[test]
fn quantifier_triggers() {
    let db = fixture_db();
    let got = rows(
        &db,
        "SELECT f.name, q.quant, q.\"trigger\", q.n_triggers FROM quantifiers q JOIN functions f USING (fn_id)
         WHERE f.module = 'fx::q' ORDER BY f.line",
    );
    assert_eq!(
        got,
        [
            "inline_trig forall explicit 1",
            "block_trig forall explicit 2",
            "no_trig forall none 0",
            "auto_trig forall auto_annotation 0",
            "exists_q exists none 0",
        ]
    );
}

#[test]
fn trusted_inventory() {
    let db = fixture_db();
    let got = rows(
        &db,
        "SELECT t.kind, coalesce(f.name, '-') FROM trusted t LEFT JOIN functions f ON f.fn_id = t.fn_id ORDER BY 1, 2",
    );
    assert_eq!(
        got,
        [
            "admit lemma_admit",
            "assume lemma_assume",
            "assume_specification ext_id",
            "broadcast_axiom axiom_b",
            "external_body exec_external",
            "external_body lemma_external",
            "external_fn ext_id",
        ]
    );
}

#[test]
fn trait_facts() {
    let db = fixture_db();
    assert_eq!(
        rows(
            &db,
            "SELECT impl_path, trait_path, self_type FROM trait_impls ORDER BY impl_path"
        ),
        [
            "fx::tr::impl&%0 fx::tr::Shape fx::tr::Sq",
            "fx::tr::impl&%1 fx::tr::Shape fx::tr::Circ"
        ]
    );
    assert_eq!(
        rows(
            &db,
            "SELECT name FROM functions WHERE kind = 'trait_decl' AND has_default"
        ),
        ["twice"]
    );
    assert_eq!(
        rows(
            &db,
            "SELECT i.path, i.trait_method FROM functions i WHERE i.kind = 'trait_impl' AND i.name = 'twice'"
        ),
        ["fx::tr::impl&%1::twice fx::tr::Shape::twice"]
    );
}

#[test]
fn group_members_from_source_scan() {
    let db = fixture_db();
    assert_eq!(
        rows(
            &db,
            "SELECT group_path, member_path, member_id IS NOT NULL FROM group_members ORDER BY 1, 2"
        ),
        [
            "fx::live::group_live fx::live::lemma_in_group true",
            "fx::t::group_b fx::t::axiom_b true",
            "fx::t::group_b fx::t::proven_b true",
        ]
    );
}

fn run(db: &Db, id: &str) -> Vec<verus_lint::rules::Finding> {
    let rule = verus_lint::rules::builtin()
        .unwrap()
        .into_iter()
        .find(|r| r.id == id)
        .unwrap();
    verus_lint::rules::run_rule(&db.conn, &rule, &Default::default()).unwrap()
}

#[test]
fn quantifier_rule_reports_only_untriggered() {
    let db = fixture_db();
    let mut e: Vec<_> = run(&db, "verus/quantifier-auto-trigger")
        .into_iter()
        .map(|f| f.entity)
        .collect();
    e.sort();
    assert_eq!(e, ["fx::q::exists_q", "fx::q::no_trig"]);
}

#[test]
fn trusted_rule_lists_each_item_and_kind() {
    let db = fixture_db();
    let e: Vec<_> = run(&db, "verus/trusted-inventory")
        .into_iter()
        .map(|f| f.entity)
        .collect();
    assert_eq!(e.len(), 7);
    assert!(e.contains(&"fx::t::lemma_admit#admit".to_string()));
    assert!(e.contains(&"fx::t::axiom_b#broadcast_axiom".to_string()));
}

#[test]
fn trait_default_rule_counts_overrides() {
    let db = fixture_db();
    let f = run(&db, "verus/trait-spec-default");
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].entity, "fx::tr::Shape::twice");
    assert!(
        f[0].message.contains("1 of 2 implementations"),
        "{}",
        f[0].message
    );
}

#[test]
fn dead_proof_code_roots_groups_and_dispatch() {
    let db = fixture_db();
    let dead = run(&db, "verus/dead-proof-code");
    let names: Vec<&str> = dead.iter().map(|f| f.entity.as_str()).collect();
    for live in [
        "fx::live::theorem_top",
        "fx::live::lemma_used",
        "fx::q::p",
        // a function-local `broadcast use` of a group keeps its members live
        "fx::live::lemma_via_broadcast",
        "fx::live::lemma_in_group",
        // use_shape names the trait method; both implementations of it are live
        "fx::tr::Shape::twice",
        "fx::tr::impl&%1::twice",
        "fx::tr::Shape::area",
        "fx::tr::impl&%0::area",
    ] {
        assert!(!names.contains(&live), "{live} reported dead: {names:?}");
    }
    for d in [
        "fx::live::lemma_dead_a",
        "fx::live::lemma_dead_b",
        "fx::live::dead_spec",
        "fx::tr::Shape::required",
        "fx::tr::impl&%0::required",
        "fx::t::axiom_b",
    ] {
        assert!(names.contains(&d), "{d} not reported dead: {names:?}");
    }
    // The pinned function is unused API, not dead code.
    assert!(!names.contains(&"fx::live::lemma_dead_leaf"));
    let api: Vec<_> = run(&db, "verus/unused-public-api")
        .into_iter()
        .map(|f| f.entity)
        .collect();
    assert_eq!(api, ["fx::live::lemma_dead_leaf"]);
    // The mutually recursive pair is one component of size 2, reported once per member.
    let pair: Vec<_> = dead
        .iter()
        .filter(|f| f.entity.contains("lemma_dead_"))
        .collect();
    assert_eq!(pair.len(), 2);
    assert!(pair.iter().all(|f| f.message.contains("dead cycle of 2")));
    assert_eq!(pair[0].props["scc"], pair[1].props["scc"]);
}

#[test]
fn generated_accessors_are_not_dead_code() {
    let db = fixture_db();
    assert_eq!(
        rows(
            &db,
            "SELECT name FROM functions WHERE generated ORDER BY name"
        ),
        ["arrow_0", "arrow_A_0", "arrow_B_w", "arrow_w"]
    );
    let dead = run(&db, "verus/dead-proof-code");
    assert!(dead.iter().all(|f| !f.entity.contains("arrow_")));
}
