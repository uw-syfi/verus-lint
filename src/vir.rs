//! VIR log to facts: one crate's `crate.vir` becomes function and use rows.
//!
//! Only the crate's own items are kept: the log also holds pruned copies of
//! imported items, and each crate's own log is authoritative for its items.
//! The parser fails closed: a form it interprets that does not have the
//! expected shape is an error naming the function and span.

use crate::sexp::{Node, Reader, fields, fun_path};
use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct FunctionRow {
    pub path: String,
    pub friendly: String,
    pub krate: String,
    pub module: String,
    pub name: String,
    pub self_type: Option<String>,
    pub trait_path: Option<String>,
    pub mode: String,
    pub kind: String,
    pub item_kind: String,
    pub vis: String,
    pub body_vis: String,
    pub opaque: bool,
    pub reveal_vis: Option<String>,
    pub external_body: bool,
    pub broadcast_forall: bool,
    pub broadcast_forall_only: bool,
    pub rlimit_attr: Option<String>,
    pub spinoff_prover: bool,
    pub integer_ring: bool,
    pub bit_vector: bool,
    pub nonlinear: bool,
    pub has_body: bool,
    pub file: String,
    pub line: u32,
    pub end_line: u32,
    pub body_lines: u32,
    pub n_requires: u32,
    pub n_ensures: u32,
}

#[derive(Debug, Clone)]
pub struct UseRow {
    /// Index into `CrateFacts::functions`.
    pub caller: usize,
    pub callee_path: String,
    pub section: &'static str,
    pub kind: &'static str,
    pub in_trigger: bool,
    pub fuel: Option<String>,
    pub file: String,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Default)]
pub struct CrateFacts {
    pub krate: String,
    pub functions: Vec<FunctionRow>,
    pub uses: Vec<UseRow>,
    pub modules: Vec<String>,
    /// Broadcast group paths owned by this crate (`(group_id ..)` forms).
    pub groups: Vec<String>,
    pub forms: usize,
}

/// Map from impl path to (trait path, self type), from `--log impl-names`.
pub type ImplNames = HashMap<String, (String, String)>;

pub fn parse_impl_names(text: &str) -> ImplNames {
    let mut m = HashMap::new();
    for line in text.lines() {
        let p: Vec<&str> = line.split("   ###   ").collect();
        if p.len() >= 3 {
            m.insert(
                p[0].trim().to_string(),
                (p[1].trim().to_string(), p[2].trim().to_string()),
            );
        }
    }
    m
}

/// `file:l:c: l2:c2 (#n)` split into (file, line, col, end_line).
pub fn parse_span(s: &str) -> Option<(String, u32, u32, u32)> {
    let s = s.rsplit_once(" (#").map(|x| x.0).unwrap_or(s);
    let (start, end) = s.rsplit_once(": ")?;
    let end_line = end.split(':').next()?.parse().ok()?;
    let mut it = start.rsplitn(3, ':');
    let col = it.next()?.parse().ok()?;
    let line = it.next()?.parse().ok()?;
    let file = it.next()?.to_string();
    Some((file, line, col, end_line))
}

struct RawUse<'a> {
    callee: String,
    section: &'static str,
    kind: &'static str,
    in_trigger: bool,
    fuel: Option<String>,
    span: &'a str,
}

struct Ctx<'a> {
    span: &'a str,
}

