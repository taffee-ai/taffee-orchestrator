//! From what the workers returned to the reply: preconditions, coverage, checks and the
//! status rules. Deterministic; the conformance `result` vectors drive this directly.

use super::checks::{deliver_check, deliver_problem, variables, Evaluator};
use super::inputs::{files_under, InputStore};
use super::kind::{Lookup, Registry};
use super::manifest::Manifest;
use super::reply::{judge, CheckStatus, Coverage, Failure, FailureCode, Outcome, Reply, Usage};
use serde_json::{json, Value};

/// What came back from the inner loop.
#[derive(Debug, Clone, Default)]
pub struct WorkOutput {
    pub answer: Option<Value>,
    /// Files the workers say they searched.
    pub searched: Option<Vec<String>>,
    pub usage: Usage,
    pub proposals: Vec<Value>,
    pub failure: Option<Failure>,
}

/// Evaluate `requires`. Ok(missing ids), or Err if a precondition could not be evaluated.
pub fn preconditions(eval: &Evaluator, m: &Manifest, unavailable: &[String]) -> Result<Vec<String>, String> {
    let vars = variables(m, None, None, &json!({}));
    let mut missing = Vec::new();
    for (id, status) in eval.eval_all(m.requires.as_ref().unwrap_or(&Default::default()), &vars, unavailable) {
        match status {
            CheckStatus::Pass => {}
            CheckStatus::Fail => missing.push(id),
            CheckStatus::Indeterminate => return Err(format!("requires.{id} could not be evaluated")),
        }
    }
    Ok(missing)
}

/// WM-CHK-06: the orchestrator lists the files itself.
pub fn coverage(registry: &Registry, store: &dyn InputStore, m: &Manifest, searched: &[String]) -> Option<Coverage> {
    let Lookup::Found(entry) = registry.lookup(m.kind.as_deref()?) else { return None };
    let cov = entry.kind.coverage.as_ref()?;
    let repo = m.given.as_ref()?.get(&cov.given)?;
    let globs: Vec<String> = m.parameters.as_ref()?.get(&cov.paths)?.as_array()?.iter().filter_map(|v| v.as_str().map(String::from)).collect();
    let files = files_under(store, repo, &globs)?;
    let hit: Vec<&String> = files.iter().filter(|f| searched.contains(f)).collect();
    let missed: Vec<String> = files.iter().filter(|f| !searched.contains(f)).cloned().collect();
    Some(Coverage { searched: hit.len() as u64, total: files.len() as u64, missed })
}

pub fn assemble(registry: &Registry, store: &dyn InputStore, eval: &Evaluator, m: &Manifest, work: Option<WorkOutput>, unavailable: &[String]) -> Reply {
    let mut o = Outcome::default();
    match preconditions(eval, m, unavailable) {
        Ok(missing) if !missing.is_empty() => {
            o.missing = missing;
            return judge(m, o);
        }
        Ok(_) => {}
        Err(e) => {
            o.failure = Some(Failure { code: FailureCode::InputUnavailable, detail: Some(e) });
            return judge(m, o);
        }
    }
    let Some(w) = work else {
        o.failure = Some(Failure { code: FailureCode::Internal, detail: Some("no work output".into()) });
        return judge(m, o);
    };
    o.usage = w.usage.clone();
    if let Some(f) = w.failure {
        o.failure = Some(f);
        return judge(m, o);
    }
    let cov = coverage(registry, store, m, w.searched.as_deref().unwrap_or(&[]));
    let cov_json = cov.as_ref().map(|c| serde_json::to_value(c).unwrap());
    let usage_json = serde_json::to_value(&w.usage).unwrap_or(json!({}));
    let schema = m.deliver.as_ref().and_then(|d| registry.content(&d.schema));
    let mut checks = std::collections::BTreeMap::new();
    checks.insert(
        "deliver".to_string(),
        match schema {
            Some(s) => deliver_check(s, w.answer.as_ref()),
            None => CheckStatus::Indeterminate,
        },
    );
    if checks.get("deliver") == Some(&CheckStatus::Fail) {
        o.detail = w.answer.as_ref().and_then(super::agents::stringified_problem).or_else(|| schema.and_then(|s| deliver_problem(s, w.answer.as_ref())));
    }
    let vars = variables(m, w.answer.as_ref(), cov_json.as_ref(), &usage_json);
    checks.extend(eval.eval_all(m.checks.as_ref().unwrap_or(&Default::default()), &vars, unavailable));
    o.checks = checks;
    o.answer = w.answer;
    o.coverage = cov;
    // Proposals come back as sparse manifests, never run here (WM-RES-08).
    o.proposals = w
        .proposals
        .into_iter()
        .filter_map(|p| Manifest::from_value(p).ok())
        .filter(|p| p.check_structure().is_ok())
        .map(|mut p| {
            p.digest = None;
            p
        })
        .collect();
    judge(m, o)
}
