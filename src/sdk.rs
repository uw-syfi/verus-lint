//! Rust rule SDK: typed fact access, use graphs and the `Rule` trait.
//!
//! A user rules crate depends on `verus-lint`, implements [`Rule`] for its checks and calls
//! [`crate::run`] with them from `main`. The CLI builds and starts that binary for
//! `check` and `run` when `[rules] rust` names the crate. A rule reads facts through
//! [`Facts`] (typed rows of `functions` and `uses`, plus read-only SQL for the rest) and
//! reports through [`Findings`].

use crate::analysis::sccs;
use crate::rules::Finding;
use anyhow::{Context, Result, anyhow, bail};
use duckdb::Connection;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Set of functions, ordered by id so iteration is deterministic.
pub type FnSet = BTreeSet<FnId>;

/// Identifier of a function row (`functions.fn_id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FnId(pub i64);

/// Declared severity of a rule's findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Informational.
    Note,
    /// Should be fixed.
    Warning,
    /// Must be fixed.
    Error,
}

impl Severity {
    /// Parse `note`, `warning` or `error`.
    ///
    /// # Errors
    /// Fails on any other text.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "note" => Ok(Self::Note),
            "warning" => Ok(Self::Warning),
            "error" => Ok(Self::Error),
            _ => bail!("bad severity `{s}` (note, warning or error)"),
        }
    }

    /// The text form used in rule headers and output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// How a rule's findings are compared with the baseline file.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Ratchet {
    /// No ratchet: every finding counts.
    #[default]
    None,
    /// A finding is covered when its entity is in the baseline's set.
    Set,
    /// A finding is covered when its entity is in the baseline and its metric does not exceed
    /// `max(ratio * base, base + abs)`.
    Metric {
        /// Allowed multiplicative growth over the baseline value.
        ratio: f64,
        /// Allowed absolute growth over the baseline value.
        abs: f64,
    },
}

impl Ratchet {
    /// Parse the header form `set`, `metric` or `metric, ratio = 1.2, abs = 5`.
    ///
    /// # Errors
    /// Fails on an unknown kind or key, or a non-numeric value.
    pub fn parse(s: &str) -> Result<Self> {
        let mut parts = s.split(',').map(str::trim);
        match parts.next().unwrap_or("") {
            "set" => Ok(Self::Set),
            "metric" => {
                let (mut ratio, mut abs) = (1.0, 0.0);
                for kv in parts {
                    let (k, v) = kv
                        .split_once('=')
                        .ok_or_else(|| anyhow!("bad ratchet option `{kv}`"))?;
                    let v: f64 = v
                        .trim()
                        .parse()
                        .with_context(|| format!("ratchet option `{kv}`"))?;
                    match k.trim() {
                        "ratio" => ratio = v,
                        "abs" => abs = v,
                        other => bail!("unknown ratchet option `{other}`"),
                    }
                }
                Ok(Self::Metric { ratio, abs })
            }
            other => bail!("unknown ratchet kind `{other}` (set or metric)"),
        }
    }

    /// Largest metric still covered by a baseline value.
    #[must_use]
    pub fn allowed(self, base: f64) -> f64 {
        match self {
            Self::Metric { ratio, abs } => (ratio * base).max(base + abs),
            _ => base,
        }
    }
}

/// Static description of a rule: identity, default severity, parameters and ratchet.
#[derive(Debug, Clone)]
pub struct RuleMeta {
    /// Unique id, `namespace/name`.
    pub id: String,
    /// One-line description.
    pub summary: String,
    /// Severity of the rule's findings unless a finding overrides it.
    pub severity: Severity,
    /// The rule reads dynamic (verification) facts and is skipped when there are none.
    pub needs_dynamic: bool,
    /// Baseline comparison.
    pub ratchet: Ratchet,
    /// Parameter defaults, overridable in the config and on the command line.
    pub params: Vec<(String, String)>,
    /// Schema version range the rule was written against (informational).
    pub schema: Option<String>,
}

impl RuleMeta {
    /// A rule with a warning severity, no parameters and no ratchet.
    #[must_use]
    pub fn new(id: &str, summary: &str) -> Self {
        Self {
            id: id.to_string(),
            summary: summary.to_string(),
            severity: Severity::Warning,
            needs_dynamic: false,
            ratchet: Ratchet::None,
            params: Vec::new(),
            schema: None,
        }
    }

    /// Set the default severity.
    #[must_use]
    pub const fn severity(mut self, s: Severity) -> Self {
        self.severity = s;
        self
    }

