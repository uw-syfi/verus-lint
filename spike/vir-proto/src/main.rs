//! Spike: parse a Verus `--log vir` file (`crate.vir`) into a function table
//! and an edge table. No dependencies; output is TSV.
//!
//! usage: vir-proto <crate.vir> <out_dir> [path_prefix]
//!
//! Only functions whose path starts with `path_prefix` (default: all) are
//! emitted as rows; edges are emitted from those functions to any callee.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::time::Instant;

#[derive(Debug)]
enum Node<'a> {
    Atom(&'a str),
    Str(&'a str),
    List(Vec<Node<'a>>),
}

impl<'a> Node<'a> {
    fn atom(&self) -> Option<&'a str> {
        match self {
            Node::Atom(a) => Some(a),
            _ => None,
        }
    }
    fn list(&self) -> Option<&[Node<'a>]> {
        match self {
            Node::List(v) => Some(v),
            _ => None,
        }
    }
    fn head(&self) -> Option<&'a str> {
        self.list().and_then(|v| v.first()).and_then(|n| n.atom())
    }
}

struct Parser<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while self.i < self.b.len() {
            match self.b[self.i] {
                b' ' | b'\n' | b'\t' | b'\r' => self.i += 1,
                b';' => {
                    while self.i < self.b.len() && self.b[self.i] != b'\n' {
                        self.i += 1
                    }
                }
                _ => break,
            }
        }
    }
    fn next(&mut self) -> Option<Node<'a>> {
        self.skip_ws();
        if self.i >= self.b.len() {
            return None;
        }
        match self.b[self.i] {
            b'(' => {
                self.i += 1;
                let mut v = Vec::new();
                loop {
                    self.skip_ws();
                    if self.b[self.i] == b')' {
                        self.i += 1;
                        return Some(Node::List(v));
                    }
                    v.push(self.next().expect("unterminated list"));
                }
            }
            b'"' => {
                let st = self.i + 1;
                self.i += 1;
                while self.b[self.i] != b'"' {
                    if self.b[self.i] == b'\\' {
                        self.i += 1;
                    }
                    self.i += 1;
                }
                self.i += 1;
                Some(Node::Str(&self.s[st..self.i - 1]))
            }
            _ => {
                let st = self.i;
                while self.i < self.b.len()
                    && !matches!(self.b[self.i], b' ' | b'\n' | b'\t' | b'\r' | b'(' | b')')
                {
                    self.i += 1;
                }
                Some(Node::Atom(&self.s[st..self.i]))
            }
        }
    }
}

/// `:key value` pairs following the head of a record-style list.
fn fields<'a, 'b>(v: &'b [Node<'a>]) -> HashMap<&'a str, &'b Node<'a>> {
    let mut m = HashMap::new();
    let mut k = 0;
    while k + 1 < v.len() {
        if let Some(a) = v[k].atom() {
            if a.starts_with(':') && a.len() > 1 {
                m.insert(&a[1..], &v[k + 1]);
                k += 2;
                continue;
            }
        }
        k += 1;
    }
    m
}

/// `(Fun :path P)` -> P
fn fun_path<'a>(n: &Node<'a>) -> Option<&'a str> {
    let v = n.list()?;
    if v.len() == 3 && v[0].atom() == Some("Fun") && v[1].atom() == Some(":path") {
        v[2].atom()
    } else {
        None
    }
}

fn render(n: &Node) -> String {
    match n {
        Node::Atom(a) => a.to_string(),
        Node::Str(s) => format!("\"{s}\""),
        Node::List(v) => format!("({})", v.iter().map(render).collect::<Vec<_>>().join(" ")),
    }
}

struct Edge<'a> {
    callee: &'a str,
    section: &'static str,
    kind: &'static str,
    in_trigger: bool,
    span: &'a str,
}

/// Walk an expression tree, recording every function reference with the
/// expression kind that holds it and whether it sits inside a trigger.
fn walk<'a>(
    n: &Node<'a>,
    section: &'static str,
    kind: &'static str,
    in_trigger: bool,
    span: &'a str,
    out: &mut Vec<Edge<'a>>,
    quants: &mut (usize, usize),
) {
    let Some(v) = n.list() else { return };
    if let Some(p) = fun_path(n) {
        out.push(Edge { callee: p, section, kind, in_trigger, span });
        return;
    }
    let mut kind = kind;
    let mut in_trigger = in_trigger;
    let mut span = span;
    match v.first().and_then(|h| h.atom()) {
        Some("@@") | Some("@") => {
            if let Some(Node::Str(s)) = v.get(1) {
                span = s;
            }
        }
        Some(">") => match v.get(1).and_then(|h| h.atom()) {
            Some("Call") => kind = "call",
            Some("Fuel") => {
                // (> Fuel (Fun :path P) fuel is_broadcast_use)
                kind = if v.get(4).and_then(|x| x.atom()) == Some("true") {
                    "broadcast_use"
                } else {
                    "reveal"
                };
            }
            Some("ExecFnByName") => kind = "fn_value",
            Some("WithTriggers") => {
                // :triggers are trigger terms; the body is not.
                let f = fields(&v[2..]);
                if let Some(t) = f.get("triggers") {
                    walk(t, section, kind, true, span, out, quants);
                }
                if let Some(b) = f.get("body") {
                    walk(b, section, kind, in_trigger, span, out, quants);
                }
                return;
            }
            Some("Quant") => {
                quants.0 += 1;
                if !has_trigger(n) {
                    quants.1 += 1;
                }
            }
            Some("Unary") => {
                if let Some(op) = v.get(2).and_then(|x| x.list()) {
                    if op.first().and_then(|x| x.atom()) == Some("UnaryOp")
                        && op.get(1).and_then(|x| x.atom()) == Some("Trigger")
                    {
                        in_trigger = true;
                    }
                }
            }
            _ => kind = "other",
        },
        Some("Typ") => kind = "type",
        Some("CallTargetKind") => kind = "resolved_impl",
        _ => {}
    }
    for c in v {
        walk(c, section, kind, in_trigger, span, out, quants);
    }
}

