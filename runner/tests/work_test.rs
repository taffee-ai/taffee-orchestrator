//! The work engine end to end: request → normalizer → (planner) → workers → checks → reply,
//! with real strand-agent.wasm instances against the mock model on 127.0.0.1:8000.
//! The mock reads the fixture repository this test posts to it. Run with --test-threads=1.

use axum::body::Body;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use taffee_orchestrator::pool::PoolConfigOptions;
use taffee_orchestrator::runtime::{Runtime, RuntimeOptions};
use taffee_orchestrator::work::engine::{Engine, WorkConfig};
use taffee_orchestrator::work::kind::yaml_to_json;
use taffee_orchestrator::work::reply::{CheckStatus, FailureCode, Reply, Status};
use taffee_orchestrator::{api, Orchestrator, OrchestratorConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

const MODEL: &str = "http://127.0.0.1:8000";
const WASM: &str = "../strand-agent/dist/strand-agent.wasm";
const SPEC: &str = "../docs/spec/work-manifest";
const COMMIT: &str = "4f2c9e1b7a3d5c8e9f0a1b2c3d4e5f6a7b8c9d0e";
const SENTENCE: &str = "If I rename PaymentService to PaymentGateway in payments-service, what breaks? Need file and line.";

async fn mock(method: &str, path: &str, body: &str) -> Value {
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", 8000)).await.expect("mock reachable");
    let req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:8000\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf);
    serde_json::from_str(text.split("\r\n\r\n").nth(1).unwrap_or("{}")).unwrap_or(json!({}))
}

/// Reset the mock and give it the fixture repository.
async fn prepare() {
    mock("POST", "/reset", "").await;
    let fixture = yaml_to_json(&std::fs::read_to_string(format!("{SPEC}/fixtures/payments-service@4f2c9e1.yaml")).unwrap()).unwrap();
    mock("POST", "/fixture", &fixture.to_string()).await;
}

/// Model requests by role.
async fn role_calls(role: &str) -> usize {
    let reqs = mock("GET", "/requests", "").await["requests"].as_array().cloned().unwrap_or_default();
    reqs.iter().filter(|r| r["system"].to_string().contains(&format!("TAFFEE-ROLE: {role}"))).count()
}

fn runtime() -> Arc<Runtime> {
    static RT: OnceLock<Arc<Runtime>> = OnceLock::new();
    RT.get_or_init(|| Runtime::load(&PathBuf::from(WASM), RuntimeOptions { pool: PoolConfigOptions { max_agents: 16, ..Default::default() }, ..Default::default() }).expect("load wasm")).clone()
}

fn engine(planner: bool) -> Arc<Engine> {
    engine_on(planner, json!({}))
}

/// An engine without a planner whose `user:alice` has this policy.
fn engine_policy(policy: Value) -> Arc<Engine> {
    engine_with(false, json!({}), policy)
}

/// `role` is the agent spec every role starts from (for example, a different model).
fn engine_on(planner: bool, role: Value) -> Arc<Engine> {
    engine_with(planner, role, json!({ "max_effects": "reversible", "max_usd": 2.0, "max_time": "1h" }))
}

fn engine_with(planner: bool, role: Value, policy: Value) -> Arc<Engine> {
    let mut cfg = OrchestratorConfig::default();
    cfg.component = PathBuf::from(WASM);
    cfg.turn_timeout = Duration::from_secs(30);
    cfg.default_spec.model.endpoint = Some(MODEL.into());
    let orch = Orchestrator::with_runtime(runtime(), &cfg).unwrap();
    let pinned = format!("github.com/acme/payments-service@{COMMIT}");
    let work: WorkConfig = serde_json::from_value(json!({
        "kinds_dir": format!("{SPEC}/kinds"),
        "principal": "orchestrator:test",
        "resolve": {
            "names": { "payments-service": ["github.com/acme/payments-service"] },
            "versions": { "github.com/acme/payments-service@main": COMMIT }
        },
        "policy": { "principals": { "user:alice": policy.clone(), "orchestrator:test": policy } },
        "inputs": { pinned: format!("{SPEC}/fixtures/payments-service@4f2c9e1.yaml") },
        "roles": { "normalizer": role.clone(), "planner": if planner { role.clone() } else { Value::Null }, "worker": role }
    }))
    .unwrap();
    Engine::new(orch, &work, &cfg.default_spec).unwrap()
}