    /// Add a parameter with its default value.
    #[must_use]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "builder argument: any displayable value"
    )]
    pub fn param(mut self, name: &str, default: impl ToString) -> Self {
        self.params.push((name.to_string(), default.to_string()));
        self
    }

    /// Set the ratchet.
    #[must_use]
    pub const fn ratchet(mut self, r: Ratchet) -> Self {
        self.ratchet = r;
        self
    }

    /// Mark the rule as reading dynamic facts.
    #[must_use]
    pub const fn needs_dynamic(mut self) -> Self {
        self.needs_dynamic = true;
        self
    }
}

/// Resolved parameter values of one rule: defaults, then config, then the command line.
#[derive(Debug, Clone, Default)]
pub struct Params(pub BTreeMap<String, String>);

impl Params {
    /// Value of a parameter as text.
    ///
    /// # Errors
    /// Fails when the rule declared no such parameter.
    pub fn get_str(&self, name: &str) -> Result<&str> {
        self.0
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| anyhow!("unknown parameter `{name}`"))
    }

    /// Value of a parameter as an unsigned integer.
    ///
    /// # Errors
    /// Fails when the parameter is missing or not an unsigned integer.
    pub fn get_u64(&self, name: &str) -> Result<u64> {
        let v = self.get_str(name)?;
        v.parse()
            .with_context(|| format!("parameter `{name}` = `{v}` is not an unsigned integer"))
    }

    /// Value of a parameter as a float.
    ///
    /// # Errors
    /// Fails when the parameter is missing or not a number.
    pub fn get_f64(&self, name: &str) -> Result<f64> {
        let v = self.get_str(name)?;
        v.parse()
            .with_context(|| format!("parameter `{name}` = `{v}` is not a number"))
    }
}

/// Execution mode of a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Compiled code.
    Exec,
    /// Ghost code with a proof body.
    Proof,
    /// Mathematical definition.
    Spec,
}

/// How a function relates to traits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A free function or inherent method.
    Static,
    /// A trait method declaration.
    TraitDecl,
    /// An implementation of a trait declared in an extracted crate.
    TraitImpl,
    /// An implementation of a trait declared outside the extracted crates.
    ForeignTraitImpl,
}

/// Kind of a reference from one function to another item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UseKind {
    /// A call.
    Call,
    /// A `reveal` of an opaque definition.
    Reveal,
    /// A function-local `broadcast use`.
    BroadcastUse,
    /// A function used as a value.
    FnValue,
    /// A trait method call resolved to one implementation.
    ResolvedImpl,
    /// A `hide`.
    Hide,
}

fn parse_mode(s: &str) -> Result<Mode> {
    match s {
        "exec" => Ok(Mode::Exec),
        "proof" => Ok(Mode::Proof),
        "spec" => Ok(Mode::Spec),
        _ => bail!("unknown function mode `{s}`"),
    }
}

fn parse_kind(s: &str) -> Result<Kind> {
    match s {
        "static" => Ok(Kind::Static),
        "trait_decl" => Ok(Kind::TraitDecl),
        "trait_impl" => Ok(Kind::TraitImpl),
        "foreign_trait_impl" => Ok(Kind::ForeignTraitImpl),
        _ => bail!("unknown function kind `{s}`"),
    }
}

fn parse_use_kind(s: &str) -> Result<UseKind> {
    match s {
        "call" => Ok(UseKind::Call),
        "reveal" => Ok(UseKind::Reveal),
        "broadcast_use" => Ok(UseKind::BroadcastUse),
        "fn_value" => Ok(UseKind::FnValue),
        "resolved_impl" => Ok(UseKind::ResolvedImpl),
        "hide" => Ok(UseKind::Hide),
        _ => bail!("unknown use kind `{s}`"),
    }
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "mirrors the boolean columns of the functions table"
)]
/// One row of the `functions` table (the columns most rules need; use [`Facts::query`] for the
/// rest).
#[derive(Debug, Clone)]
pub struct Function {
    /// Row id.
    pub id: FnId,
    /// Full VIR path, unique per function.
    pub path: String,
    /// Rust-style path as Verus prints it in reports.
    pub friendly: String,
    /// Crate identifier.
    pub krate: String,
    /// Module path.
    pub module: String,
    /// Bare function name.
    pub name: String,
    /// Execution mode.
    pub mode: Mode,
    /// Trait relationship.
    pub kind: Kind,
    /// Visibility: `pub`, or the restricting module.
    pub vis: String,
    /// Spec is opaque.
    pub opaque: bool,
    /// Marked `external_body`.
    pub external_body: bool,
    /// Has a body in the log.
    pub has_body: bool,
    /// Compiler-generated (for example field accessors).
    pub generated: bool,
    /// Source file, relative to the workspace.
    pub file: String,
    /// First source line.
    pub line: u32,
    /// Lines of the body.
    pub body_lines: u32,
}