fn has_trigger(n: &Node) -> bool {
    match n {
        Node::List(v) => {
            if v.first().and_then(|x| x.atom()) == Some(">")
                && v.get(1).and_then(|x| x.atom()) == Some("WithTriggers")
            {
                return true;
            }
            if v.first().and_then(|x| x.atom()) == Some("UnaryOp")
                && v.get(1).and_then(|x| x.atom()) == Some("Trigger")
            {
                return true;
            }
            v.iter().any(has_trigger)
        }
        _ => false,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (path, out_dir) = (&args[1], &args[2]);
    let prefix = args.get(3).map(|s| s.as_str()).unwrap_or("");
    let t0 = Instant::now();
    let text = fs::read_to_string(path).expect("read");
    let t_read = t0.elapsed();
    let mut p = Parser { s: &text, b: text.as_bytes(), i: 0 };
    fs::create_dir_all(out_dir).unwrap();
    let mut fns = std::io::BufWriter::new(fs::File::create(format!("{out_dir}/functions.tsv")).unwrap());
    let mut eds = std::io::BufWriter::new(fs::File::create(format!("{out_dir}/edges.tsv")).unwrap());
    writeln!(fns, "path\tmode\tkind\topaqueness\tvisibility\tbody_visibility\trlimit\tspinoff_prover\texternal_body\tbroadcast_forall\thidden\tquants\tquants_no_trigger\tassumes\tspan").unwrap();
    writeln!(eds, "caller\tcallee\tsection\tkind\tin_trigger\tspan").unwrap();
    let (mut n_forms, mut n_fns, mut n_edges) = (0usize, 0usize, 0usize);
    let mut kinds: HashMap<String, usize> = HashMap::new();
    while let Some(form) = p.next() {
        n_forms += 1;
        let Some(v) = form.list() else { continue };
        let top = if v.first().and_then(|x| x.atom()) == Some("@") { v.get(2) } else { Some(&form) };
        let Some(top) = top else { continue };
        *kinds.entry(top.head().unwrap_or("?").to_string()).or_default() += 1;
        if top.head() != Some("Function") {
            continue;
        }
        let span = match v.get(1) {
            Some(Node::Str(s)) => *s,
            _ => "",
        };
        let f = fields(&top.list().unwrap()[1..]);
        let name = f.get("name").and_then(|n| fun_path(n)).unwrap_or("?");
        if !name.starts_with(prefix) {
            continue;
        }
        n_fns += 1;
        let a = f.get("attrs").and_then(|n| n.list()).map(|v| fields(&v[1..])).unwrap_or_default();
        let attr = |k: &str| a.get(k).map(|n| render(n)).unwrap_or_default();
        let vis = |k: &str| f.get(k).map(|n| render(n)).unwrap_or_default();
        let mut edges = Vec::new();
        let mut quants = (0, 0);
        for (sec, key) in [
            ("require", "require"),
            ("ensure", "ensure"),
            ("returns", "returns"),
            ("decrease", "decrease"),
            ("decrease_by", "decrease_by"),
            ("body", "body"),
        ] {
            if let Some(n) = f.get(key) {
                walk(n, sec, "other", false, span, &mut edges, &mut quants);
            }
        }
        if let Some(h) = a.get("hidden") {
            walk(h, "hide", "hide", false, span, &mut edges, &mut quants);
        }
        let body = f.get("body").map(|n| render(n)).unwrap_or_default();
        let assumes = body.matches(":is_assume true").count();
        writeln!(
            fns,
            "{name}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{assumes}\t{span}",
            vis("mode"),
            vis("kind"),
            vis("opaqueness"),
            vis("visibility"),
            vis("body_visibility"),
            attr("rlimit"),
            attr("spinoff_prover"),
            attr("is_external_body"),
            attr("broadcast_forall"),
            attr("hidden"),
            quants.0,
            quants.1,
        )
        .unwrap();
        for e in edges {
            if e.kind == "type" {
                continue;
            }
            n_edges += 1;
            writeln!(eds, "{name}\t{}\t{}\t{}\t{}\t{}", e.callee, e.section, e.kind, e.in_trigger, e.span).unwrap();
        }
    }
    let total = t0.elapsed();
    eprintln!(
        "bytes={} forms={} functions={} edges={} read={:.2?} total={:.2?}",
        text.len(),
        n_forms,
        n_fns,
        n_edges,
        t_read,
        total
    );
    let mut k: Vec<_> = kinds.into_iter().collect();
    k.sort();
    eprintln!("top-level kinds: {k:?}");
}
