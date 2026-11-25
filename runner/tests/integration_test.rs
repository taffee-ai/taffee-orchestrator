//! End-to-end tests against the real strand-agent.wasm and the mock servers.
//!
//! Needs (started by scripts/ci.sh):
//!   mock-bedrock on 127.0.0.1:8000 (model)
//!   mock-bedrock on 127.0.0.1:8001 with --mcp-token MCP_TOKEN (MCP server)
//! Run with --test-threads=1: tests read the mocks' shared request logs.

use axum::body::Body;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use taffee_orchestrator::egress::{CallOutcome, CredentialRule, EgressGate, EgressPolicy};
use taffee_orchestrator::pool::PoolConfigOptions;
use taffee_orchestrator::runtime::{Runtime, RuntimeOptions, TurnFailure};
use taffee_orchestrator::sigv4::AwsCredentials;
use taffee_orchestrator::spec::Authority;
use taffee_orchestrator::{api, AgentSpec, OrchError, Orchestrator, OrchestratorConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

const MODEL: &str = "http://127.0.0.1:8000";
const MCP_URL: &str = "http://127.0.0.1:8001/mcp";
const MCP_TOKEN: &str = "test-mcp-token-0123456789";
const WASM: &str = "../strand-agent/dist/strand-agent.wasm";

// ------------------------------------------------------------------------------------------
// helpers
// ------------------------------------------------------------------------------------------

async fn raw(port: u16, method: &str, path: &str) -> Value {
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.expect("mock reachable");
    let req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf);
    serde_json::from_str(text.split("\r\n\r\n").nth(1).unwrap_or("{}")).unwrap_or(json!({}))
}

async fn reset_mocks() {
    raw(8000, "POST", "/reset_stats").await;
    raw(8001, "POST", "/reset_stats").await;
}

async fn model_requests() -> Vec<Value> {
    raw(8000, "GET", "/requests").await["requests"].as_array().cloned().unwrap_or_default()
}

async fn mcp_requests() -> Vec<Value> {
    raw(8001, "GET", "/mcp_requests").await["requests"].as_array().cloned().unwrap_or_default()
}

fn shared_runtime() -> Arc<Runtime> {
    static RT: OnceLock<Arc<Runtime>> = OnceLock::new();
    RT.get_or_init(|| {
        Runtime::load(
            &PathBuf::from(WASM),
            RuntimeOptions { pool: PoolConfigOptions { max_agents: 16, ..Default::default() }, ..Default::default() },
        )
        .expect("load strand-agent.wasm")
    })
    .clone()
}

fn config() -> OrchestratorConfig {
    let mut c = OrchestratorConfig::default();
    c.component = PathBuf::from(WASM);
    c.turn_timeout = Duration::from_secs(30);
    c.default_spec.model.endpoint = Some(MODEL.into());
    c
}

fn orch_with(cfg: &OrchestratorConfig) -> Arc<Orchestrator> {
    Orchestrator::with_runtime(shared_runtime(), cfg).unwrap()
}

fn mcp_spec() -> AgentSpec {
    serde_json::from_value(json!({ "mcpServers": [{ "name": "ctx", "url": MCP_URL }] })).unwrap()
}

fn mcp_bearer() -> CredentialRule {
    CredentialRule::bearer("context-mcp".into(), Authority::from_url(MCP_URL).unwrap(), MCP_TOKEN.into())
}

// ------------------------------------------------------------------------------------------
// agent loop
// ------------------------------------------------------------------------------------------

#[tokio::test]
async fn calculator_tool_roundtrip() {
    reset_mocks().await;
    let o = orch_with(&config());
    let r = o
        .run_once(AgentSpec::default(), "Calculate 21 * 2 using calculator tool and return the result", None, None)
        .await
        .expect("turn");
    assert_eq!(r.outcome, "ok", "{r:?}");
    assert_eq!(r.result.as_deref().map(str::trim), Some("The calculation result is 42."));
    assert_eq!(r.calls.len(), 2, "two Converse calls: {:?}", r.calls);
    assert!(r.calls.iter().all(|c| c.outcome == CallOutcome::Sent && c.status == Some(200)));

    let reqs = model_requests().await;
    assert_eq!(reqs.len(), 2);
    let second = serde_json::to_string(&reqs[1]["messages"]).unwrap();
    assert!(second.contains("toolResult") && second.contains("42"));
}

