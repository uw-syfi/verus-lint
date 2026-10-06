//! Rule evaluation: run SQL and Rust rules, apply levels and the baseline.

use crate::baseline::Baseline;
use crate::config::{Config, Level};
use crate::rules::{self, Finding};
use crate::sdk::{Cx, Facts, Findings, Params, Ratchet, Rule, RuleMeta};
use anyhow::{Result, bail};
use std::collections::{BTreeMap, BTreeSet};

/// A rule from either source.
pub enum Source<'a> {
    /// A `.sql` rule file.
    Sql(rules::Rule),
    /// A rule implemented in Rust.
    Native(&'a dyn Rule),
}

impl Source<'_> {
    fn meta(&self) -> Result<RuleMeta> {
        match self {
            Self::Sql(r) => r.meta(),
            Self::Native(r) => Ok(r.meta()),
        }
    }
}

/// A finding with the level and baseline status it was reported under.
#[derive(Debug, Clone)]
pub struct Reported {
    /// The finding; `rule` and `severity` are set.
    pub finding: Finding,
    /// Level of the finding's rule.
    pub level: Level,
    /// The baseline covers the finding.
    pub baselined: bool,
}

/// Summary of one rule that ran.
#[derive(Debug, Clone)]
pub struct RuleRun {
    /// Description of the rule.
    pub meta: RuleMeta,
    /// Configured level.
    pub level: Level,
    /// Baseline entities the rule no longer reports.
    pub fixed: Vec<String>,
}

/// Everything one `check` produced.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Rules that ran, in order.
    pub runs: Vec<RuleRun>,
    /// Findings of all rules, in rule order.
    pub findings: Vec<Reported>,
    /// Rules not run, with the reason.
    pub skipped: Vec<(String, String)>,
}

impl Outcome {
    /// Findings of gated rules that the baseline does not cover.
    #[must_use]
    pub fn gate_failures(&self) -> usize {
        self.findings
            .iter()
            .filter(|r| r.level == Level::Gate && !r.baselined)
            .count()
    }
}

/// Options of one evaluation.
pub struct EvalOptions<'a> {
    /// Config levels and parameters.
    pub cfg: &'a Config,
    /// `--param` overrides.
    pub overrides: &'a BTreeMap<String, String>,
    /// Run only rules whose id contains this text.
    pub only: Option<&'a str>,
}

/// Run `sources` over `facts` and compare the findings with `baseline`.
///
/// # Errors
/// Fails on duplicate rule ids, invalid rule metadata or levels, SQL errors and rule errors.
pub fn evaluate(
    facts: &Facts,
    sources: &[Source],
    baseline: &Baseline,
    o: &EvalOptions,
) -> Result<Outcome> {
    let mut out = Outcome::default();
    let mut ids = BTreeSet::new();
    for s in sources {
        let meta = s.meta()?;
        if !ids.insert(meta.id.clone()) {
            bail!("duplicate rule id {}", meta.id);
        }
        if o.only.is_some_and(|f| !meta.id.contains(f)) {
            continue;
        }
        let level = o.cfg.level(&meta.id)?;
        if level == Level::Off {
            out.skipped.push((meta.id.clone(), "level off".into()));
            continue;
        }
        if meta.needs_dynamic && !facts.has_dynamic() {
            out.skipped.push((
                meta.id.clone(),
                "needs dynamic facts; none extracted".into(),
            ));
            continue;
        }
        let mut values: BTreeMap<String, String> = meta.params.iter().cloned().collect();
        values.extend(o.cfg.rule_params(&meta.id));
        for (k, v) in o.overrides {
            if values.contains_key(k) || matches!(s, Source::Sql(_)) {
                values.insert(k.clone(), v.clone());
            }
        }
        let mut found = match s {
            Source::Sql(r) => {
                let ov: BTreeMap<String, String> = values.clone();
                rules::run_rule(facts.connection(), r, &ov)?
            }
            Source::Native(r) => {
                let mut f = Findings::default();
                let params = Params(values);
                r.check(
                    &Cx {
                        facts,
                        params: &params,
                    },
                    &mut f,
                )
                .map_err(|e| e.context(format!("rule {}", meta.id)))?;
                f.0
            }
        };
        for f in &mut found {
            f.rule.clone_from(&meta.id);
            if level == Level::Note {
                f.severity = "note".into();
            } else if f.severity.is_empty() {
                f.severity = meta.severity.as_str().into();
            }
        }
        found.sort_by(|a, b| {
            b.metric
                .partial_cmp(&a.metric)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.file.cmp(&b.file))
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.entity.cmp(&b.entity))
        });
        let fixed = baseline.fixed(&meta.id, &found);
        for f in found {
            let baselined = baseline.covers(&meta.id, meta.ratchet, &f);
            out.findings.push(Reported {
                finding: f,
                level,
                baselined,
            });
        }
        out.runs.push(RuleRun { meta, level, fixed });
    }
    Ok(out)
}