fn walk<'a>(
    n: &Node<'a>,
    section: &'static str,
    kind: &'static str,
    in_trigger: bool,
    cx: &mut Ctx<'a>,
    out: &mut Vec<RawUse<'a>>,
    fuel: Option<String>,
) -> Result<(), String> {
    let Some(v) = n.list() else { return Ok(()) };
    if let Some(p) = fun_path(n) {
        out.push(RawUse {
            callee: p.to_string(),
            section,
            kind,
            in_trigger,
            fuel,
            span: cx.span,
        });
        return Ok(());
    }
    let (mut kind, mut in_trigger, mut fuel) = (kind, in_trigger, fuel);
    let saved = cx.span;
    match v.first().and_then(|h| h.atom()) {
        Some("@@") | Some("@") => {
            if let Some(Node::Str(s)) = v.get(1) {
                cx.span = s;
            }
        }
        Some(">") => match v.get(1).and_then(|h| h.atom()) {
            Some("Call") => kind = "call",
            Some("Fuel") => {
                // (> Fuel (Fun :path P) fuel is_broadcast_use)
                let (Some(f), Some(b)) = (
                    v.get(3).and_then(|x| x.atom()),
                    v.get(4).and_then(|x| x.atom()),
                ) else {
                    return Err(format!("malformed Fuel form at {}", cx.span));
                };
                kind = if b == "true" {
                    "broadcast_use"
                } else {
                    "reveal"
                };
                fuel = Some(f.to_string());
            }
            Some("ExecFnByName") => kind = "fn_value",
            Some("WithTriggers") => {
                // :triggers are trigger terms; the body is not.
                let f = fields(&v[2..]);
                if let Some(t) = f.get("triggers") {
                    walk(t, section, kind, true, cx, out, fuel.clone())?;
                }
                if let Some(b) = f.get("body") {
                    walk(b, section, kind, in_trigger, cx, out, fuel)?;
                }
                cx.span = saved;
                return Ok(());
            }
            Some("Unary") => {
                if let Some(op) = v.get(2).and_then(|x| x.list())
                    && op.first().and_then(|x| x.atom()) == Some("UnaryOp")
                    && op.get(1).and_then(|x| x.atom()) == Some("Trigger")
                {
                    in_trigger = true;
                }
            }
            _ => kind = "other",
        },
        Some("Typ") => kind = "type",
        Some("CallTargetKind") => kind = "resolved_impl",
        _ => {}
    }
    for c in v {
        walk(c, section, kind, in_trigger, cx, out, fuel.clone())?;
    }
    cx.span = saved;
    Ok(())
}

fn flag(a: &std::collections::HashMap<&str, &Node>, k: &str) -> bool {
    a.get(k).and_then(|n| n.atom()) == Some("true")
}

fn vis_of(n: Option<&Node>) -> String {
    // (Visibility :restricted_to None | <module path>)
    let Some(v) = n.and_then(|n| n.list()) else {
        return "pub".into();
    };
    match v.get(2).and_then(|x| x.atom()) {
        Some("None") | None => "pub".into(),
        Some(m) => m.to_string(),
    }
}

fn count_exprs(n: Option<&&Node>) -> u32 {
    match n.and_then(|n| n.list()) {
        None => 0,
        Some(v) if v.first().and_then(|x| x.atom()) == Some("tuple") => v[1..]
            .iter()
            .map(|x| x.list().map_or(0, |l| l.len() as u32))
            .sum(),
        Some(v) => v.len() as u32,
    }
}

/// `self` parameter type path, when the function has one.
fn self_param_type(params: Option<&&Node>) -> Option<String> {
    for p in params?.list()? {
        let pn = match p.head() {
            Some("@") => p.list()?.get(2)?,
            _ => p,
        };
        if pn.head() != Some("Param") {
            continue;
        }
        let f = fields(&pn.list()?[1..]);
        let is_self = f
            .get("name")
            .and_then(|n| n.list())
            .and_then(|v| v.get(1))
            .map(|x| matches!(x, Node::Str("self")))
            .unwrap_or(false);
        if !is_self {
            continue;
        }
        return typ_path(f.get("typ")?);
    }
    None
}

/// `(Typ Datatype (Dt Path P) ..)`, possibly under `Decorate`, gives `P`.
fn typ_path(t: &Node) -> Option<String> {
    let v = t.list()?;
    if v.first()?.atom() == Some("Typ") {
        match v.get(1)?.atom()? {
            "Datatype" => {
                let dt = v.get(2)?.list()?;
                return Some(dt.get(2)?.atom()?.to_string());
            }
            "Decorate" => return v.iter().skip(2).find_map(typ_path),
            _ => return None,
        }
    }
    None
}

