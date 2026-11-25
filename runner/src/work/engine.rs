//! The engine: takes a request across the boundary and returns one reply.
//!
//! intake (normalizer agent for sentences, then deterministic normalize and admit)
//! → outer plan (planner agent, or one worker) → worker manifests, each checked for narrowing
//! → worker agents → merge → coverage and checks → reply.

use super::agents::{self, RunUsage};
use super::assemble::{assemble, preconditions, WorkOutput};
use super::checks::Evaluator;
use super::context::{Estimator, NoEstimate, Policy, Resolver};
use super::delegate::{delegate, Spent, Target};
use super::inputs::{files_under, InputStore, Store};
use super::jcs;
use super::kind::{Entry, Lookup, Registry};
use super::manifest::{duration_secs, format_duration, Manifest, ReplyMode};
use super::normalize::{intake, Ctx, Intake};
use super::reply::{Failure, FailureCode, Normalizer, Reply, Status, Usage};
use crate::orchestrator::Orchestrator;
use crate::spec::AgentSpec;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `work` in the orchestrator config file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkConfig {
    pub kinds_dir: PathBuf,
    /// This orchestrator's own principal, used as `by` on the manifests it hands on.
    #[serde(default = "default_principal")]
    pub principal: String,
    #[serde(default)]
    pub resolve: Resolver,
    #[serde(default)]
    pub policy: Policy,
    /// Pinned reference → fixture (`.yaml`) or checked-out directory.
    #[serde(default)]
    pub inputs: BTreeMap<String, PathBuf>,
    #[serde(default = "default_price")]
    pub usd_per_1k_tokens: f64,
    #[serde(default = "default_max_workers")]
    pub max_workers: usize,
    #[serde(default = "default_worker_turns")]
    pub worker_turns: u32,
    #[serde(default)]
    pub roles: RolesConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RolesConfig {
    /// Absent: sentences are refused as unsupported; callers must send `kind`.
    pub normalizer: Option<AgentSpec>,
    /// Absent: every manifest runs as one worker.
    pub planner: Option<AgentSpec>,
    #[serde(default)]
    pub worker: AgentSpec,
}

fn default_principal() -> String {
    "orchestrator:local".into()
}
fn default_price() -> f64 {
    0.003
}
fn default_max_workers() -> usize {
    4
}
fn default_worker_turns() -> u32 {
    30
}

/// Share of the remaining budget offered to the planner for its workers; the rest covers the
/// planner's own call.
const PLANNER_OFFER: f64 = 0.8;

/// Most file text handed to one worker, in bytes.
const CONTENTS_CAP: usize = 200_000;

pub struct Engine {
    pub registry: Registry,
    pub resolver: Resolver,
    pub policy: Policy,
    pub estimator: Box<dyn Estimator>,
    pub store: Arc<dyn InputStore>,
    pub eval: Evaluator,
    pub orch: Arc<Orchestrator>,
    pub roles: RolesConfig,
    pub normalizer: Normalizer,
    pub principal: String,
    pub usd_per_1k_tokens: f64,
    pub max_workers: usize,
    pub worker_turns: u32,
    receipts: Mutex<HashMap<String, Option<Reply>>>,
}

impl Engine {
    pub fn new(orch: Arc<Orchestrator>, cfg: &WorkConfig, default_spec: &AgentSpec) -> anyhow::Result<Arc<Engine>> {
        let registry = Registry::load_dir(&cfg.kinds_dir)?;
        let store: Arc<dyn InputStore> = Arc::new(Store::from_map(&cfg.inputs)?);
        let mut roles = cfg.roles.clone();
        roles.normalizer = roles.normalizer.map(|s| s.with_defaults(default_spec));
        roles.planner = roles.planner.map(|s| s.with_defaults(default_spec));
        roles.worker = roles.worker.with_defaults(default_spec);
        Ok(Arc::new(Engine {
            registry,
            resolver: cfg.resolve.clone(),
            policy: cfg.policy.clone(),
            estimator: Box::new(NoEstimate),
            eval: Evaluator::new(store.clone()),
            store,
            orch,
            roles,
            normalizer: Normalizer { id: "taffee-normalizer".into(), version: env!("CARGO_PKG_VERSION").into() },
            principal: cfg.principal.clone(),
            usd_per_1k_tokens: cfg.usd_per_1k_tokens,
            max_workers: cfg.max_workers.max(1),
            worker_turns: cfg.worker_turns.max(1),
            receipts: Mutex::new(HashMap::new()),
        }))
    }

    pub fn ctx(&self) -> Ctx<'_> {
        Ctx { registry: &self.registry, resolver: &self.resolver, policy: &self.policy, estimator: self.estimator.as_ref(), normalizer: self.normalizer.clone() }
    }

    fn usd(&self, tokens: u64) -> f64 {
        ((tokens as f64) * self.usd_per_1k_tokens / 1000.0 * 1e6).round() / 1e6
    }

    /// One request in, one reply out (or ACCEPTED with a receipt). Logs the start and the end of
    /// every request (who, status, time, cost; never its content).
    pub async fn submit(self: &Arc<Self>, body: Value, principal: &str) -> Reply {
        let id = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        let t = Instant::now();
        tracing::info!(request = %id, principal, "work request");
        let r = self.submit_inner(body, principal, &id).await;
        let u = r.usage.as_ref();
        tracing::info!(
            request = %id, status = ?r.status, secs = t.elapsed().as_secs_f64(),
            usd = u.and_then(|u| u.usd), model_calls = u.and_then(|u| u.model_calls),
            trace = r.trace.as_deref().unwrap_or(""), "work reply"
        );
        r
    }

    async fn submit_inner(self: &Arc<Self>, body: Value, principal: &str, id: &str) -> Reply {
        let req = match Manifest::from_value(body) {
            Ok(m) => m,
            Err(i) => {
                let mut r = Reply::refused(super::reply::Reason::Invalid, i.rule, i.detail);
                r.normalized_by = Some(self.normalizer.clone());
                return r;
            }
        };
        let started = Instant::now();
        let mut proposal = None;
        let mut prior = RunUsage::default();
        if req.kind.is_none() && !req.is_frozen() {
            if let (Some(want), Some(tpl)) = (req.want.as_deref(), self.roles.normalizer.as_ref()) {
                let (p, u) = agents::propose(&self.orch, tpl, &self.registry, want, principal).await;
                tracing::info!(request = %id, secs = started.elapsed().as_secs_f64(), ok = p.is_ok(), "normalized");
                prior = u;
                match p {
                    Ok(p) => proposal = Some(p),
                    Err(e) => {
                        let mut r = Reply::refused(super::reply::Reason::Unsupported, "WM-NRM-06", format!("the request could not be read: {e}"));
                        r.normalized_by = Some(self.normalizer.clone());
                        return r;
                    }
                }
            }
        }
        let m = match intake(&self.ctx(), req, principal, proposal) {
            Intake::Reply(r) => return *r,
            Intake::Run(m) => *m,
        };
        if m.reply == Some(ReplyMode::Receipt) {
            let receipt = format!("rcpt-{}", uuid::Uuid::new_v4().simple());
            self.receipts.lock().unwrap().insert(receipt.clone(), None);
            let me = self.clone();
            let id = receipt.clone();
            let digest = m.digest.clone().unwrap_or_default();
            let assumed = m.assumed.clone();
            tokio::spawn(async move {
                let r = me.execute_with(m, prior, started).await;
                me.receipts.lock().unwrap().insert(id, Some(r));
            });
            let mut r = Reply::accepted(&digest, &receipt);
            r.normalized_by = Some(self.normalizer.clone());
            r.assumed = assumed;
            return r;
        }
        self.execute_with(m, prior, started).await
    }

    /// The final reply for a receipt; ACCEPTED while the work is still running.
    pub fn receipt(&self, id: &str) -> Option<Reply> {
        let receipts = self.receipts.lock().unwrap();
        match receipts.get(id)? {
            Some(r) => Some(r.clone()),
            None => {
                let mut r = Reply::new(Status::Accepted);
                r.receipt = Some(id.to_string());
                Some(r)
            }
        }
    }

    fn entry(&self, m: &Manifest) -> Option<&Entry> {
        match self.registry.lookup(m.kind.as_deref()?) {
            Lookup::Found(e) => Some(e),
            _ => None,
        }
    }

    /// Files the work covers, when the kind declares coverage.
    fn files(&self, entry: &Entry, m: &Manifest) -> Vec<String> {
        let Some(cov) = &entry.kind.coverage else { return vec![] };
        let repo = m.given.as_ref().and_then(|g| g.get(&cov.given));
        let globs: Vec<String> = m
            .parameters
            .as_ref()
            .and_then(|p| p.get(&cov.paths))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        repo.and_then(|r| files_under(self.store.as_ref(), r, &globs)).unwrap_or_default()
    }

    /// The text of the files a worker searches, as `file:line: text` lines read from the pinned
    /// inputs, capped at CONTENTS_CAP bytes (empty lines left out), and the files whose text was
    /// given in full.
    fn contents(&self, entry: &Entry, m: &Manifest, files: &[String]) -> (String, Vec<String>) {
        let Some(cov) = &entry.kind.coverage else { return (String::new(), vec![]) };
        let Some(repo) = m.given.as_ref().and_then(|g| g.get(&cov.given)) else { return (String::new(), vec![]) };
        let mut out = String::new();
        let mut given = Vec::new();
        'files: for f in files {
            let n = self.store.line_count(repo, f).unwrap_or(0);
            for i in 1..=n {
                if let Some(text) = self.store.line(repo, f, i).filter(|t| !t.trim().is_empty()) {
                    let line = format!("{f}:{i}: {text}\n");
                    if out.len() + line.len() > CONTENTS_CAP {
                        out.push_str("(truncated)\n");
                        break 'files;
                    }
                    out.push_str(&line);
                }
            }
            given.push(f.clone());
        }
        (out, given)
    }

    /// Run a frozen manifest to its reply.
    pub async fn execute(self: &Arc<Self>, m: Manifest) -> Reply {
        self.execute_with(m, RunUsage::default(), Instant::now()).await
    }

    /// `prior` is what was already spent on this request (the normalizer), and `started` is when
    /// the request arrived: the reply's usage covers the whole request, not only the work.
    async fn execute_with(self: &Arc<Self>, m: Manifest, prior: RunUsage, started: Instant) -> Reply {
        let mut trace: Vec<String> = Vec::new();
        let Some(entry) = self.entry(&m).cloned() else {
            let mut r = Reply::new(Status::Failed);
            r.failure = Some(Failure { code: FailureCode::Internal, detail: Some("kind not available".into()) });
            r.digest = m.digest.clone();
            return r;
        };
        // Preconditions before any work (WM-RES-04).
        if !matches!(preconditions(&self.eval, &m, &[]), Ok(ref missing) if missing.is_empty()) {
            let r = assemble(&self.registry, self.store.as_ref(), &self.eval, &m, None, &[]);
            return self.finish(r, &m);
        }
        let limit_secs = m.limits.as_ref().and_then(|l| l.time.as_deref()).and_then(duration_secs).unwrap_or(600);
        let run = tokio::time::timeout(Duration::from_secs(limit_secs), self.clone().run_inner(m.clone(), entry, self.usd(prior.tokens), &mut trace)).await;
        let mut work = match run {
            Ok(w) => w,
            Err(_) => WorkOutput { failure: Some(Failure { code: FailureCode::Timeout, detail: Some(format!("limits.time {} reached", format_duration(limit_secs))) }), ..Default::default() },
        };
        let tokens = work.usage.tokens.unwrap_or(0) + prior.tokens;
        work.usage.tokens = Some(tokens);
        work.usage.model_calls = Some(work.usage.model_calls.unwrap_or(0) + prior.model_calls);
        work.usage.usd = Some(self.usd(tokens));
        work.usage.time = Some(format_duration(started.elapsed().as_secs().max(1)));
        let limit_usd = m.limits.as_ref().and_then(|l| l.usd).unwrap_or(0.0);
        if work.failure.is_none() && work.usage.usd.unwrap_or(0.0) > limit_usd + 1e-9 {
            work.failure = Some(Failure { code: FailureCode::BudgetExhausted, detail: Some(format!("used ${:.4} of ${limit_usd}", work.usage.usd.unwrap_or(0.0))) });
        }
        let mut reply = assemble(&self.registry, self.store.as_ref(), &self.eval, &m, Some(work), &[]);
        if !trace.is_empty() {
            reply.trace = Some(trace.join("; "));
        }
        self.finish(reply, &m)
    }

    /// Every reply after normalization names the normalizer and carries `assumed`
    /// (WM-NRM-10, WM-RES-07).
    fn finish(&self, mut reply: Reply, m: &Manifest) -> Reply {
        reply.normalized_by = Some(self.normalizer.clone());
        reply.assumed = m.assumed.clone();
        reply
    }

    /// `prior_usd`: already spent on this request before the work (the normalizer).
    async fn run_inner(self: Arc<Self>, m: Manifest, entry: Entry, prior_usd: f64, trace: &mut Vec<String>) -> WorkOutput {
        let files = self.files(&entry, &m);
        let mut usage = RunUsage::default();
        let t = Instant::now();
        let workers = self.plan_workers(&m, &entry, &files, prior_usd, &mut usage, trace).await;
        trace.push(format!("workers={}", workers.len()));
        tracing::info!(digest = m.digest.as_deref().unwrap_or(""), workers = workers.len(), secs = t.elapsed().as_secs_f64(), "planned");

        let schema = m.deliver.as_ref().and_then(|d| self.registry.content(&d.schema)).cloned().unwrap_or(json!({ "type": "object" }));
        let mut set = tokio::task::JoinSet::new();
        for wm in workers {
            let me = self.clone();
            let entry = entry.clone();
            let schema = schema.clone();
            set.spawn(async move {
                let files = me.files(&entry, &wm);
                let usd = wm.limits.as_ref().and_then(|l| l.usd).unwrap_or(0.0);
                let tokens = ((usd / me.usd_per_1k_tokens) * 1000.0) as u64;
                let tpl = me.roles.worker.clone();
                let (contents, given) = me.contents(&entry, &wm, &files);
                let mut r = agents::work(&me.orch, &tpl, &wm, &entry, &schema, &files, &contents, me.worker_turns, tokens, &me.principal).await;
                // The host handed this worker the full text of `given`; when the worker answered,
                // those files were searched, whatever its own list says (WM-CHK-06).
                if r.error.is_none() && r.answer.is_some() {
                    for f in given {
                        if !r.searched.contains(&f) {
                            r.searched.push(f);
                        }
                    }
                }
                r
            });
        }
        let mut results = Vec::new();
        while let Some(r) = set.join_next().await {
            match r {
                Ok(r) => {
                    tracing::info!(digest = m.digest.as_deref().unwrap_or(""), answered = r.answer.is_some(), error = r.error.as_ref().map(|e| e.0.as_str()).unwrap_or(""), "worker done");
                    results.push(r);
                }
                Err(e) => tracing::warn!(error = %e, "worker task failed"),
            }
        }

        let mut out = WorkOutput::default();
        let mut searched: Vec<String> = Vec::new();
        let mut answers: Vec<Value> = Vec::new();
        let mut errors: Vec<(String, bool)> = Vec::new();
        for r in results {
            usage += r.usage;
            if let Some(e) = r.error {
                errors.push(e);
                continue;
            }
            for f in r.searched {
                if !searched.contains(&f) {
                    searched.push(f);
                }
            }
            if let Some(a) = r.answer {
                answers.push(a);
            }
            out.proposals.extend(r.proposals);
        }
        for (e, _) in &errors {
            trace.push(format!("worker failed: {e}"));
        }
        out.usage = Usage { usd: Some(self.usd(usage.tokens)), time: None, tokens: Some(usage.tokens), model_calls: Some(usage.model_calls) };
        if answers.is_empty() {
            let timeout = errors.iter().any(|(_, t)| *t);
            out.failure = Some(Failure {
                code: if timeout { FailureCode::Timeout } else { FailureCode::Internal },
                detail: Some(errors.first().map(|(e, _)| e.clone()).unwrap_or_else(|| "no worker returned an answer".into())),
            });
            return out;
        }
        out.answer = Some(merge(&answers, m.deliver.as_ref().and_then(|d| d.order.clone()).unwrap_or_default()));
        out.searched = Some(searched);
        out
    }

    /// The outer plan: worker manifests, each checked for narrowing. Falls back to one worker
    /// carrying the whole manifest if the planner is absent, fails, or proposes anything that
    /// does not narrow.
    async fn plan_workers(&self, m: &Manifest, entry: &Entry, files: &[String], prior_usd: f64, usage: &mut RunUsage, trace: &mut Vec<String>) -> Vec<Manifest> {
        let ctx = self.ctx();
        let base = |parameters: Option<Map<String, Value>>, usd: Option<f64>, time: Option<String>| Manifest {
            manifest: Some(Value::String(super::manifest::VERSION.into())),
            kind: m.kind.clone(),
            why: m.why.clone(),
            parameters: parameters.or_else(|| m.parameters.clone()),
            given: m.given.clone(),
            decide: m.decide.clone(),
            partial_ok: m.partial_ok,
            approved: m.approved,
            limits: Some(super::manifest::Limits { effects: None, usd, time, tokens: None }),
            ..Default::default()
        };
        // One worker carrying the whole manifest. If even that hand-over is refused (for example
        // by the orchestrator's own policy), say why instead of running nothing silently.
        let single = |trace: &mut Vec<String>| match delegate(&ctx, m, Spent::default(), base(None, None, None), &self.principal, Target::Worker) {
            Ok(w) => vec![w],
            Err(v) => {
                trace.push(format!("could not hand the work to a worker: {}", v.join(", ")));
                vec![]
            }
        };

        let (Some(tpl), Some(cov)) = (self.roles.planner.as_ref(), entry.kind.coverage.as_ref()) else {
            return single(trace);
        };
        let l = m.limits.clone().unwrap_or_default();
        let secs = l.time.as_deref().and_then(duration_secs).unwrap_or(0);
        // Offer the planner what is left after the normalizer, minus a reserve for the planner's
        // own call, so a plan that uses all of its offer still fits (WM-DLG-02).
        let budget = l.usd.unwrap_or(0.0);
        let offer = ((budget - prior_usd).max(0.0) * PLANNER_OFFER * 100.0).floor() / 100.0;
        let (planned, u) = agents::plan(&self.orch, tpl, m, entry, &cov.paths, files, offer, secs, self.max_workers, &self.principal).await;
        *usage += u;
        let spent = Spent { usd: prior_usd + self.usd(u.tokens), secs: 0 };
        let planned = match planned {
            Ok(p) => p,
            Err(e) => {
                trace.push(format!("plan fell back to one worker: {e}"));
                return single(trace);
            }
        };
        let mut children = Vec::new();
        let mut problems = Vec::new();
        if planned.len() > self.max_workers {
            problems.push(format!("{} workers is more than {}", planned.len(), self.max_workers));
        }
        let total: f64 = planned.iter().map(|w| w.usd).sum();
        let remaining = budget - spent.usd;
        if total > remaining + 1e-9 {
            problems.push(format!("WM-DLG-02: the workers' usd adds up to {total:.2}, more than the {remaining:.2} left"));
        }
        for w in &planned {
            let mut params = m.parameters.clone().unwrap_or_default();
            for (k, v) in &w.parameters {
                // Repeating the parent's value is fine (models often copy every parameter);
                // only a different value is a change.
                if k != &cov.paths && m.parameters.as_ref().and_then(|p| p.get(k)) != Some(v) {
                    let p = format!("a worker changed `{k}`, which it may not narrow");
                    if !problems.contains(&p) {
                        problems.push(p);
                    }
                }
                params.insert(k.clone(), v.clone());
            }
            match delegate(&ctx, m, spent, base(Some(params), Some(w.usd), Some(w.time.clone())), &self.principal, Target::Worker) {
                Ok(c) => children.push(c),
                Err(v) => problems.push(v.join(",")),
            }
        }
        // Together the workers must cover every file the parent covers (WM-CHK-06): a plan that
        // leaves files out would end in a failed `complete` check after all the work was paid for.
        if problems.is_empty() {
            let covered: std::collections::BTreeSet<String> = children.iter().flat_map(|c| self.files(entry, c)).collect();
            let left_out: Vec<&String> = files.iter().filter(|f| !covered.contains(*f)).collect();
            if !left_out.is_empty() {
                let shown: Vec<&str> = left_out.iter().take(5).map(|s| s.as_str()).collect();
                problems.push(format!("the workers leave out {} file(s): {}", left_out.len(), shown.join(", ")));
            }
        }
        if !problems.is_empty() {
            trace.push(format!("plan rejected ({}); one worker", problems.join("; ")));
            return single(trace);
        }
        trace.push(format!("planned {} workers", children.len()));
        children
    }
}