/// One row of the `uses` table: a reference from `caller` to `callee_path`.
#[derive(Debug, Clone)]
pub struct Use {
    /// Referencing function.
    pub caller: FnId,
    /// Referenced function, when it is one of the extracted functions.
    pub callee: Option<FnId>,
    /// Path of the referenced item as written in the log.
    pub callee_path: String,
    /// Part of the caller where the reference occurs (`requires`, `body`, and so on).
    pub section: String,
    /// Kind of reference.
    pub kind: UseKind,
    /// The reference sits inside a trigger.
    pub in_trigger: bool,
    /// Source file.
    pub file: String,
    /// Source line.
    pub line: u32,
}

/// Facts of one extracted workspace, loaded from the database.
pub struct Facts {
    conn: Connection,
    functions: Vec<Function>,
    uses: Vec<Use>,
    by_id: HashMap<FnId, usize>,
    by_path: HashMap<String, usize>,
    from: HashMap<FnId, Vec<usize>>,
    of: HashMap<FnId, Vec<usize>>,
    roots: Vec<FnId>,
}

impl Facts {
    /// Load the typed tables from an open database connection.
    ///
    /// # Errors
    /// Fails on a database error or an enum value this SDK version does not know.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "line numbers and counts are small non-negative integers"
    )]
    pub fn load(conn: Connection) -> Result<Self> {
        let mut functions = Vec::new();
        {
            let mut st = conn.prepare(
                "SELECT fn_id, path, friendly, crate, module, name, mode, kind, vis, opaque, \
                 external_body, has_body, generated, file, line, body_lines FROM functions \
                 ORDER BY fn_id",
            )?;
            let mut rows = st.query([])?;
            while let Some(r) = rows.next()? {
                functions.push(Function {
                    id: FnId(r.get(0)?),
                    path: r.get(1)?,
                    friendly: r.get(2)?,
                    krate: r.get(3)?,
                    module: r.get(4)?,
                    name: r.get(5)?,
                    mode: parse_mode(&r.get::<_, String>(6)?)?,
                    kind: parse_kind(&r.get::<_, String>(7)?)?,
                    vis: r.get(8)?,
                    opaque: r.get(9)?,
                    external_body: r.get(10)?,
                    has_body: r.get(11)?,
                    generated: r.get(12)?,
                    file: r.get::<_, Option<String>>(13)?.unwrap_or_default(),
                    line: r.get::<_, Option<i32>>(14)?.unwrap_or(0) as u32,
                    body_lines: r.get::<_, Option<i32>>(15)?.unwrap_or(0) as u32,
                });
            }
        }
        let mut uses = Vec::new();
        {
            let mut st = conn.prepare(
                "SELECT caller_id, callee_id, callee_path, section, kind, in_trigger, file, line \
                 FROM uses",
            )?;
            let mut rows = st.query([])?;
            while let Some(r) = rows.next()? {
                uses.push(Use {
                    caller: FnId(r.get(0)?),
                    callee: r.get::<_, Option<i64>>(1)?.map(FnId),
                    callee_path: r.get(2)?,
                    section: r.get(3)?,
                    kind: parse_use_kind(&r.get::<_, String>(4)?)?,
                    in_trigger: r.get::<_, Option<bool>>(5)?.unwrap_or(false),
                    file: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                    line: r.get::<_, Option<i32>>(7)?.unwrap_or(0) as u32,
                });
            }
        }
        let roots: Vec<FnId> = {
            let mut st = conn.prepare("SELECT DISTINCT fn_id FROM roots ORDER BY fn_id")?;
            let mut rows = st.query([])?;
            let mut v = Vec::new();
            while let Some(r) = rows.next()? {
                v.push(FnId(r.get(0)?));
            }
            v
        };
        let by_id = functions
            .iter()
            .enumerate()
            .map(|(i, f)| (f.id, i))
            .collect();
        let by_path = functions
            .iter()
            .enumerate()
            .map(|(i, f)| (f.path.clone(), i))
            .collect();
        let (mut from, mut of): (HashMap<FnId, Vec<usize>>, HashMap<FnId, Vec<usize>>) =
            (HashMap::new(), HashMap::new());
        for (i, u) in uses.iter().enumerate() {
            from.entry(u.caller).or_default().push(i);
            if let Some(c) = u.callee {
                of.entry(c).or_default().push(i);
            }
        }
        Ok(Self {
            conn,
            functions,
            uses,
            by_id,
            by_path,
            from,
            of,
            roots,
        })
    }

    /// All functions, ordered by id.
    #[must_use]
    pub fn functions(&self) -> &[Function] {
        &self.functions
    }

    /// The function with this id.
    ///
    /// # Panics
    /// Panics on an id that did not come from this `Facts`.
    #[must_use]
    #[allow(clippy::indexing_slicing, reason = "ids come from this database")]
    pub fn function(&self, id: FnId) -> &Function {
        &self.functions[self.by_id[&id]]
    }

    /// The function with this VIR path.
    #[must_use]
    pub fn by_path(&self, path: &str) -> Option<&Function> {
        self.by_path.get(path).map(|&i| &self.functions[i])
    }

    /// Every use whose caller is `f`.
    pub fn uses_from(&self, f: FnId) -> impl Iterator<Item = &Use> {
        self.from
            .get(&f)
            .into_iter()
            .flatten()
            .map(|&i| &self.uses[i])
    }

    /// Every use whose callee is `f`.
    pub fn uses_of(&self, f: FnId) -> impl Iterator<Item = &Use> {
        self.of
            .get(&f)
            .into_iter()
            .flatten()
            .map(|&i| &self.uses[i])
    }

    /// All uses.
    #[must_use]
    pub fn uses(&self) -> &[Use] {
        &self.uses
    }

    /// Functions that are live by definition (the `roots` view: exec functions, config
    /// patterns, name files, foreign trait impls, type invariants).
    #[must_use]
    pub fn roots(&self) -> &[FnId] {
        &self.roots
    }

    /// Graph over functions with an edge for every resolved use that `keep` accepts.
    pub fn graph(&self, keep: impl Fn(&Use) -> bool) -> Graph {
        let n = self.functions.len();
        let (mut succ, mut pred) = (vec![Vec::new(); n], vec![Vec::new(); n]);
        for u in &self.uses {
            let Some(c) = u.callee else { continue };
            if !keep(u) {
                continue;
            }
            let (a, b) = (self.by_id[&u.caller], self.by_id[&c]);
            succ[a].push(b);
            pred[b].push(a);
        }
        for v in succ.iter_mut().chain(pred.iter_mut()) {
            v.sort_unstable();
            v.dedup();
        }
        Graph {
            ids: self.functions.iter().map(|f| f.id).collect(),
            index: self.by_id.clone(),
            succ,
            pred,
        }
    }

    /// The database connection, for SQL.
    #[must_use]
    pub const fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Run a read-only query (`SELECT` or `WITH`) and return every cell as text, header row first.
    ///
    /// # Errors
    /// Fails on SQL errors or when the statement is not a query.
    pub fn query(&self, sql: &str) -> Result<Vec<Vec<String>>> {
        let head = sql.trim_start().to_ascii_lowercase();
        if !(head.starts_with("select") || head.starts_with("with")) {
            bail!("Facts::query runs SELECT or WITH statements only");
        }
        let mut st = self.conn.prepare(sql)?;
        let mut rows = st.query([])?;
        let names: Vec<String> = rows
            .as_ref()
            .map(duckdb::Statement::column_names)
            .unwrap_or_default();
        let mut out = vec![names.clone()];
        while let Some(r) = rows.next()? {
            let mut cells = Vec::new();
            for i in 0..names.len() {
                cells.push(crate::rules::cell_text(
                    &r.get::<_, duckdb::types::Value>(i)?,
                ));
            }
            out.push(cells);
        }
        Ok(out)
    }

    /// The database has verification facts (a non-empty `verify_fn` table).
    #[must_use]
    pub fn has_dynamic(&self) -> bool {
        self.conn
            .query_row("SELECT count(*) FROM verify_fn", [], |r| r.get::<_, i64>(0))
            .is_ok_and(|n| n > 0)
    }
}

