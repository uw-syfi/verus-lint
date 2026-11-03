//! `--time-expanded --output-json` report of this format generation to a normalized `Report`.

use crate::facts::{FnCost, ModuleCost, Report};
use serde_json::Value;
use std::collections::HashMap;

/// Normalize one report object. The caller has checked the release in its `verus` section.
pub fn parse(obj: &Value) -> Result<Report, String> {
    let v = &obj["verus"];
    let s = |k: &str| v[k].as_str().map(String::from);
    let (Some(version), Some(commit)) = (s("version"), s("commit")) else {
        return Err("report has no verus.version and verus.commit".into());
    };
    let smt = obj["times-ms"]["smt"]["smt-run-module-times"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let mut functions = Vec::new();
    for m in smt {
        let module = m["module"].as_str().unwrap_or("");
        for f in m["function-breakdown"].as_array().into_iter().flatten() {
            let Some(name) = f["function"].as_str() else {
                continue;
            };
            functions.push(FnCost {
                module: module.to_string(),
                function: name.to_string(),
                // This generation spells the key "mode:" (with the colon).
                mode: f["mode:"]
                    .as_str()
                    .or_else(|| f["mode"].as_str())
                    .map(String::from),
                rlimit: f["rlimit"].as_i64().unwrap_or(0),
                time_us: f["time-micros"].as_i64().unwrap_or(0),
                success: f["success"].as_bool().unwrap_or(true),
            });
        }
    }
    // A module with spun-off functions has several session entries under one name; the longest
    // one bounds the module's wall time.
    let mut session: HashMap<&str, i64> = HashMap::new();
    for m in obj["times-ms"]["total-verify-module-times"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let e = session
            .entry(m["module"].as_str().unwrap_or(""))
            .or_insert(0);
        *e = (*e).max(m["time"].as_i64().unwrap_or(0));
    }
    let modules = smt
        .iter()
        .map(|m| {
            let name = m["module"].as_str().unwrap_or("");
            ModuleCost {
                module: name.to_string(),
                rlimit: m["rlimit"].as_i64().unwrap_or(0),
                smt_time_ms: m["time"].as_i64().unwrap_or(0),
                session_time_ms: session.get(name).copied().unwrap_or(0),
            }
        })
        .collect();
    Ok(Report {
        version,
        commit,
        functions,
        modules,
    })
}