fn status_of(r: &Reply, id: &str) -> Option<CheckStatus> {
    r.checks.as_ref().and_then(|c| c.get(id)).copied()
}

#[tokio::test]
async fn a_sentence_runs_to_a_checked_answer() {
    prepare().await;
    let e = engine(false);
    let r = e.submit(json!({ "manifest": "0.1", "want": SENTENCE }), "user:alice").await;
    assert_eq!(r.status, Status::Completed, "{}", r.to_value());
    for id in ["deliver", "cited", "on_target", "complete"] {
        assert_eq!(status_of(&r, id), Some(CheckStatus::Pass), "{id}");
    }
    let a = r.answer.as_ref().unwrap();
    assert_eq!(a["breaks"].as_array().unwrap().len(), 5);
    assert_eq!(a["mentions"].as_array().unwrap().len(), 2);
    assert_eq!(a["breaks"][0], json!({ "file": "src/api/routes.ts", "line": 2, "reason": "import" }), "sorted by file, line");
    let cov = r.coverage.as_ref().unwrap();
    assert_eq!((cov.searched, cov.total), (6, 6));
    assert_eq!(r.proposals.len(), 1, "the docs mention becomes a proposal");
    assert!(r.proposals[0].digest.is_none(), "proposals are sparse and are not run");
    let assumed = r.assumed.as_ref().unwrap();
    assert!(assumed.contains_key("kind") && assumed.contains_key("given.repo"), "{assumed:?}");
    assert!(r.usage.as_ref().unwrap().tokens.unwrap() > 0);
    assert_eq!(r.usage.as_ref().unwrap().model_calls, Some(4), "usage covers the normalizer's 2 calls and the worker's 2");
    assert_eq!(role_calls("normalizer").await, 2, "submit, then the closing turn");
    assert_eq!(role_calls("worker").await, 2);
    assert_eq!(role_calls("planner").await, 0);
}

#[tokio::test]
async fn a_sentence_runs_on_a_chat_completions_model() {
    // Same request and checks as above, with every role on an OpenAI-style chat model
    // (OpenRouter, LiteLLM ...). The mock answers /v1/chat/completions.
    prepare().await;
    let e = engine_on(true, json!({ "model": { "provider": "openai-chat", "modelId": "test-chat-model", "endpoint": format!("{MODEL}/v1") } }));
    let r = e.submit(json!({ "manifest": "0.1", "want": SENTENCE }), "user:alice").await;
    assert_eq!(r.status, Status::Completed, "{}", r.to_value());
    for id in ["deliver", "cited", "on_target", "complete"] {
        assert_eq!(status_of(&r, id), Some(CheckStatus::Pass), "{id}");
    }
    assert_eq!(r.answer.as_ref().unwrap()["breaks"].as_array().unwrap().len(), 5);
    let u = r.usage.as_ref().unwrap();
    assert!(u.tokens.unwrap() > 0, "usage is counted for chat models: {u:?}");
    let reqs = mock("GET", "/requests", "").await["requests"].as_array().cloned().unwrap_or_default();
    assert!(!reqs.is_empty() && reqs.iter().all(|q| q["modelId"] == "test-chat-model"), "every call went over chat completions");
    // Workers run in parallel, one per top directory; the one covering src/ sees src/ files.
    let workers: Vec<String> = reqs.iter().map(|q| q["system"].to_string()).filter(|s| s.contains("TAFFEE-ROLE: worker")).collect();
    assert!(!workers.is_empty(), "a worker call");
    assert!(workers.iter().any(|s| s.contains("src/api/routes.ts:2:")), "the src/ worker sees the file contents");
    assert!(workers.iter().all(|s| s.contains("File contents")), "every worker gets its files' contents");
}