#[tokio::test]
async fn http_request_tool_fetches_a_page_as_text() {
    // Our own http_request (plain fetch): Strands' vended one uses AbortSignal.timeout/any,
    // which the sandbox's JS engine lacks, and failed every call with "5e-324".
    reset_mocks().await;
    let o = orch_with(&config());
    let spec = AgentSpec { tools: Some(vec!["http_request".into()]), ..Default::default() };
    let r = o.run_once(spec, "fetchpage please", None, None).await.expect("turn");
    assert_eq!(r.outcome, "ok", "{r:?}");
    let text = r.result.clone().unwrap_or_default();
    assert!(text.contains("Hello page") && text.contains("\"status\":200"), "{text}");
    assert!(!text.contains("<h1>") && !text.contains("var x"), "HTML comes back as text: {text}");
    assert!(text.contains("more here (https://example.com/more)"), "links keep their target: {text}");
    assert!(r.calls.iter().any(|c| c.path == "/page" && c.status == Some(200)), "{:?}", r.calls);
}

#[tokio::test]
async fn agents_keep_history_and_are_isolated() {
    let o = orch_with(&config());
    let a = o.create_agent(AgentSpec::default(), None, None).await.unwrap();
    let b = o.create_agent(AgentSpec::default(), None, None).await.unwrap();
    let a1 = o.turn(&a.agent_id, "Remember the marker ALPHA731").await.unwrap();
    let a2 = o.turn(&a.agent_id, "Second turn").await.unwrap();
    let b1 = o.turn(&b.agent_id, "Independent turn").await.unwrap();
    assert_eq!((a1.message_count, a2.message_count, b1.message_count), (Some(2), Some(4), Some(2)));
    assert_eq!((a2.turn, b1.turn), (2, 1));
    assert_eq!(o.agent(&a.agent_id).await.unwrap().turns, 2);
}

#[tokio::test]
async fn seeded_history_and_system_prompt_reach_the_model() {
    reset_mocks().await;
    let o = orch_with(&config());
    let spec: AgentSpec = serde_json::from_value(json!({
        "systemPrompt": "You are a terse assistant.",
        "messages": [
            {"role": "user", "content": [{"text": "My name is Zed"}]},
            {"role": "assistant", "content": [{"text": "Hello Zed"}]}
        ]
    }))
    .unwrap();
    let r = o.run_once(spec, "What is my name?", None, None).await.unwrap();
    assert_eq!(r.outcome, "ok", "{r:?}");
    assert_eq!(r.message_count, Some(4));
    let req = &model_requests().await[0];
    assert_eq!(req["messages"].as_array().unwrap().len(), 3);
    assert!(req["messages"][0].to_string().contains("My name is Zed"));
    assert!(req["system"].to_string().contains("terse assistant"), "system prompt: {}", req["system"]);
}

#[tokio::test]
async fn failed_turn_rolls_back_history() {
    let o = orch_with(&config());
    let a = o.create_agent(AgentSpec::default(), None, None).await.unwrap();
    let ok1 = o.turn(&a.agent_id, "hello").await.unwrap();
    let bad = o.turn(&a.agent_id, "please explode").await.unwrap();
    assert_eq!(bad.outcome, "turn_failed", "{bad:?}");
    assert_eq!(bad.message_count, ok1.message_count, "failed turn left no messages behind");
    let ok2 = o.turn(&a.agent_id, "still alive?").await.unwrap();
    assert_eq!(ok2.outcome, "ok");
    assert_eq!(ok2.message_count, Some(ok1.message_count.unwrap() + 2), "conversation still alternates");
}

#[tokio::test]
async fn sdk_turn_limit_stops_a_runaway_tool_loop() {
    reset_mocks().await;
    let o = orch_with(&config());
    let spec: AgentSpec = serde_json::from_value(json!({ "limits": { "turns": 3 } })).unwrap();
    let r = o.run_once(spec, "loop forever with the calculator", None, None).await.unwrap();
    assert_eq!(r.outcome, "ok", "{r:?}");
    assert!(r.stop_reason.as_deref().unwrap_or("").to_lowercase().contains("limit"), "stop reason {:?}", r.stop_reason);
    let n = model_requests().await.len();
    assert!(n <= 3, "turn limit 3 allowed {n} model calls");
}

