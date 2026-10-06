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
