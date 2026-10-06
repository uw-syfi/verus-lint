//! VIR log to facts: one crate's `crate.vir` becomes function and use rows.
//!
//! Only the crate's own items are kept: the log also holds pruned copies of
//! imported items, and each crate's own log is authoritative for its items.
//! The parser fails closed: a form it interprets that does not have the
//! expected shape is an error naming the function and span.

use crate::num::to_u32;
use crate::sexp::{Node, Reader, fields, fun_path};
use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "mirrors the boolean columns of the functions table"
)]
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
    /// Trait method declaration with a default body.
    pub has_default: bool,
    /// For a trait method implementation: path of the declaration it implements.
    pub trait_method: Option<String>,
    /// `#[verifier::type_invariant]` function: used implicitly by the verifier.
    pub type_invariant: bool,
    /// Compiler-generated datatype field accessor (`arrow_Variant_0`): no source item to delete.
    pub generated: bool,
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

#[derive(Debug, Clone)]
pub struct QuantRow {
    pub caller: usize,
    /// forall, exists or choose.
    pub quant: &'static str,
    /// explicit (`#[trigger]` or `#![trigger ..]`), `auto_annotation` (`#![auto]`), or none.
    pub trigger: &'static str,
    pub n_triggers: u32,
    pub section: &'static str,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub struct TrustedRow {
    pub caller: Option<usize>,
    /// assume, admit, `external_body`, `external_fn`, `external_type`, `assume_specification`, `broadcast_axiom`.
    pub kind: &'static str,
    pub file: String,
    pub line: u32,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct TraitImplRow {
    pub impl_path: String,
    pub trait_path: String,
    pub self_type: String,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Default)]