/// Directed graph over functions.
pub struct Graph {
    ids: Vec<FnId>,
    index: HashMap<FnId, usize>,
    succ: Vec<Vec<usize>>,
    pred: Vec<Vec<usize>>,
}

impl Graph {
    /// Functions reachable from `from` along edges, including `from` itself.
    #[must_use]
    pub fn reachable(&self, from: &[FnId]) -> FnSet {
        let mut seen = vec![false; self.ids.len()];
        let mut stack: Vec<usize> = from
            .iter()
            .filter_map(|f| self.index.get(f).copied())
            .collect();
        for &s in &stack {
            seen[s] = true;
        }
        while let Some(v) = stack.pop() {
            for &w in &self.succ[v] {
                if !seen[w] {
                    seen[w] = true;
                    stack.push(w);
                }
            }
        }
        seen.iter()
            .enumerate()
            .filter(|&(_, &s)| s)
            .map(|(i, _)| self.ids[i])
            .collect()
    }

    /// Strongly connected components, each sorted by id, ordered by their smallest id. Every
    /// function is in exactly one component; a function outside any cycle is a component of
    /// one.
    #[must_use]
    pub fn sccs(&self) -> Vec<Vec<FnId>> {
        let comp = sccs(self.ids.len(), &self.succ);
        let mut groups: BTreeMap<usize, Vec<FnId>> = BTreeMap::new();
        for (i, c) in comp.iter().enumerate() {
            groups.entry(*c).or_default().push(self.ids[i]);
        }
        let mut out: Vec<Vec<FnId>> = groups.into_values().collect();
        for g in &mut out {
            g.sort_unstable();
        }
        out.sort_unstable_by_key(|g| g[0]);
        out
    }