/// Merge worker answers: list fields are concatenated, duplicates dropped, then sorted by
/// `deliver.order`; other fields come from the first answer.
pub fn merge(answers: &[Value], order: Vec<String>) -> Value {
    if answers.len() == 1 && order.is_empty() {
        return answers[0].clone();
    }
    let mut out = Map::new();
    for a in answers {
        let Some(obj) = a.as_object() else { continue };
        for (k, v) in obj {
            match (out.get_mut(k), v) {
                (Some(Value::Array(acc)), Value::Array(items)) => {
                    for i in items {
                        if !acc.iter().any(|x| jcs::canonical(x) == jcs::canonical(i)) {
                            acc.push(i.clone());
                        }
                    }
                }
                (None, _) => {
                    out.insert(k.clone(), v.clone());
                }
                _ => {}
            }
        }
    }
    for v in out.values_mut() {
        if let Value::Array(items) = v {
            items.sort_by(|a, b| {
                for key in &order {
                    let (x, y) = (a.get(key), b.get(key));
                    let o = match (x, y) {
                        (Some(Value::Number(p)), Some(Value::Number(q))) => p.as_f64().partial_cmp(&q.as_f64()).unwrap_or(std::cmp::Ordering::Equal),
                        (Some(p), Some(q)) => jcs::canonical(p).cmp(&jcs::canonical(q)),
                        _ => std::cmp::Ordering::Equal,
                    };
                    if o != std::cmp::Ordering::Equal {
                        return o;
                    }
                }
                std::cmp::Ordering::Equal
            });
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_dedupes_and_orders() {
        let a = json!({ "breaks": [ { "file": "b.ts", "line": 2 }, { "file": "a.ts", "line": 9 } ], "mentions": [] });
        let b = json!({ "breaks": [ { "file": "a.ts", "line": 9 }, { "file": "a.ts", "line": 1 } ], "mentions": [ { "file": "d.md", "line": 1 } ] });
        let m = merge(&[a, b], vec!["file".into(), "line".into()]);
        assert_eq!(m["breaks"], json!([ { "file": "a.ts", "line": 1 }, { "file": "a.ts", "line": 9 }, { "file": "b.ts", "line": 2 } ]));
        assert_eq!(m["mentions"], json!([ { "file": "d.md", "line": 1 } ]));
    }
}