fn parse_function(
    top: &Node,
    span: &str,
    krate: &str,
    names: &ImplNames,
    facts: &mut CrateFacts,
) -> Result<(), String> {
    let v = top.list().unwrap();
    let f = fields(&v[1..]);
    let ctx = |what: &str| format!("{span}: function form: {what}");
    let path = f
        .get("name")
        .and_then(|n| fun_path(n))
        .ok_or_else(|| ctx("missing :name (Fun :path ..)"))?;
    let module = f
        .get("owning_module")
        .and_then(|n| n.atom())
        .ok_or_else(|| ctx("missing :owning_module"))?;
    if module.split("::").next() != Some(krate) {
        return Ok(()); // imported copy
    }
    let mode = f
        .get("mode")
        .and_then(|n| n.atom())
        .ok_or_else(|| ctx("missing :mode"))?
        .to_lowercase();
    if !matches!(mode.as_str(), "exec" | "proof" | "spec") {
        return Err(ctx(&format!("unknown mode {mode}")));
    }
    let kind_list = f
        .get("kind")
        .and_then(|n| n.list())
        .ok_or_else(|| ctx("missing :kind"))?;
    let kf = fields(&kind_list[1..]);
    let (kind, trait_path) = match kind_list.get(1).and_then(|x| x.atom()) {
        Some("Static") => ("static", None),
        Some("TraitMethodDecl") => ("trait_decl", kf.get("trait_path").and_then(|n| n.atom())),
        Some("TraitMethodImpl") => ("trait_impl", kf.get("trait_path").and_then(|n| n.atom())),
        Some("ForeignTraitMethodImpl") => (
            "foreign_trait_impl",
            kf.get("trait_path").and_then(|n| n.atom()),
        ),
        other => return Err(ctx(&format!("unknown FunctionKind {other:?}"))),
    };
    let impl_path = kf.get("impl_path").and_then(|n| n.atom());
    let item_kind = match f
        .get("item_kind")
        .and_then(|n| n.list())
        .and_then(|l| l.get(1))
        .and_then(|x| x.atom())
    {
        Some("Function") => "function",
        Some("Const") => "const",
        Some("Static") => "static",
        other => return Err(ctx(&format!("unknown ItemKind {other:?}"))),
    };
    let (opaque, reveal_vis) = match f.get("opaqueness").and_then(|n| n.list()) {
        Some(l) if l.get(1).and_then(|x| x.atom()) == Some("Opaque") => (true, None),
        Some(l) if l.get(1).and_then(|x| x.atom()) == Some("Revealed") => (
            false,
            Some(vis_of(fields(&l[2..]).get("visibility").copied())),
        ),
        other => {
            return Err(ctx(&format!(
                "unknown :opaqueness {:?}",
                other.map(|l| l.len())
            )));
        }
    };
    let body_vis = match f.get("body_visibility").and_then(|n| n.list()) {
        Some(l) if l.get(1).and_then(|x| x.atom()) == Some("Visibility") => vis_of(l.get(2)),
        Some(l) if l.get(1).and_then(|x| x.atom()) == Some("Uninterpreted") => "none".into(),
        _ => return Err(ctx("unknown :body_visibility")),
    };
    let a = f
        .get("attrs")
        .and_then(|n| n.list())
        .map(|l| fields(&l[1..]))
        .ok_or_else(|| ctx("missing :attrs"))?;
    let has_body = f
        .get("body")
        .map(|n| n.atom() != Some("None"))
        .unwrap_or(false);
    let (file, line, _col, header_end) = parse_span(span).ok_or_else(|| ctx("unparsable span"))?;
    // The function's own span covers only its header; the body expression has its own span.
    let body_end = f
        .get("body")
        .and_then(|b| b.list())
        .filter(|l| l.first().and_then(|x| x.atom()) == Some("@@"))
        .and_then(|l| match l.get(1) {
            Some(Node::Str(s)) => parse_span(s).map(|x| x.3),
            _ => None,
        });
    let end_line = body_end.unwrap_or(header_end).max(header_end);
    let name = path.rsplit("::").next().unwrap_or(path).to_string();
    let self_type = match (kind, impl_path) {
        ("trait_impl", Some(ip)) => names.get(ip).map(|x| x.1.clone()),
        _ => None,
    }
    .or_else(|| {
        if path.contains("::impl&%") {
            self_param_type(f.get("params"))
        } else {
            None
        }
    });
    let friendly = match (&self_type, path.find("::impl&%")) {
        (Some(t), Some(_)) => format!("{t}::{name}"),
        _ => path.to_string(),
    };
    let rlimit_attr = a.get("rlimit").and_then(|n| match n {
        Node::Atom("None") => None,
        n => Some(crate::sexp::render(n)),
    });
    let row = FunctionRow {
        path: path.to_string(),
        friendly,
        krate: krate.to_string(),
        module: module.to_string(),
        name,
        self_type,
        trait_path: trait_path.map(String::from),
        mode,
        kind: kind.into(),
        item_kind: item_kind.into(),
        vis: vis_of(f.get("visibility").copied()),
        body_vis,
        opaque,
        reveal_vis,
        external_body: flag(&a, "is_external_body"),
        broadcast_forall: flag(&a, "broadcast_forall"),
        broadcast_forall_only: flag(&a, "broadcast_forall_only"),
        rlimit_attr,
        spinoff_prover: flag(&a, "spinoff_prover"),
        integer_ring: flag(&a, "integer_ring"),
        bit_vector: flag(&a, "bit_vector"),
        nonlinear: flag(&a, "nonlinear"),
        has_body,
        file,
        line,
        end_line,
        body_lines: end_line.saturating_sub(line) + 1,
        n_requires: count_exprs(f.get("require")),
        n_ensures: count_exprs(f.get("ensure")),
    };
    let caller = facts.functions.len();
    let mut edges = Vec::new();
    let mut cx = Ctx { span };
    for key in [
        "require",
        "ensure",
        "returns",
        "decrease",
        "decrease_by",
        "body",
    ] {
        if let Some(n) = f.get(key) {
            let section = match key {
                "require" => "require",
                "ensure" => "ensure",
                "returns" => "returns",
                "decrease" => "decrease",
                "decrease_by" => "decrease_by",
                _ => "body",
            };
            walk(n, section, "other", false, &mut cx, &mut edges, None)
                .map_err(|e| format!("{path}: {e}"))?;
        }
    }
    if let Some(h) = a.get("hidden") {
        walk(h, "hide", "hide", false, &mut cx, &mut edges, None)?;
    }
    for RawUse {
        callee,
        section,
        kind,
        in_trigger,
        fuel,
        span: sp,
    } in edges
    {
        if kind == "type" {
            continue;
        }
        let (file, line, col, _) =
            parse_span(sp).unwrap_or_else(|| (row.file.clone(), row.line, 0, row.line));
        facts.uses.push(UseRow {
            caller,
            callee_path: callee,
            section,
            kind,
            in_trigger,
            fuel,
            file,
            line,
            col,
        });
    }
    facts.functions.push(row);
    Ok(())
}