/// The baseline after `--update-baseline`: entries of the rules that ran replace the old ones.
#[must_use]
pub fn updated_baseline(old: &Baseline, out: &Outcome, verus_commit: &str) -> Baseline {
    let mut b = old.clone();
    b.verus_commit = verus_commit.to_string();
    for run in &out.runs {
        let now: Vec<Finding> = out
            .findings
            .iter()
            .filter(|r| r.finding.rule == run.meta.id)
            .map(|r| r.finding.clone())
            .collect();
        if run.meta.ratchet != Ratchet::None {
            b.record(&run.meta.id, run.meta.ratchet, &now);
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{CrateInfo, Db};
    use crate::vir::{ImplNames, parse_log};

    struct Two;
    impl Rule for Two {
        fn meta(&self) -> RuleMeta {
            RuleMeta::new("t/two", "two findings")
                .ratchet(Ratchet::Set)
                .param("n", 2)
        }
        fn check(&self, cx: &Cx, out: &mut Findings) -> Result<()> {
            let n = usize::try_from(cx.params.get_u64("n")?)?;
            for f in cx.facts.functions().iter().take(n) {
                out.push(Finding::at(f, "listed"));
            }
            Ok(())
        }
    }

    fn facts() -> Facts {
        let text = include_str!("../tests/fixtures/mini.vir");
        let parsed = parse_log(text, "mini", &ImplNames::new()).unwrap();
        let mut db = Db::in_memory().unwrap();
        db.load_crate(
            &parsed,
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
    fn levels_baseline_and_update() {
        let facts = facts();
        let cfg = Config::parse("[rules.levels]\n\"t/two\" = \"gate\"\n").unwrap();
        let none = BTreeMap::new();
        let o = EvalOptions {
            cfg: &cfg,
            overrides: &none,
            only: None,
        };
        let src = [Source::Native(&Two)];
        let first = evaluate(&facts, &src, &Baseline::default(), &o).unwrap();
        assert_eq!(first.findings.len(), 2);
        assert_eq!(first.gate_failures(), 2);
        let base = updated_baseline(&Baseline::default(), &first, "c");
        let again = evaluate(&facts, &src, &base, &o).unwrap();
        assert_eq!(again.gate_failures(), 0);
        assert!(again.findings.iter().all(|f| f.baselined));
        // A parameter override adds a finding the baseline does not cover.
        let ov = BTreeMap::from([("n".to_string(), "3".to_string())]);
        let more = evaluate(
            &facts,
            &src,
            &base,
            &EvalOptions {
                overrides: &ov,
                ..o
            },
        )
        .unwrap();
        assert_eq!(more.gate_failures(), 1);
        // A smaller result reports the dropped entity as fixed.
        let ov = BTreeMap::from([("n".to_string(), "1".to_string())]);
        let fewer = evaluate(
            &facts,
            &src,
            &base,
            &EvalOptions {
                overrides: &ov,
                ..o
            },
        )
        .unwrap();
        assert_eq!(fewer.runs[0].fixed.len(), 1);
    }

    #[test]
    fn off_and_duplicates() {
        let facts = facts();
        let cfg = Config::parse("[rules.levels]\n\"t/two\" = \"off\"\n").unwrap();
        let none = BTreeMap::new();
        let o = EvalOptions {
            cfg: &cfg,
            overrides: &none,
            only: None,
        };
        let r = evaluate(&facts, &[Source::Native(&Two)], &Baseline::default(), &o).unwrap();
        assert!(r.findings.is_empty() && r.skipped.len() == 1);
        let dup = [Source::Native(&Two), Source::Native(&Two)];
        assert!(evaluate(&facts, &dup, &Baseline::default(), &o).is_err());
    }
}