#[tokio::test]
async fn any_question_runs_as_a_general_task_with_every_tool() {
    // Not about code, no repository: the normalizer picks task.general, and the worker gets
    // every tool (http_request, notebook, subagent, ask_model, calculator).
    prepare().await;
    let e = engine(false);
    let r = e.submit(json!({ "manifest": "0.1", "want": "Compare the pricing of three LLM gateways for a small team" }), "user:alice").await;
    assert_eq!(r.status, Status::Completed, "{}", r.to_value());
    let a = r.answer.as_ref().unwrap();
    assert!(a["answer"].as_str().unwrap().contains("LLM gateways"), "{a}");
    assert_eq!(status_of(&r, "answered"), Some(CheckStatus::Pass));
    let reqs = mock("GET", "/requests", "").await["requests"].as_array().cloned().unwrap_or_default();
    let worker = reqs.iter().find(|q| q["system"].to_string().contains("TAFFEE-ROLE: worker")).expect("a worker call");
    let tools: Vec<String> = worker["toolConfig"]["tools"].as_array().unwrap().iter().map(|t| t["toolSpec"]["name"].as_str().unwrap_or("").to_string()).collect();
    for t in ["http_request", "notebook", "subagent", "calculator", "submit"] {
        assert!(tools.iter().any(|n| n == t), "worker has {t}: {tools:?}");
    }
}

#[tokio::test]
async fn an_action_in_the_world_takes_two_deliberate_steps() {
    // "Book a flight" is task.act (irreversible). Spending money must not be easy:
    // 1. the orchestrator will not act on its own reading of a sentence: REFUSED unclear, with
    //    the frozen manifest as a readback for the caller to confirm;
    // 2. the confirmed manifest still needs approval for irreversible work: REFUSED needs_approval;
    // 3. only the confirmed manifest with `approved: irreversible` runs.
    prepare().await;
    let e = engine_policy(json!({ "max_effects": "irreversible", "max_usd": 5.0, "max_time": "1h" }));
    let r = e.submit(json!({ "manifest": "0.1", "want": "Book me a flight to Lisbon on Friday" }), "user:alice").await;
    let v = r.to_value();
    assert_eq!((r.status, v["reason"].as_str()), (Status::Refused, Some("unclear")), "{v}");
    let mut frozen = v["manifest"].clone();
    assert_eq!(frozen["kind"].as_str().unwrap_or("").split('#').next(), Some("task.act@1"), "{frozen}");
    frozen.as_object_mut().unwrap().remove("approved");
    let r = e.submit(frozen.clone(), "user:alice").await;
    assert_eq!((r.status, r.to_value()["reason"].as_str()), (Status::Refused, Some("needs_approval")), "{}", r.to_value());
    frozen["approved"] = json!("irreversible");
    let r = e.submit(frozen, "user:alice").await;
    assert_eq!(r.status, Status::Completed, "{}", r.to_value());
    assert!(r.answer.as_ref().unwrap()["actions"].is_array(), "actions are reported");
}

#[tokio::test]
async fn a_worker_that_invents_a_line_fails_the_checks() {
    prepare().await;
    let e = engine(false);
    let r = e
        .submit(
            json!({ "manifest": "0.1", "kind": "code.impact_analysis@1", "why": "TEST-HALLUCINATE",
                    "parameters": { "symbol": "PaymentService", "rename_to": "PaymentGateway" },
                    "given": { "repo": "github.com/acme/payments-service@main" } }),
            "user:alice",
        )
        .await;
    assert_eq!(r.status, Status::Failed, "{}", r.to_value());
    assert_eq!(r.failure.as_ref().unwrap().code, FailureCode::ChecksFailed);
    assert_eq!(status_of(&r, "cited"), Some(CheckStatus::Fail));
    assert!(r.answer.is_none(), "no answer comes back unless it passed");
}