// ------------------------------------------------------------------------------------------
// credentials stay on the host
// ------------------------------------------------------------------------------------------

#[tokio::test]
async fn guest_sends_only_placeholder_credentials() {
    reset_mocks().await;
    let o = orch_with(&config());
    o.run_once(AgentSpec::default(), "hello", None, None).await.unwrap();
    let auth = model_requests().await[0]["authorization"].as_str().unwrap_or("").to_string();
    assert!(auth.contains("Credential=TAFFEE_PLACEHOLDER/"), "guest signed with: {auth}");
}

#[tokio::test]
async fn host_resigns_model_calls_with_sigv4() {
    reset_mocks().await;
    let mut cfg = config();
    let creds = AwsCredentials { access_key_id: "AKIDTESTHOST".into(), secret_access_key: "host-only-secret".into(), session_token: None };
    cfg.egress = EgressPolicy::new(
        vec![],
        vec![CredentialRule::sigv4("model".into(), Authority::from_url(MODEL).unwrap(), "us-east-1".into(), "bedrock".into(), creds)],
        None,
    )
    .unwrap();
    let o = orch_with(&cfg);
    let r = o.run_once(AgentSpec::default(), "Calculate 21 * 2", None, None).await.unwrap();
    assert_eq!(r.outcome, "ok", "{r:?}");
    assert!(r.calls.iter().all(|c| c.credential.as_deref() == Some("model")));
    for req in model_requests().await {
        let auth = req["authorization"].as_str().unwrap_or("");
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDTESTHOST/"), "authorization: {auth}");
        assert!(auth.contains("/us-east-1/bedrock/aws4_request"));
        assert!(!auth.contains("TAFFEE_PLACEHOLDER"));
        assert!(req["amzDate"].as_str().is_some_and(|d| d.len() == 16));
    }
}

// ------------------------------------------------------------------------------------------
// MCP tools
// ------------------------------------------------------------------------------------------

#[tokio::test]
async fn mcp_tools_work_with_host_injected_token() {
    reset_mocks().await;
    let mut cfg = config();
    cfg.egress = EgressPolicy::new(vec![], vec![mcp_bearer()], None).unwrap();
    let o = orch_with(&cfg);
    let r = o.run_once(mcp_spec(), "search for permission checks", None, None).await.unwrap();
    assert_eq!(r.outcome, "ok", "{r:?}");
    assert!(r.result.as_deref().unwrap_or("").contains("3 matches"), "{r:?}");

    let mcp = mcp_requests().await;
    let methods: Vec<&str> = mcp.iter().filter_map(|m| m["method"].as_str()).collect();
    assert_eq!(methods, vec!["initialize", "notifications/initialized", "tools/list", "tools/call"]);
    for m in &mcp {
        assert_eq!(m["authorization"].as_str(), Some(format!("Bearer {MCP_TOKEN}").as_str()));
    }
    assert_eq!(mcp[2]["session"].as_str(), Some("mock-session-1"), "session id is carried");
    let model = model_requests().await;
    let tools = model[0]["toolConfig"].to_string();
    assert!(tools.contains("search_code") && tools.contains("calculator"));
    assert!(r.calls.iter().any(|c| c.authority == "127.0.0.1:8001" && c.credential.as_deref() == Some("context-mcp")));
}

#[tokio::test]
async fn mcp_without_credential_fails_the_spec_cleanly() {
    let o = orch_with(&config());
    let a = o.create_agent(mcp_spec(), None, None).await.unwrap();
    let r = o.turn(&a.agent_id, "search anything").await.unwrap();
    assert_eq!(r.outcome, "spec_invalid", "{r:?}");
    assert!(r.error.as_deref().unwrap_or("").contains("401"), "{r:?}");
}

// ------------------------------------------------------------------------------------------
// egress gate
// ------------------------------------------------------------------------------------------

