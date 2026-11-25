//! Runs every conformance vector in docs/spec/work-manifest/vectors/ against this
//! orchestrator, and checks that every rule proved by vectors is covered by passing ones.
//!
//! Sentence vectors go through the real normalizer role: strand-agent.wasm against the mock
//! model on 127.0.0.1:8000 (started by scripts/ci.sh). Everything else calls the deterministic
//! functions directly. The mock normalizer is deterministic, so each sentence vector runs once.

use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use taffee_orchestrator::pool::PoolConfigOptions;
use taffee_orchestrator::runtime::{Runtime, RuntimeOptions};
use taffee_orchestrator::work::agents;
use taffee_orchestrator::work::assemble::{assemble, WorkOutput};
use taffee_orchestrator::work::checks::Evaluator;
use taffee_orchestrator::work::context::{Estimator, Policy, PolicyEntry, Resolver};
use taffee_orchestrator::work::delegate::{delegate, Spent, Target};
use taffee_orchestrator::work::inputs::{Fixture, InputStore, Store};
use taffee_orchestrator::work::kind::{yaml_to_json, Registry};
use taffee_orchestrator::work::manifest::{duration_secs, Limits, Manifest, ReplyMode};
use taffee_orchestrator::work::normalize::{intake, Ctx, Intake};
use taffee_orchestrator::work::reply::{Failure, Normalizer, Reason, Reply, Usage};
use taffee_orchestrator::{AgentSpec, Orchestrator, OrchestratorConfig};

const SPEC: &str = "../docs/spec/work-manifest";
const WASM: &str = "../strand-agent/dist/strand-agent.wasm";
const MODEL: &str = "http://127.0.0.1:8000";