#[tokio::test]
async fn a_planned_run_splits_into_narrowed_workers() {
    prepare().await;
    let e = engine(true);
    let r = e.submit(json!({ "manifest": "0.1", "want": SENTENCE }), "user:alice").await;
    assert_eq!(r.status, Status::Completed, "{}", r.to_value());
    assert!(r.trace.as_deref().unwrap_or("").contains("planned 3 workers"), "{:?}", r.trace);
    assert_eq!(r.answer.as_ref().unwrap()["breaks"].as_array().unwrap().len(), 5);
    assert_eq!(role_calls("planner").await, 2);
    assert_eq!(role_calls("worker").await, 6, "three workers, two model calls each");
}

#[tokio::test]
async fn a_plan_that_overspends_falls_back_to_one_worker() {
    prepare().await;
    let e = engine(true);
    let r = e.submit(json!({ "manifest": "0.1", "want": SENTENCE, "why": "TEST-OVERSPEND" }), "user:alice").await;
    assert_eq!(r.status, Status::Completed, "{}", r.to_value());
    let trace = r.trace.clone().unwrap_or_default();
    assert!(trace.contains("plan rejected") && trace.contains("WM-DLG-02"), "{trace}");
    assert_eq!(role_calls("worker").await, 2, "one worker");
}

#[tokio::test]
async fn a_plan_that_repeats_the_parents_parameters_is_accepted() {
    // A real model (Haiku, 2026-11-17) copied `symbol` and `rename_to` into every worker
    // unchanged; that is not a change and must not be refused.
    prepare().await;
    let e = engine(true);
    let r = e.submit(json!({ "manifest": "0.1", "want": SENTENCE, "why": "TEST-COPY-PARAMS" }), "user:alice").await;
    assert_eq!(r.status, Status::Completed, "{}", r.to_value());
    assert!(r.trace.as_deref().unwrap_or("").contains("planned 3 workers"), "{:?}", r.trace);
}

#[tokio::test]
async fn a_plan_that_uses_its_whole_offer_is_accepted() {
    // A real model (Haiku, 2026-11-17) split exactly the budget it was told about; the offer
    // must already leave room for the planner's and normalizer's own spending.
    prepare().await;
    let e = engine(true);
    let r = e.submit(json!({ "manifest": "0.1", "want": SENTENCE, "why": "TEST-USE-ALL" }), "user:alice").await;
    assert_eq!(r.status, Status::Completed, "{}", r.to_value());
    assert!(r.trace.as_deref().unwrap_or("").contains("planned 3 workers"), "{:?}", r.trace);
}

#[tokio::test]
async fn a_plan_that_leaves_files_out_falls_back_to_one_worker() {
    // A real model (Haiku, 2026-11-18) planned workers whose paths missed two files; the run
    // then failed its `complete` check after paying for every worker. The plan must be refused.
    prepare().await;
    let e = engine(true);
    let r = e.submit(json!({ "manifest": "0.1", "want": SENTENCE, "why": "TEST-PARTIAL-PLAN" }), "user:alice").await;
    assert_eq!(r.status, Status::Completed, "{}", r.to_value());
    let trace = r.trace.clone().unwrap_or_default();
    assert!(trace.contains("plan rejected") && trace.contains("leave out"), "{trace}");
    let cov = r.coverage.as_ref().unwrap();
    assert_eq!((cov.searched, cov.total), (6, 6));
}