#[tokio::test]
async fn egress_to_unlisted_destination_is_denied_and_recorded() {
    // The worker's gate only allows a port the spec does not use.
    let rt = shared_runtime();
    let gate = EgressGate::new(Arc::new(EgressPolicy::default()), &[Authority::from_url("http://127.0.0.1:9").unwrap()]);
    let mut w = rt.spawn(gate).await.unwrap();
    let input = json!({ "prompt": "hello", "spec": { "model": { "endpoint": MODEL } } }).to_string();
    let out = w.invoke(&input, Duration::from_secs(30)).await.map_err(|(f, _)| f.to_string()).unwrap();
    assert_eq!(out.output["status"], "error");
    assert!(!out.calls.is_empty());
    assert!(out.calls.iter().all(|c| c.outcome == CallOutcome::Denied && c.authority == "127.0.0.1:8000"));
    assert!(!w.is_poisoned(), "a denied call is a failed turn, not a dead agent");
}

// ------------------------------------------------------------------------------------------
// deadlines and lifecycle
// ------------------------------------------------------------------------------------------

#[tokio::test]
async fn turn_deadline_removes_the_agent() {
    let mut cfg = config();
    cfg.turn_timeout = Duration::from_secs(2);
    let o = orch_with(&cfg);
    let a = o.create_agent(AgentSpec::default(), None, None).await.unwrap();
    let t = Instant::now();
    let err = o.turn(&a.agent_id, "slowpoke").await.unwrap_err();
    assert!(matches!(err, OrchError::Timeout(_)), "{err}");
    assert!(t.elapsed() < Duration::from_secs(5), "deadline enforced in {:?}", t.elapsed());
    assert!(matches!(o.agent(&a.agent_id).await, Err(OrchError::NotFound)));
    assert_eq!(o.stats().await["turns"]["timed_out"], 1);
}