pub struct CrateFacts {
    pub krate: String,
    pub functions: Vec<FunctionRow>,
    pub uses: Vec<UseRow>,
    pub modules: Vec<String>,
    /// Broadcast group paths owned by this crate (`(group_id ..)` forms).
    pub groups: Vec<String>,
    pub quants: Vec<QuantRow>,
    pub trusted: Vec<TrustedRow>,
    pub trait_impls: Vec<TraitImplRow>,
    /// Own-crate `external_fn` and `external_type` ids: (kind, path).
    pub externals: Vec<(&'static str, String)>,
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

/// Rows of `trait_impls` from `--log impl-names`, for impls owned by `krate`.
pub fn parse_impl_rows(text: &str, krate: &str) -> Vec<TraitImplRow> {
    let mut v = Vec::new();
    for line in text.lines() {
        let p: Vec<&str> = line.split("   ###   ").collect();
        if p.len() >= 4 && p[0].split("::").next() == Some(krate) {
            let (file, l, _, _) = parse_span(p[3].trim()).unwrap_or_default();
            v.push(TraitImplRow {
                impl_path: p[0].trim().to_string(),
                trait_path: p[1].trim().to_string(),
                self_type: impl_self_type(p[2]),
                file,
                line: l,
            });
        }
    }
    v
}

/// `file:l:c: l2:c2 (#n)` split into (file, line, col, `end_line`).
pub fn parse_span(s: &str) -> Option<(String, u32, u32, u32)> {
    let s = s.rsplit_once(" (#").map_or(s, |x| x.0);
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
    quants: Vec<(&'static str, &'static str, u32, &'static str, &'a str)>,
    assumes: Vec<(bool, &'a str)>,
}

/// Trigger classification of a quantifier's subtree. Nested quantifiers are not entered:
/// their annotations belong to them.
fn scan_triggers(n: &Node, groups: &mut Vec<String>, with: &mut u32, auto: &mut bool) {
    let Some(v) = n.list() else { return };
    if v.first().and_then(Node::atom) == Some(">") {
        match v.get(1).and_then(Node::atom) {
            Some("Quant" | "Choose") => return,
            Some("WithTriggers") => {
                let f = fields(&v[2..]);
                if let Some(t) = f.get("triggers").and_then(|t| t.list()) {
                    *with += to_u32(t.len());
                }
                if let Some(b) = f.get("body") {
                    scan_triggers(b, groups, with, auto);
                }
                return;
            }
            Some("Unary") => {
                if let Some(op) = v.get(2).and_then(|x| x.list())
                    && op.first().and_then(Node::atom) == Some("UnaryOp")
                    && op.get(1).and_then(Node::atom) == Some("Trigger")
                    && let Some(ann) = op.get(2).and_then(|x| x.list())
                {
                    match ann.get(1).and_then(Node::atom) {
                        Some("Trigger") => {
                            let g = crate::sexp::render(&op[2]);
                            if !groups.contains(&g) {
                                groups.push(g);
                            }
                        }
                        Some("AutoTrigger" | "AllTriggers") => *auto = true,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    for c in v {
        scan_triggers(c, groups, with, auto);
    }
}

fn classify_quant(body: &[Node]) -> (&'static str, u32) {
    let (mut groups, mut with, mut auto) = (Vec::new(), 0, false);
    for n in body {
        scan_triggers(n, &mut groups, &mut with, &mut auto);
    }
    let n = with + to_u32(groups.len());
    if n > 0 {
        ("explicit", n)
    } else if auto {
        ("auto_annotation", 0)
    } else {
        ("none", 0)
    }
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
    match v.first().and_then(Node::atom) {
        Some("@@" | "@") => {
            if let Some(Node::Str(s)) = v.get(1) {
                cx.span = s;
            }
        }
        Some(">") => match v.get(1).and_then(Node::atom) {
            Some("Call") => kind = "call",
            Some("Fuel") => {
                // (> Fuel (Fun :path P) fuel is_broadcast_use)
                let (Some(f), Some(b)) =
                    (v.get(3).and_then(Node::atom), v.get(4).and_then(Node::atom))
                else {
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
            Some(q @ ("Quant" | "Choose")) => {
                kind = "other";
                let (name, body) = if q == "Choose" {
                    ("choose", &v[2..])
                } else {
                    let name = match v
                        .get(2)
                        .and_then(|x| x.list())
                        .and_then(|l| l.first())
                        .and_then(Node::atom)
                    {
                        Some("Forall") => "forall",
                        Some("Exists") => "exists",
                        _ => return Err(format!("unknown quantifier kind at {}", cx.span)),
                    };
                    (name, &v[3..])
                };
                let (trig, n) = classify_quant(body);
                cx.quants.push((name, trig, n, section, cx.span));
            }
            Some("AssertAssume") => {
                kind = "other";
                let f = fields(&v[2..]);
                if f.get("is_assume").and_then(|x| x.atom()) == Some("true") {
                    // `admit()` is `assume(false)`.
                    let is_false = f.get("expr").is_some_and(|e| {
                        crate::sexp::render(e).contains("(> Const (Constant Bool false))")
                    });
                    cx.assumes.push((is_false, cx.span));
                }
            }
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
                    && op.first().and_then(Node::atom) == Some("UnaryOp")
                    && op.get(1).and_then(Node::atom) == Some("Trigger")
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
    match v.get(2).and_then(Node::atom) {
        Some("None") | None => "pub".into(),
        Some(m) => m.to_string(),
    }
}

fn count_exprs(n: Option<&&Node>) -> u32 {
    match n.and_then(|n| n.list()) {
        None => 0,
        Some(v) if v.first().and_then(Node::atom) == Some("tuple") => v[1..]
            .iter()
            .map(|x| x.list().map_or(0, |l| to_u32(l.len())))
            .sum(),
        Some(v) => to_u32(v.len()),
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
            .is_some_and(|x| matches!(x, Node::Str("self")));
        if !is_self {
            continue;
        }
        return typ_path(f.get("typ")?);
    }
    None
}

/// The impl's self type from the third field of an `--log impl-names` line, which lists the self
/// type first and then the trait's type arguments, separated by commas outside brackets.
fn impl_self_type(field: &str) -> String {
    let mut depth = 0usize;
    let mut end = field.len();
    for (i, c) in field.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    field[..end].trim().to_string()
}

/// `Handles<S>` as `Handles`: Verus prints a method under its impl type without type arguments.
fn strip_generics(t: &str) -> String {
    let mut depth = 0usize;
    t.chars()
        .filter(|&c| match c {
            '<' => {
                depth += 1;
                false
            }
            '>' => {
                depth = depth.saturating_sub(1);
                false
            }
            _ => depth == 0,
        })
        .collect()
}

/// First datatype of the crate named in a type, searching depth first (`Option<Cap<E>>` gives
/// `Cap`'s path). Verus prints a function without `self` under the type it constructs, so this is
/// the fallback for the friendly name of such an associated function (a constructor by its
/// return type, a lemma over `d: &Self` by its parameters).
fn own_datatype(t: &Node, krate: &str) -> Option<String> {
    let v = t.list()?;
    if v.first().and_then(Node::atom) == Some("Typ")
        && v.get(1).and_then(Node::atom) == Some("Datatype")
        && let Some(p) = v
            .get(2)
            .and_then(Node::list)
            .and_then(|dt| dt.get(2))
            .and_then(Node::atom)
        && p.split("::").next() == Some(krate)
    {
        return Some(p.to_string());
    }
    v.iter().find_map(|x| own_datatype(x, krate))
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
            // `&T` is `Decorate Ref`; `&mut T` (an exec method taking `&mut self`) is `MutRef`.
            "Decorate" | "MutRef" => return v.iter().skip(2).find_map(typ_path),
            _ => return None,
        }
    }
    None
}

#[allow(
    clippy::too_many_lines,
    reason = "one match over the function entry fields; each arm is a column"
)]
fn parse_function(
    top: &Node,
    span: &str,
    krate: &str,
    names: &ImplNames,
    facts: &mut CrateFacts,
) -> Result<(), String> {
    let v = top
        .list()
        .ok_or_else(|| "function entry is not a list".to_string())?;
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
    let (kind, trait_path) = match kind_list.get(1).and_then(Node::atom) {
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
        .and_then(Node::atom)
    {
        Some("Function") => "function",
        Some("Const") => "const",
        Some("Static") => "static",
        other => return Err(ctx(&format!("unknown ItemKind {other:?}"))),
    };
    let (opaque, reveal_vis) = match f.get("opaqueness").and_then(|n| n.list()) {
        Some(l) if l.get(1).and_then(Node::atom) == Some("Opaque") => (true, None),
        Some(l) if l.get(1).and_then(Node::atom) == Some("Revealed") => (
            false,
            Some(vis_of(fields(&l[2..]).get("visibility").copied())),
        ),
        other => {
            return Err(ctx(&format!(
                "unknown :opaqueness {:?}",
                other.map(<[Node<'_>]>::len)
            )));
        }
    };
    let body_vis = match f.get("body_visibility").and_then(|n| n.list()) {
        Some(l) if l.get(1).and_then(Node::atom) == Some("Visibility") => vis_of(l.get(2)),
        Some(l) if l.get(1).and_then(Node::atom) == Some("Uninterpreted") => "none".into(),
        _ => return Err(ctx("unknown :body_visibility")),
    };
    let a = f
        .get("attrs")
        .and_then(|n| n.list())
        .map(|l| fields(&l[1..]))
        .ok_or_else(|| ctx("missing :attrs"))?;
    let has_body = f.get("body").is_some_and(|n| n.atom() != Some("None"));
    let (file, line, _col, header_end) = parse_span(span).ok_or_else(|| ctx("unparsable span"))?;
    // The function's own span covers only its header; the body expression has its own span.
    let body_end = f
        .get("body")
        .and_then(|b| b.list())
        .filter(|l| l.first().and_then(Node::atom) == Some("@@"))
        .and_then(|l| match l.get(1) {
            Some(Node::Str(s)) => parse_span(s).map(|x| x.3),
            _ => None,
        });
    let end_line = body_end.unwrap_or(header_end).max(header_end);
    let name = path.rsplit("::").next().unwrap_or(path).to_string();
    let self_type = match (kind, impl_path) {
        ("trait_impl", Some(ip)) => names.get(ip).map(|x| impl_self_type(&x.1)),
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
        (Some(t), Some(_)) => {
            let t = strip_generics(t);
            if t.contains("::") {
                format!("{t}::{name}")
            } else {
                // A type parameter or primitive: Verus keeps the impl's path and puts the type
                // before the name (`m::impl&%0::S::build`).
                let imp = &path[..path.len() - name.len() - 2];
                format!("{imp}::{t}::{name}")
            }
        }
        (None, Some(_)) => f
            .get("ret")
            .and_then(|r| own_datatype(r, krate))
            .or_else(|| f.get("params").and_then(|p| own_datatype(p, krate)))
            .map_or_else(|| path.to_string(), |t| format!("{t}::{name}")),
        _ => path.to_string(),
    };
    let rlimit_attr = a.get("rlimit").and_then(|n| match n {
        Node::Atom("None") => None,
        n => Some(crate::sexp::render(n)),
    });
    let has_default = kf.get("has_default").and_then(|n| n.atom()) == Some("true");
    let trait_method = if kind == "trait_impl" {
        kf.get("method").and_then(|n| fun_path(n)).map(String::from)
    } else {
        None
    };
    let generated = name.starts_with("arrow_") && path.contains("::impl&%") && mode == "spec";
    let has_proxy = f.get("proxy").is_some_and(|n| n.atom() != Some("None"));
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
        has_default,
        trait_method,
        type_invariant: flag(&a, "is_type_invariant_fn"),
        generated,
    };
    let caller = facts.functions.len();
    let mut edges = Vec::new();
    let mut cx = Ctx {
        span,
        quants: Vec::new(),
        assumes: Vec::new(),
    };
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
    let at = |sp: &str| {
        let (file, line, _, _) =
            parse_span(sp).unwrap_or_else(|| (row.file.clone(), row.line, 0, row.line));
        (file, line)
    };
    for (quant, trigger, n_triggers, section, sp) in std::mem::take(&mut cx.quants) {
        let (file, line) = at(sp);
        facts.quants.push(QuantRow {
            caller,
            quant,
            trigger,
            n_triggers,
            section,
            file,
            line,
        });
    }
    for (admit, sp) in std::mem::take(&mut cx.assumes) {
        let (file, line) = at(sp);
        facts.trusted.push(TrustedRow {
            caller: Some(caller),
            kind: if admit { "admit" } else { "assume" },
            file,
            line,
            text: String::new(),
        });
    }
    let mut own_trust = |kind: &'static str| {
        facts.trusted.push(TrustedRow {
            caller: Some(caller),
            kind,
            file: row.file.clone(),
            line: row.line,
            text: String::new(),
        });
    };
    if row.external_body {
        own_trust(if row.broadcast_forall {
            "broadcast_axiom"
        } else {
            "external_body"
        });
    }
    if has_proxy {
        own_trust("assume_specification");
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
        match v.first().and_then(Node::atom) {
            Some("module_id") => {
                if let Some(m) = v.get(1).and_then(Node::atom)
                    && m.split("::").next() == Some(krate)
                {
                    facts.modules.push(m.to_string());
                }
            }
            Some(k @ ("external_fn" | "external_type")) => {
                let path = v.get(1).and_then(|n| fun_path(n).or_else(|| n.atom()));
                if let Some(p) = path
                    && p.split("::").next() == Some(krate)
                {
                    facts.externals.push((
                        if k == "external_fn" {
                            "external_fn"
                        } else {
                            "external_type"
                        },
                        p.to_string(),
                    ));
                }
            }
            Some("group_id") => {
                if let Some(m) = v.get(1).and_then(Node::atom)
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
    name_from_impl_siblings(&mut facts);
    Ok(facts)
}

/// Friendly name of an associated function without `self` from a sibling in the same `impl`
/// block that has one: `impl&%3::lemma_pages_fit(k, n, s)` is `T::lemma_pages_fit` when
/// `impl&%3::reserve(&self)` is a method of `T`. This beats the return-type guess.
fn name_from_impl_siblings(facts: &mut CrateFacts) {
    let mut ty: HashMap<String, String> = HashMap::new();
    for f in &facts.functions {
        if let (Some(t), Some((imp, _))) = (&f.self_type, f.path.rsplit_once("::"))
            && imp.contains("::impl&%")
        {
            ty.entry(imp.to_string()).or_insert_with(|| t.clone());
        }
    }
    for f in &mut facts.functions {
        if f.self_type.is_none()
            && let Some((imp, name)) = f.path.rsplit_once("::")
            && let Some(t) = ty.get(imp)
        {
            f.friendly = format!("{t}::{name}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/mini.vir");

    /// An inherent-impl function Verus prints as `T::name`: a `&mut self` method names `T` by its
    /// receiver, a constructor by the own-crate type inside the type it returns.
    #[test]
    fn friendly_names_of_inherent_functions() {
        let f = |path: &str, params: &str, ret: &str| {
            format!(
                r#"(@ "src/m.rs:3:1: 5:2 (#0)" (Function
  :name (Fun :path {path}) :proxy None :kind (FunctionKind Static) :visibility (Visibility :restricted_to None)
  :body_visibility (BodyVisibility Visibility (Visibility :restricted_to None))
  :opaqueness (Opaqueness Opaque) :owning_module mini::m :mode Exec :typ_params () :typ_bounds () :params ({params})
  :ret (@ "src/m.rs:3:9: 3:12 (#0)" (Param :name (VarIdent "r" (VarIdentDisambiguate RustcId 1)) :typ {ret} :mode Exec :user_mut false :unwrapped_info None))
  :require () :ensure (tuple () ()) :returns None :decrease () :decrease_by None
  :item_kind (ItemKind Function) :attrs (FunctionAttrs :uses_ghost_blocks true :inline false :hidden () :broadcast_forall false :broadcast_forall_only false :no_auto_trigger false :bit_vector false :atomic false :integer_ring false :nonlinear false :spinoff_prover false :rlimit None :is_external_body false) :body None :extra_dependencies ()))
"#
            )
        };
        let self_param = r#"(@ "src/m.rs:3:2: 3:3 (#0)" (Param :name (VarIdent "self" (VarIdentDisambiguate RustcId 0)) :typ (Typ MutRef (Typ Datatype (Dt Path mini::m::Pool) () ())) :mode Exec :user_mut false :unwrapped_info None))"#;
        let ret_opt = "(Typ Datatype (Dt Path core::option::Option) ((Typ Datatype (Dt Path mini::m::Cap) ((Typ TypParam \"E\")) ())) ())";
        let text = format!(
            "(module_id mini::m)\n{}\n{}",
            f("mini::m::impl&%0::alloc", self_param, "(Typ Bool)"),
            f("mini::m::impl&%1::new", "", ret_opt),
        );
        let facts = parse_log(&text, "mini", &ImplNames::new()).unwrap();
        let name = |p: &str| {
            facts
                .functions
                .iter()
                .find(|x| x.path == p)
                .unwrap()
                .friendly
                .clone()
        };
        assert_eq!(name("mini::m::impl&%0::alloc"), "mini::m::Pool::alloc");
        assert_eq!(name("mini::m::impl&%1::new"), "mini::m::Cap::new");
        assert_eq!(strip_generics("a::H<S, Vec<T>>"), "a::H");
        assert_eq!(impl_self_type("a::E<V, C>, S"), "a::E<V, C>");
        assert_eq!(impl_self_type("S, a::Positions"), "S");
    }

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
