//! HTTP API of `orchestratord` (documented in docs/api.md).
//!
//! GET    /healthz
//! GET    /v1/stats
//! POST   /v1/agents                  {spec?, intent?}          -> 201 AgentInfo
//! GET    /v1/agents                                             -> [AgentInfo]
//! GET    /v1/agents/{id}                                        -> AgentInfo
//! DELETE /v1/agents/{id}                                        -> 204
//! POST   /v1/agents/{id}/turns       {prompt}                  -> TurnResponse
//! GET    /v1/agents/{id}/turns                                  -> [TurnRecord]
//! POST   /v1/runs                    {prompt, spec?, intent?}  -> TurnResponse (one-shot)
//! POST   /v1/work                    work manifest             -> reply (202 when ACCEPTED)
//! GET    /v1/work/{receipt}                                     -> reply
//!
//! The work routes exist when the orchestrator is configured with `work`; see
//! docs/spec/work-manifest/. Agents and runs stay available for operators.
//!
//! When API tokens are configured every /v1 request needs `Authorization: Bearer <token>`;
//! the token's name is recorded as the requester.

use crate::orchestrator::{Intent, OrchError, Orchestrator};
use crate::work::engine::Engine;
use crate::work::reply::Status;
use crate::spec::AgentSpec;
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;

const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_PROMPT_BYTES: usize = 256 * 1024;

#[derive(Clone)]
struct AppState {
    orch: Arc<Orchestrator>,
    work: Option<Arc<Engine>>,
    /// (name, sha256(token))
    tokens: Arc<Vec<(String, [u8; 32])>>,
}

#[derive(Clone, Debug)]
struct Requester(Option<String>);

struct ApiError(StatusCode, serde_json::Value);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

impl From<OrchError> for ApiError {
    fn from(e: OrchError) -> Self {
        let status = StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut body = json!({ "error": e.kind(), "message": e.to_string() });
        if !e.calls().is_empty() {
            body["calls"] = json!(e.calls());
        }
        ApiError(status, body)
    }
}