/// Parse the text of one crate's `crate.vir`.
pub fn parse_log(text: &str, krate: &str, names: &ImplNames) -> Result<CrateFacts, String> {
    let mut facts = CrateFacts {
        krate: krate.to_string(),
        ..Default::default()
    };
    let mut r = Reader::new(text);
    while let Some(form) = r.next_form()? {
        facts.forms += 1;
        let Some(v) = form.list() else { continue };
        match v.first().and_then(|x| x.atom()) {
            Some("module_id") => {
                if let Some(m) = v.get(1).and_then(|x| x.atom())
                    && m.split("::").next() == Some(krate)
                {
                    facts.modules.push(m.to_string());
                }
            }
            Some("group_id") => {
                if let Some(m) = v.get(1).and_then(|x| x.atom())
                    && m.split("::").next() == Some(krate)
                {
                    facts.groups.push(m.to_string());
                }
            }
            Some("@") => {
                let (Some(Node::Str(span)), Some(top)) = (v.get(1), v.get(2)) else {
                    continue;
                };
                if top.head() == Some("Function") {
                    parse_function(top, span, krate, names, &mut facts)?;
                }
            }
            _ => {}
        }
    }
    Ok(facts)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/mini.vir");

    #[test]
    fn span_parsing() {
        assert_eq!(
            parse_span("a/b.rs:30:1: 35:2 (#0)"),
            Some(("a/b.rs".into(), 30, 1, 35))
        );
        assert_eq!(parse_span("garbage"), None);
    }

    #[test]
    fn mini_fixture_functions() {
        let f = parse_log(FIXTURE, "mini", &ImplNames::new()).unwrap();
        let by: HashMap<_, _> = f.functions.iter().map(|x| (x.path.as_str(), x)).collect();
        assert_eq!(f.functions.len(), 4, "imported vstd copy is dropped");
        let a = by["mini::m::open_a"];
        assert_eq!(
            (
                a.mode.as_str(),
                a.vis.as_str(),
                a.body_vis.as_str(),
                a.opaque
            ),
            ("spec", "pub", "pub", false)
        );
        assert_eq!(
            (a.file.as_str(), a.line, a.end_line, a.body_lines),
            ("src/m.rs", 3, 5, 3)
        );
        assert!(by["mini::m::opaque_b"].opaque);
        assert_eq!(by["mini::m::closed_c"].body_vis, "mini::m");
        assert_eq!(by["mini::n::lemma_u"].mode, "proof");
        // Header span is one line; the body span extends the function to line 16.
        assert_eq!(
            (by["mini::n::lemma_u"].line, by["mini::n::lemma_u"].end_line),
            (8, 16)
        );
        assert_eq!(by["mini::n::lemma_u"].n_requires, 1);
        assert_eq!(by["mini::n::lemma_u"].n_ensures, 1);
        assert_eq!(f.modules, vec!["mini", "mini::m", "mini::n"]);
        assert_eq!(f.groups, vec!["mini::m::group_g"]);
    }

    #[test]
    fn mini_fixture_uses() {
        let f = parse_log(FIXTURE, "mini", &ImplNames::new()).unwrap();
        let name = |u: &UseRow| f.functions[u.caller].name.clone();
        let find = |callee: &str| {
            f.uses
                .iter()
                .filter(|u| u.callee_path.ends_with(callee))
                .map(|u| (name(u), u.section, u.kind, u.in_trigger))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            find("open_a"),
            vec![
                ("lemma_u".to_string(), "ensure", "call", false),
                ("lemma_u".to_string(), "body", "call", false),
                ("lemma_u".to_string(), "body", "call", true)
            ]
        );
        assert_eq!(
            find("opaque_b"),
            vec![("lemma_u".to_string(), "body", "reveal", false)]
        );
        assert_eq!(
            find("group_g"),
            vec![("lemma_u".to_string(), "body", "broadcast_use", false)]
        );
        let u = f
            .uses
            .iter()
            .find(|u| u.callee_path.ends_with("opaque_b"))
            .unwrap();
        assert_eq!(
            (u.file.as_str(), u.line, u.col, u.fuel.as_deref()),
            ("src/n.rs", 12, 9, Some("1"))
        );
    }

    #[test]
    fn unknown_mode_fails_closed() {
        let bad = FIXTURE.replace(":mode Proof", ":mode Weird");
        let e = parse_log(&bad, "mini", &ImplNames::new()).unwrap_err();
        assert!(e.contains("unknown mode"), "{e}");
    }

    #[test]
    fn impl_names_parse() {
        let m = parse_impl_names(
            "c::m::impl&%1   ###   t::Tr   ###   c::m::Ty   ###   f.rs:1:1: 1:2 (#3)\n",
        );
        assert_eq!(
            m["c::m::impl&%1"],
            ("t::Tr".to_string(), "c::m::Ty".to_string())
        );
    }
}