#[tokio::test]
async fn a_worker_that_omits_a_file_from_searched_still_covers_it() {
    // Real models (2026-11-18) left files with nothing relevant out of their `searched` list.
    // The host gave the worker those files' full text, so they were searched (WM-CHK-06).
    prepare().await;
    let e = engine(false);
    let r = e.submit(json!({ "manifest": "0.1", "want": SENTENCE, "why": "TEST-SKIP-SEARCHED" }), "user:alice").await;
    assert_eq!(r.status, Status::Completed, "{}", r.to_value());
    let cov = r.coverage.as_ref().unwrap();
    assert_eq!((cov.searched, cov.total), (6, 6), "{:?}", cov.missed);
}

#[tokio::test]
async fn a_plan_that_changes_a_parameter_falls_back_to_one_worker() {
    prepare().await;
    let e = engine(true);
    let r = e.submit(json!({ "manifest": "0.1", "want": SENTENCE, "why": "TEST-CHANGE-SYMBOL" }), "user:alice").await;
    assert_eq!(r.status, Status::Completed, "{}", r.to_value());
    let trace = r.trace.clone().unwrap_or_default();
    assert!(trace.contains("plan rejected") && trace.contains("changed `symbol`"), "{trace}");
    assert_eq!(trace.matches("changed `symbol`").count(), 1, "each problem is named once: {trace}");
    assert_eq!(role_calls("worker").await, 2, "one worker");
}

#[tokio::test]
async fn a_missing_symbol_is_not_found_before_any_work() {
    prepare().await;
    let e = engine(false);
    let r = e
        .submit(
            json!({ "manifest": "0.1", "kind": "code.impact_analysis@1",
                    "parameters": { "symbol": "PaymentSvc", "rename_to": "PaymentGateway" },
                    "given": { "repo": "github.com/acme/payments-service@main" } }),
            "user:alice",
        )
        .await;
    assert_eq!(r.status, Status::NotFound, "{}", r.to_value());
    assert_eq!(r.missing, vec!["declared"]);
    assert_eq!(role_calls("worker").await, 0);
}

async fn call(app: &axum::Router, method: &str, uri: &str, body: Value) -> (u16, Value) {
    let req = axum::http::Request::builder().method(method).uri(uri).header("content-type", "application/json").header("authorization", "Bearer alice-token-0123456789").body(Body::from(body.to_string())).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status().as_u16();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn http_dry_run_then_receipt() {
    prepare().await;
    let e = engine(false);
    let app = api::router_with_work(e.orch.clone(), Some(e.clone()), &[("user:alice".into(), "alice-token-0123456789".into())]);

    // The readback: a dry run returns the frozen manifest and what was assumed.
    let (s, r) = call(&app, "POST", "/v1/work", json!({ "manifest": "0.1", "want": SENTENCE, "dry_run": true })).await;
    assert_eq!(s, 200);
    assert_eq!(r["status"], "READY", "{r}");
    assert_eq!(r["manifest"]["by"], "user:alice");
    assert_eq!(r["manifest"]["given"]["repo"], format!("github.com/acme/payments-service@{COMMIT}"));
    assert!(r["assumed"]["given.repo"].is_string());
    assert_eq!(role_calls("worker").await, 0, "a dry run runs nothing");

    // Confirm by sending the frozen manifest back; get a receipt; fetch the result.
    let mut frozen = r["manifest"].clone();
    frozen.as_object_mut().unwrap().remove("dry_run");
    frozen["reply"] = json!("receipt");
    let (s, a) = call(&app, "POST", "/v1/work", frozen).await;
    assert_eq!(s, 202, "{a}");
    assert_eq!(a["digest"], r["digest"], "the same work");
    let receipt = a["receipt"].as_str().unwrap().to_string();
    let mut done = Value::Null;
    for _ in 0..100 {
        let (_, x) = call(&app, "GET", &format!("/v1/work/{receipt}"), Value::Null).await;
        if x["status"] != "ACCEPTED" {
            done = x;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(done["status"], "COMPLETED", "{done}");
    let (s, _) = call(&app, "GET", "/v1/work/rcpt-unknown", Value::Null).await;
    assert_eq!(s, 404);
}