fn bad_request(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, json!({ "error": "bad_request", "message": msg.into() }))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CreateAgent {
    #[serde(default)]
    spec: AgentSpec,
    #[serde(default)]
    intent: Option<Intent>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TurnBody {
    prompt: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunBody {
    prompt: String,
    #[serde(default)]
    spec: AgentSpec,
    #[serde(default)]
    intent: Option<Intent>,
}

fn check_prompt(p: &str) -> Result<(), ApiError> {
    if p.trim().is_empty() {
        return Err(bad_request("prompt is empty"));
    }
    if p.len() > MAX_PROMPT_BYTES {
        return Err(bad_request(format!("prompt is longer than {MAX_PROMPT_BYTES} bytes")));
    }
    Ok(())
}

/// Parse JSON ourselves so malformed bodies get the same error shape as everything else.
fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ApiError> {
    let body = if body.iter().all(u8::is_ascii_whitespace) { b"{}".as_slice() } else { body };
    serde_json::from_slice(body).map_err(|e| bad_request(format!("invalid JSON body: {e}")))
}

async fn auth(State(st): State<AppState>, mut req: Request, next: Next) -> Response {
    if st.tokens.is_empty() {
        req.extensions_mut().insert(Requester(None));
        return next.run(req).await;
    }
    let presented = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| Sha256::digest(t.as_bytes()));
    // Compare digests in full for every token so timing does not reveal which one matched.
    let mut matched = None;
    if let Some(p) = presented {
        for (name, digest) in st.tokens.iter() {
            let eq = p.iter().zip(digest.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
            if eq && matched.is_none() {
                matched = Some(name.clone());
            }
        }
    }
    match matched {
        Some(name) => {
            req.extensions_mut().insert(Requester(Some(name)));
            next.run(req).await
        }
        None => ApiError(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized", "message": "missing or unknown bearer token" }))
            .into_response(),
    }
}

/// `version` is stamped into the runtime image at build time (TAFFEE_ORCH_VERSION), so a
/// caller can see exactly what is deployed.
async fn healthz() -> impl IntoResponse {
    let version = std::env::var("TAFFEE_ORCH_VERSION").unwrap_or_else(|_| "dev".into());
    Json(json!({ "ok": true, "version": version }))
}

async fn stats(State(st): State<AppState>) -> impl IntoResponse {
    Json(st.orch.stats().await)
}

async fn create_agent(
    State(st): State<AppState>,
    Extension(who): Extension<Requester>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let b: CreateAgent = parse(&body)?;
    let info = st.orch.create_agent(b.spec, b.intent, who.0).await?;
    Ok((StatusCode::CREATED, Json(info)))
}

async fn list_agents(State(st): State<AppState>) -> impl IntoResponse {
    Json(st.orch.list_agents().await)
}

async fn get_agent(State(st): State<AppState>, Path(id): Path<String>) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(st.orch.agent(&id).await?))
}

async fn delete_agent(State(st): State<AppState>, Path(id): Path<String>) -> Result<impl IntoResponse, ApiError> {
    st.orch.delete_agent(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn post_turn(
    State(st): State<AppState>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let b: TurnBody = parse(&body)?;
    check_prompt(&b.prompt)?;
    Ok(Json(st.orch.turn(&id, &b.prompt).await?))
}

async fn list_turns(State(st): State<AppState>, Path(id): Path<String>) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(st.orch.turn_records(&id).await?))
}

async fn run_once(
    State(st): State<AppState>,
    Extension(who): Extension<Requester>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let b: RunBody = parse(&body)?;
    check_prompt(&b.prompt)?;
    Ok(Json(st.orch.run_once(b.spec, &b.prompt, b.intent, who.0).await?))
}

/// The caller's principal: the token name (`kind:name`, or `service:<name>` when the name
/// has no kind), or `user:anonymous` when the API is open.
fn principal(who: &Requester) -> String {
    match &who.0 {
        Some(n) if n.contains(':') => n.clone(),
        Some(n) => format!("service:{n}"),
        None => "user:anonymous".into(),
    }
}

async fn post_work(
    State(st): State<AppState>,
    Extension(who): Extension<Requester>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let engine = st.work.clone().ok_or_else(|| ApiError(StatusCode::NOT_FOUND, json!({ "error": "not_found", "message": "this orchestrator is not configured for work manifests" })))?;
    let body: serde_json::Value = parse(&body)?;
    let reply = engine.submit(body, &principal(&who)).await;
    let status = if reply.status == Status::Accepted { StatusCode::ACCEPTED } else { StatusCode::OK };
    Ok((status, Json(reply)))
}

async fn get_work(State(st): State<AppState>, Path(id): Path<String>) -> Result<impl IntoResponse, ApiError> {
    let engine = st.work.clone().ok_or_else(|| ApiError(StatusCode::NOT_FOUND, json!({ "error": "not_found", "message": "no work API here" })))?;
    let reply = engine.receipt(&id).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, json!({ "error": "not_found", "message": "unknown receipt" })))?;
    Ok(Json(reply))
}

pub fn router(orch: Arc<Orchestrator>, tokens: &[(String, String)]) -> Router {
    router_with_work(orch, None, tokens)
}

pub fn router_with_work(orch: Arc<Orchestrator>, work: Option<Arc<Engine>>, tokens: &[(String, String)]) -> Router {
    let tokens = tokens.iter().map(|(n, t)| (n.clone(), Sha256::digest(t.as_bytes()).into())).collect();
    let st = AppState { orch, work, tokens: Arc::new(tokens) };
    let v1 = Router::new()
        .route("/v1/stats", get(stats))
        .route("/v1/agents", post(create_agent).get(list_agents))
        .route("/v1/agents/{id}", get(get_agent).delete(delete_agent))
        .route("/v1/agents/{id}/turns", post(post_turn).get(list_turns))
        .route("/v1/runs", post(run_once))
        .route("/v1/work", post(post_work))
        .route("/v1/work/{id}", get(get_work))
        .route_layer(middleware::from_fn_with_state(st.clone(), auth));
    Router::new()
        .route("/healthz", get(healthz))
        .merge(v1)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(st)
}