    /// Functions with an edge to `f`.
    #[must_use]
    pub fn callers(&self, f: FnId) -> Vec<FnId> {
        self.index
            .get(&f)
            .map(|&i| self.pred[i].iter().map(|&j| self.ids[j]).collect())
            .unwrap_or_default()
    }

    /// Functions `f` has an edge to.
    #[must_use]
    pub fn callees(&self, f: FnId) -> Vec<FnId> {
        self.index
            .get(&f)
            .map(|&i| self.succ[i].iter().map(|&j| self.ids[j]).collect())
            .unwrap_or_default()
    }
}

/// What a rule sees: the facts and its resolved parameters.
pub struct Cx<'a> {
    /// Extracted facts.
    pub facts: &'a Facts,
    /// Parameters of this rule.
    pub params: &'a Params,
}

/// A check over the facts. Implement this in a rules crate and pass instances to [`crate::run`].
pub trait Rule {
    /// Identity, severity, parameters and ratchet.
    fn meta(&self) -> RuleMeta;

    /// Report findings for the facts.
    ///
    /// # Errors
    /// An error aborts the run with exit status 2.
    fn check(&self, cx: &Cx, out: &mut Findings) -> Result<()>;
}

/// Collector the rule pushes findings into.
#[derive(Debug, Default)]
pub struct Findings(pub(crate) Vec<Finding>);

impl Findings {
    /// Add a finding.
    pub fn push(&mut self, f: Finding) {
        self.0.push(f);
    }

    /// Number of findings so far.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// No findings so far.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{CrateInfo, Db};
    use crate::vir::{ImplNames, parse_log};

    fn fixture() -> Facts {
        let text = include_str!("../tests/fixtures/mini.vir");
        let facts = parse_log(text, "mini", &ImplNames::new()).unwrap();
        let mut db = Db::in_memory().unwrap();
        db.load_crate(
            &facts,
            &CrateInfo {
                manifest: "Cargo.toml",
                log_bytes: 0,
            },
        )
        .unwrap();
        db.resolve().unwrap();
        Facts::load(db.conn).unwrap()
    }

    #[test]
    fn typed_rows_and_graph() {
        let f = fixture();
        assert_eq!(f.functions().len(), 4);
        let open = f.by_path("mini::m::open_a").unwrap();
        assert_eq!(open.mode, Mode::Spec);
        let callers: Vec<_> = f
            .uses_of(open.id)
            .map(|u| u.caller)
            .collect::<FnSet>()
            .into_iter()
            .collect();
        assert_eq!(callers.len(), 1);
        let g = f.graph(|u| u.kind == UseKind::Call);
        assert_eq!(g.callers(open.id), callers);
        assert!(g.reachable(&callers).contains(&open.id));
        let comps = g.sccs();
        assert_eq!(comps.iter().map(Vec::len).sum::<usize>(), 4);
    }

    #[test]
    fn query_is_read_only() {
        let f = fixture();
        assert_eq!(
            f.query("SELECT count(*) AS n FROM functions").unwrap()[1],
            ["4"]
        );
        assert!(f.query("DROP TABLE functions").is_err());
    }

    #[test]
    fn ratchet_headers() {
        assert_eq!(Ratchet::parse("set").unwrap(), Ratchet::Set);
        let r = Ratchet::parse("metric, ratio = 1.2, abs = 5").unwrap();
        assert!((r.allowed(100.0) - 120.0).abs() < 1e-9);
        assert!((r.allowed(10.0) - 15.0).abs() < 1e-9);
        assert!(Ratchet::parse("metric, bogus = 1").is_err());
        assert!(Ratchet::parse("level").is_err());
    }
}