fn load(p: &Path) -> Value {
    yaml_to_json(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

struct FixedEstimate(Option<Limits>);
impl Estimator for FixedEstimate {
    fn estimate(&self, _: &Value) -> Option<Limits> {
        self.0.clone()
    }
}

struct Harness {
    registry: Registry,
    resolver: Resolver,
    policy: Policy,
    store: Arc<dyn InputStore>,
    eval: Evaluator,
    normalizer: Normalizer,
    orch: Arc<Orchestrator>,
    template: AgentSpec,
    outputs: BTreeMap<String, Value>,
}

fn build() -> Harness {
    let dir = PathBuf::from(SPEC).join("vectors");
    let ctx = load(&dir.join("context.yaml"));
    let mut registry = Registry::default();
    let mut base: Option<Value> = None;
    for item in ctx["registry"].as_array().unwrap() {
        match item {
            Value::String(path) => {
                let v = load(&dir.join(path));
                registry.insert(v.clone()).unwrap();
                base.get_or_insert(v);
            }
            stub => {
                let mut k = base.clone().expect("a full kind before the stubs");
                let o = k.as_object_mut().unwrap();
                o.insert("id".into(), stub["stub"].clone());
                o.insert("summary".into(), stub["want_template"].clone());
                o.insert("want_template".into(), stub["want_template"].clone());
                o.insert("given".into(), stub["given"].clone());
                o.insert("limits".into(), stub["limits"].clone());
                o.insert("examples".into(), json!([]));
                let names: Vec<&String> = stub["parameters"].as_object().unwrap().keys().collect();
                let props: Map<String, Value> = names.iter().map(|n| (n.to_string(), json!({ "type": "string" }))).collect();
                o.insert("parameters".into(), json!({ "type": "object", "required": names, "additionalProperties": false, "properties": props }));
                registry.insert(k).unwrap();
            }
        }
    }
    let r = &ctx["resolve"];
    let resolver = Resolver {
        names: serde_json::from_value(r["names"].clone()).unwrap(),
        versions: serde_json::from_value(r["versions"].clone()).unwrap(),
        default_version: "main".into(),
    };
    let mut store = Store::default();
    for (pinned, path) in r["repos"].as_object().unwrap() {
        let mut f = Fixture::load(&dir.join(path.as_str().unwrap())).unwrap();
        f.r#ref = pinned.clone();
        store.add_fixture(f);
    }
    let store: Arc<dyn InputStore> = Arc::new(store);
    let policy = Policy {
        principals: serde_json::from_value::<BTreeMap<String, PolicyEntry>>(ctx["policy"].clone()).unwrap(),
        default: PolicyEntry::default(),
    };
    let normalizer: Normalizer = serde_json::from_value(ctx["normalizer"].clone()).unwrap();

    let runtime = Runtime::load(&PathBuf::from(WASM), RuntimeOptions { pool: PoolConfigOptions { max_agents: 8, ..Default::default() }, ..Default::default() }).expect("load strand-agent.wasm");
    let mut cfg = OrchestratorConfig::default();
    cfg.component = PathBuf::from(WASM);
    cfg.default_spec.model.endpoint = Some(MODEL.into());
    let orch = Orchestrator::with_runtime(runtime, &cfg).unwrap();
    let mut template = AgentSpec::default();
    template.model.endpoint = Some(MODEL.into());
    Harness { registry, resolver, policy, eval: Evaluator::new(store.clone()), store, normalizer, orch, template, outputs: BTreeMap::new() }
}

impl Harness {
    fn lookup(&self, path: &str) -> Value {
        let mut parts = path.split('.');
        let id = parts.next().unwrap();
        let mut v = self.outputs.get(id).unwrap_or_else(|| panic!("no output for {id} (needed by {path})")).clone();
        for p in parts {
            v = v.get(p).cloned().unwrap_or(Value::Null);
        }
        v
    }

    /// Inputs: `{ $from, with }` and `{ $same }`.
    fn resolve(&self, v: &Value) -> Value {
        match v {
            Value::Object(o) if o.contains_key("$from") => {
                let mut base = self.lookup(o["$from"].as_str().unwrap());
                let with = o.get("with").map(|w| self.resolve(w)).unwrap_or(json!({}));
                let refreeze = base.get("digest").is_some() && with.get("digest").is_none();
                for (k, x) in with.as_object().unwrap() {
                    base[k] = x.clone();
                }
                if refreeze {
                    let mut m = Manifest::from_value(base).unwrap();
                    m.freeze();
                    base = m.to_value();
                }
                base
            }
            Value::Object(o) if o.contains_key("$same") => self.lookup(o["$same"].as_str().unwrap()),
            Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), self.resolve(x))).collect()),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.resolve(x)).collect()),
            x => x.clone(),
        }
    }

    fn matches(&self, exp: &Value, act: Option<&Value>, path: &str, errs: &mut Vec<String>) {
        let act_present = act.filter(|a| !a.is_null());
        match exp {
            Value::String(s) if s == "$any" => {
                if act_present.is_none() {
                    errs.push(format!("{path}: expected a value, got none"));
                }
            }
            Value::String(s) if s == "$absent" => {
                if let Some(a) = act_present {
                    errs.push(format!("{path}: expected nothing, got {a}"));
                }
            }
            Value::Object(o) if o.contains_key("$match") => {
                let re = Regex::new(o["$match"].as_str().unwrap()).unwrap();
                if !act_present.and_then(Value::as_str).is_some_and(|s| re.is_match(s)) {
                    errs.push(format!("{path}: {act:?} does not match {}", o["$match"]));
                }
            }
            Value::Object(o) if o.contains_key("$same") => {
                let want = self.lookup(o["$same"].as_str().unwrap());
                if act != Some(&want) {
                    errs.push(format!("{path}: expected {want}, got {act:?}"));
                }
            }
            Value::Object(o) if o.contains_key("$differs") => {
                let other = self.lookup(o["$differs"].as_str().unwrap());
                if act == Some(&other) || act_present.is_none() {
                    errs.push(format!("{path}: expected something other than {other}"));
                }
            }
            Value::Object(o) if o.contains_key("$contains") => {
                for item in o["$contains"].as_array().unwrap() {
                    let ok = match act_present {
                        Some(Value::Array(a)) => a.contains(item),
                        Some(Value::Object(m)) => item.as_str().is_some_and(|k| m.contains_key(k)),
                        _ => false,
                    };
                    if !ok {
                        errs.push(format!("{path}: does not contain {item}"));
                    }
                }
            }
            Value::Object(o) => match act_present {
                Some(Value::Object(_)) => {
                    for (k, e) in o {
                        self.matches(e, act_present.unwrap().get(k), &format!("{path}.{k}"), errs);
                    }
                }
                _ => errs.push(format!("{path}: expected an object, got {act:?}")),
            },
            Value::Array(items) => match act_present {
                Some(Value::Array(a)) if a.len() == items.len() => {
                    for (i, (e, x)) in items.iter().zip(a).enumerate() {
                        self.matches(e, Some(x), &format!("{path}[{i}]"), errs);
                    }
                }
                _ => errs.push(format!("{path}: expected {exp}, got {act:?}")),
            },
            Value::Number(n) => {
                if act_present.and_then(Value::as_f64).is_none_or(|x| (x - n.as_f64().unwrap()).abs() > 1e-9) {
                    errs.push(format!("{path}: expected {n}, got {act:?}"));
                }
            }
            e => {
                if act_present != Some(e) {
                    errs.push(format!("{path}: expected {e}, got {act:?}"));
                }
            }
        }
    }

    fn ctx<'a>(&'a self, est: &'a dyn Estimator) -> Ctx<'a> {
        Ctx { registry: &self.registry, resolver: &self.resolver, policy: &self.policy, estimator: est, normalizer: self.normalizer.clone() }
    }

    async fn admit(&self, v: &Value) -> Value {
        let principal = v["principal"].as_str().unwrap();
        let req = match Manifest::from_value(self.resolve(&v["request"])) {
            Ok(m) => m,
            Err(i) => return Reply::refused(Reason::Invalid, i.rule, i.detail).to_value(),
        };
        let mut proposal = None;
        if req.kind.is_none() && !req.is_frozen() {
            if let Some(want) = req.want.as_deref() {
                match agents::propose(&self.orch, &self.template, &self.registry, want, principal).await.0 {
                    Ok(p) => proposal = Some(p),
                    Err(e) => return Reply::refused(Reason::Unsupported, "WM-NRM-06", e).to_value(),
                }
            }
        }
        let est = FixedEstimate(v.pointer("/context/estimate").map(|e| serde_json::from_value(e.clone()).unwrap()));
        match intake(&self.ctx(&est), req, principal, proposal) {
            Intake::Reply(r) => r.to_value(),
            Intake::Run(m) if m.reply == Some(ReplyMode::Receipt) => Reply::accepted(m.digest.as_deref().unwrap(), "rcpt-vector").to_value(),
            Intake::Run(m) => json!({ "runs": true, "digest": m.digest }),
        }
    }

    fn assemble(&self, v: &Value) -> Value {
        let m = Manifest::from_value(self.resolve(&v["manifest"])).unwrap();
        let work = match &v["work"] {
            Value::String(s) if s == "none" => None,
            w => Some(WorkOutput {
                answer: w.get("answer").cloned(),
                searched: w.get("searched").map(|s| serde_json::from_value(s.clone()).unwrap()),
                usage: w.get("usage").map(|u| serde_json::from_value::<Usage>(u.clone()).unwrap()).unwrap_or_default(),
                proposals: w.get("proposals").and_then(Value::as_array).cloned().unwrap_or_default(),
                failure: w.get("failure").map(|f| Failure { code: serde_json::from_value(f.clone()).unwrap(), detail: None }),
            }),
        };
        let unavailable: Vec<String> = v.pointer("/context/unavailable_functions").map(|u| serde_json::from_value(u.clone()).unwrap()).unwrap_or_default();
        assemble(&self.registry, self.store.as_ref(), &self.eval, &m, work, &unavailable).to_value()
    }

    fn delegate(&self, v: &Value) -> Value {
        let parent = Manifest::from_value(self.resolve(&v["parent"])).unwrap();
        let child = Manifest::from_value(self.resolve(&v["child"])).unwrap();
        let s = &v["spent"];
        let spent = Spent { usd: s["usd"].as_f64().unwrap_or(0.0), secs: s["time"].as_str().and_then(duration_secs).unwrap_or(0) };
        let est = FixedEstimate(None);
        match delegate(&self.ctx(&est), &parent, spent, child, "orchestrator:taffee-a", Target::Orchestrator) {
            Ok(m) => json!({ "sent": true, "child": m.to_value() }),
            Err(violates) => json!({ "sent": false, "violates": violates }),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conformance_vectors() {
    let mut h = build();
    let dir = PathBuf::from(SPEC).join("vectors");
    let mut passed: Vec<String> = Vec::new();
    let mut failed: Vec<(String, Vec<String>)> = Vec::new();
    for file in ["normalize.yaml", "handle.yaml", "result.yaml", "delegate.yaml"] {
        let doc = load(&dir.join(file));
        let stage = doc["stage"].as_str().unwrap().to_string();
        for v in doc["vectors"].as_array().unwrap() {
            let id = v["id"].as_str().unwrap().to_string();
            let out = match stage.as_str() {
                "normalize" | "admit" => h.admit(v).await,
                "assemble" => h.assemble(v),
                "delegate" => h.delegate(v),
                s => panic!("unknown stage {s}"),
            };
            // Stored first: an expectation may refer to its own vector's output (N-01).
            h.outputs.insert(id.clone(), out.clone());
            let mut errs = Vec::new();
            h.matches(&v["expect"], Some(&out), &id, &mut errs);
            if errs.is_empty() {
                println!("PASS {id}  {}", v["title"].as_str().unwrap_or(""));
                passed.push(id.clone());
            } else {
                println!("FAIL {id}  {}\n  {}\n  output: {}", v["title"].as_str().unwrap_or(""), errs.join("\n  "), out);
                failed.push((id.clone(), errs));
            }
        }
    }

    // Every rule proved by vectors: its vectors all passed.
    let rules = load(&PathBuf::from(SPEC).join("rules.yaml"));
    let mut uncovered = Vec::new();
    let mut proved = 0;
    for r in rules["rules"].as_array().unwrap() {
        if r["proof"] != "vectors" {
            continue;
        }
        let vs: Vec<&str> = r["vectors"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        if vs.is_empty() || vs.iter().any(|x| !passed.iter().any(|p| p == x)) {
            uncovered.push(r["id"].as_str().unwrap().to_string());
        } else {
            proved += 1;
        }
    }
    println!("\nvectors: {} passed, {} failed; rules proved by vectors: {proved}, not proved: {uncovered:?}", passed.len(), failed.len());
    assert!(failed.is_empty(), "{} vectors failed: {:?}", failed.len(), failed.iter().map(|f| &f.0).collect::<Vec<_>>());
    assert!(uncovered.is_empty(), "rules not proved: {uncovered:?}");
}