#[tokio::test]
async fn second_concurrent_turn_on_one_agent_is_busy() {
    let o = orch_with(&config());
    let a = o.create_agent(AgentSpec::default(), None, None).await.unwrap();
    let (o2, id) = (o.clone(), a.agent_id.clone());
    let slow = tokio::spawn(async move { o2.turn(&id, "slowpoke").await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(o.agent(&a.agent_id).await.unwrap().busy);
    assert!(matches!(o.turn(&a.agent_id, "hello").await, Err(OrchError::Busy)));
    assert_eq!(slow.await.unwrap().unwrap().outcome, "ok");
}

#[tokio::test]
async fn capacity_is_enforced_and_released() {
    let rt = Runtime::load(
        &PathBuf::from(WASM),
        RuntimeOptions { pool: PoolConfigOptions { max_agents: 2, ..Default::default() }, ..Default::default() },
    )
    .unwrap();
    let o = Orchestrator::with_runtime(rt, &config()).unwrap();
    let a = o.create_agent(AgentSpec::default(), None, None).await.unwrap();
    let _b = o.create_agent(AgentSpec::default(), None, None).await.unwrap();
    assert!(matches!(o.create_agent(AgentSpec::default(), None, None).await, Err(OrchError::AtCapacity(2))));
    o.delete_agent(&a.agent_id).await.unwrap();
    o.create_agent(AgentSpec::default(), None, None).await.expect("slot released on delete");
}

#[tokio::test]
async fn idle_agents_are_evicted() {
    let mut cfg = config();
    cfg.idle_ttl = Duration::from_millis(50);
    let o = orch_with(&cfg);
    let a = o.create_agent(AgentSpec::default(), None, None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(o.evict_idle().await, 1);
    assert!(matches!(o.agent(&a.agent_id).await, Err(OrchError::NotFound)));
}

#[tokio::test]
async fn turn_records_hold_no_content() {
    let dir = std::env::temp_dir().join(format!("taffee-records-{}", std::process::id()));
    let mut cfg = config();
    cfg.records_path = Some(dir.join("turns.jsonl"));
    let o = orch_with(&cfg);
    let intent = serde_json::from_value(json!({ "objective": "test records", "riskClass": "low" })).unwrap();
    let a = o.create_agent(AgentSpec::default(), Some(intent), Some("tester".into())).await.unwrap();
    o.turn(&a.agent_id, "SECRET-PROMPT-TEXT calculate 2 + 3").await.unwrap();
    let text = std::fs::read_to_string(dir.join("turns.jsonl")).unwrap();
    let rec: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    assert_eq!(rec["agent_id"], a.agent_id.as_str());
    assert_eq!(rec["requester"], "tester");
    assert!(rec["intent_id"].as_str().unwrap().starts_with("intent-"));
    assert_eq!(rec["outcome"], "ok");
    assert!(!text.contains("SECRET-PROMPT-TEXT") && !text.contains("The calculation result"));
    assert_eq!(o.turn_records(&a.agent_id).await.unwrap().len(), 1);
}

// ------------------------------------------------------------------------------------------
// any guest that implements the contract (hand-written WAT, no JS)
// ------------------------------------------------------------------------------------------

fn wat_guest(body: &str) -> String {
    format!(
        r#"(component
  (core module $m
    (memory (export "memory") 1)
    (global $bump (mut i32) (i32.const 4096))
    (func (export "realloc") (param i32 i32 i32 i32) (result i32)
      (local $p i32)
      (local.set $p (global.get $bump))
      (global.set $bump (i32.add (global.get $bump) (local.get 3)))
      (local.get $p))
    {body}
  )
  (core instance $i (instantiate $m))
  (func $run (param "input" string) (result string)
    (canon lift (core func $i "run") (memory (core memory $i "memory")) (realloc (core func $i "realloc"))))
  (instance $w (export "run-turn" (func $run)))
  (export "taffee:agent/worker@0.1.0" (instance $w))
)"#
    )
}

fn small_runtime(wat: &str) -> Arc<Runtime> {
    Runtime::from_bytes(
        wat.as_bytes(),
        RuntimeOptions { pool: PoolConfigOptions { max_agents: 4, ..Default::default() }, ..Default::default() },
    )
    .unwrap()
}

#[tokio::test]
async fn non_js_guest_implementing_the_contract_runs() {
    let reply = r#"{"httpStatus":200,"status":"ok","result":"pong","stopReason":"endTurn","messageCount":2}"#;
    let escaped = reply.replace('\\', "\\\\").replace('"', "\\\"");
    let body = format!(
        r#"(data (i32.const 16) "{escaped}")
    (func (export "run") (param i32 i32) (result i32)
      (i32.store (i32.const 0) (i32.const 16))
      (i32.store (i32.const 4) (i32.const {len}))
      (i32.const 0))"#,
        len = reply.len()
    );
    let rt = small_runtime(&wat_guest(&body));
    let o = Orchestrator::with_runtime(rt, &config()).unwrap();
    let r = o.run_once(AgentSpec::default(), "ping", None, None).await.unwrap();
    assert_eq!((r.outcome.as_str(), r.result.as_deref()), ("ok", Some("pong")));
}

#[tokio::test]
async fn guest_past_its_memory_limit_is_removed() {
    // Asks for 1,100 more pages (~69 MB) on top of its 1 page; the 64 MB limit refuses, so
    // memory.grow returns -1 and the guest traps.
    let rt = small_runtime(&wat_guest(
        r#"(func (export "run") (param i32 i32) (result i32)
      (if (i32.eq (memory.grow (i32.const 1100)) (i32.const -1)) (then (unreachable)))
      (i32.const 0))"#,
    ));
    let o = Orchestrator::with_runtime(rt, &config()).unwrap();
    let a = o.create_agent(AgentSpec::default(), None, None).await.unwrap();
    let err = o.turn(&a.agent_id, "grow").await.unwrap_err();
    assert!(matches!(err, OrchError::AgentLost(..)), "{err}");
    assert_eq!(err.http_status(), 502);
    assert!(matches!(o.agent(&a.agent_id).await, Err(OrchError::NotFound)));
}

#[tokio::test(flavor = "current_thread")]
async fn cpu_bound_guest_is_preempted_and_does_not_starve_others() {
    let rt = small_runtime(&wat_guest(r#"(func (export "run") (param i32 i32) (result i32) (loop $l (br $l)) (unreachable))"#));
    let mut w = rt.spawn(EgressGate::new(Arc::new(EgressPolicy::default()), &[])).await.unwrap();

    // On a single-threaded runtime, a timer must still fire on time while the guest spins.
    let timer = tokio::spawn(async {
        let t = Instant::now();
        tokio::time::sleep(Duration::from_millis(50)).await;
        t.elapsed()
    });
    let t = Instant::now();
    let res = w.invoke("{}", Duration::from_millis(500)).await;
    let spun = t.elapsed();
    assert!(matches!(res, Err((TurnFailure::Timeout, _))), "spinning guest must hit its deadline");
    assert!(spun < Duration::from_secs(2), "preempted after {spun:?}");
    assert!(w.is_poisoned());
    let waited = timer.await.unwrap();
    assert!(waited < Duration::from_millis(250), "timer starved for {waited:?} by a spinning guest");
}

// ------------------------------------------------------------------------------------------
// HTTP API
// ------------------------------------------------------------------------------------------

async fn call(app: &axum::Router, method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> (u16, Value) {
    let mut req = axum::http::Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let req = req.body(body.map(|b| Body::from(b.to_string())).unwrap_or_else(Body::empty)).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn http_api_end_to_end() {
    let token = "api-token-for-tests-0123";
    let app = api::router(orch_with(&config()), &[("ci".to_string(), token.to_string())]);

    assert_eq!(call(&app, "GET", "/healthz", None, None).await.0, 200);
    assert_eq!(call(&app, "GET", "/v1/agents", None, None).await.0, 401);
    assert_eq!(call(&app, "GET", "/v1/agents", Some("wrong-token-wrong-token"), None).await.0, 401);

    let (s, created) = call(&app, "POST", "/v1/agents", Some(token), Some(json!({ "intent": { "objective": "api test" } }))).await;
    assert_eq!(s, 201, "{created}");
    assert_eq!(created["requester"], "ci");
    let id = created["agent_id"].as_str().unwrap().to_string();

    let (s, t1) = call(&app, "POST", &format!("/v1/agents/{id}/turns"), Some(token), Some(json!({ "prompt": "Calculate 6 * 7" }))).await;
    assert_eq!(s, 200, "{t1}");
    assert_eq!(t1["result"].as_str().map(str::trim), Some("The calculation result is 42."));

    let (s, recs) = call(&app, "GET", &format!("/v1/agents/{id}/turns"), Some(token), None).await;
    assert_eq!((s, recs.as_array().map(Vec::len)), (200, Some(1)));

    let (s, list) = call(&app, "GET", "/v1/agents", Some(token), None).await;
    assert!(s == 200 && list.as_array().unwrap().iter().any(|a| a["agent_id"] == id.as_str()));

    let (s, stats) = call(&app, "GET", "/v1/stats", Some(token), None).await;
    assert_eq!(s, 200);
    assert!(stats["agents"]["active"].as_u64().unwrap() >= 1);

    // errors
    assert_eq!(call(&app, "POST", &format!("/v1/agents/{id}/turns"), Some(token), Some(json!({ "prompt": "" }))).await.0, 400);
    assert_eq!(call(&app, "POST", &format!("/v1/agents/{id}/turns"), Some(token), Some(json!({ "nope": 1 }))).await.0, 400);
    assert_eq!(call(&app, "POST", "/v1/agents/ag-missing/turns", Some(token), Some(json!({ "prompt": "x" }))).await.0, 404);
    let (s, e) = call(&app, "POST", "/v1/agents", Some(token), Some(json!({ "spec": { "tools": ["shell"] } }))).await;
    assert_eq!((s, e["error"].as_str()), (400, Some("invalid_spec")));

    assert_eq!(call(&app, "DELETE", &format!("/v1/agents/{id}"), Some(token), None).await.0, 204);
    assert_eq!(call(&app, "GET", &format!("/v1/agents/{id}"), Some(token), None).await.0, 404);

    let (s, run) = call(&app, "POST", "/v1/runs", Some(token), Some(json!({ "prompt": "Calculate 21 * 2" }))).await;
    assert_eq!(s, 200, "{run}");
    assert_eq!(run["outcome"], "ok");
}
