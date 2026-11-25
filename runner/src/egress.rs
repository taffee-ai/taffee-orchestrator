//! The host side of every guest `fetch()`: `wasi:http/outgoing-handler` lands here.
//!
//! For each outgoing request the gate:
//! 1. checks the destination against the agent's allowed authorities (its spec's model and
//!    MCP endpoints plus the deployment allow list) and answers 403 itself if it is not there;
//! 2. removes any credential the guest attached and injects the host's credential for that
//!    destination (Bearer token or a fresh AWS SigV4 signature), so secrets never enter a
//!    guest;
//! 3. optionally routes the request through a `via` proxy (a generic hook for a later
//!    tunnel provider such as a WireGuard gateway; transport security is not done here);
//! 4. writes a call record: destination, status, timing, which credential was used. Never
//!    headers or bodies.

use crate::sigv4::{self, AwsCredentials, SigningInput, GUEST_SIGNATURE_HEADERS};
use crate::spec::Authority;
use anyhow::{bail, Result};
use bytes::Bytes;
use http_body_util::BodyExt;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use wasmtime_wasi_http::{Error as HttpError, RequestOptions, WasiBody, WasiHttpHooks};

/// One allow-list entry.
#[derive(Debug, Clone, PartialEq)]
pub enum AllowRule {
    Any,
    Host { scheme: Option<String>, host: String, port: Option<u16> },
}

impl AllowRule {
    pub fn parse(entry: &str) -> Result<Self> {
        let entry = entry.trim();
        if entry == "*" {
            return Ok(Self::Any);
        }
        if entry.is_empty() {
            bail!("empty egress allow entry");
        }
        let (scheme, rest) = match entry.split_once("://") {
            Some((s, r)) => (Some(s.to_ascii_lowercase()), r),
            None => (None, entry),
        };
        let rest = rest.trim_end_matches('/');
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p.parse::<u16>()?)),
            _ => (rest, None),
        };
        if host.is_empty() || host.contains('/') {
            bail!("invalid egress allow entry '{entry}'");
        }
        Ok(Self::Host { scheme, host: host.to_ascii_lowercase(), port })
    }

    pub fn exact(a: &Authority) -> Self {
        Self::Host { scheme: Some(a.scheme.clone()), host: a.host.clone(), port: Some(a.port) }
    }

    pub fn matches(&self, a: &Authority) -> bool {
        match self {
            Self::Any => true,
            Self::Host { scheme, host, port } => {
                scheme.as_ref().is_none_or(|s| s == &a.scheme) && host == &a.host && port.is_none_or(|p| p == a.port)
            }
        }
    }
}

#[derive(Clone)]
pub enum CredentialKind {
    Bearer { token: String },
    Sigv4 { region: String, service: String, creds: AwsCredentials },
}

#[derive(Clone)]
pub struct CredentialRule {
    pub name: String,
    pub destination: Authority,
    pub kind: CredentialKind,
}

impl std::fmt::Debug for CredentialRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match &self.kind {
            CredentialKind::Bearer { .. } => "bearer",
            CredentialKind::Sigv4 { .. } => "aws_sigv4",
        };
        f.debug_struct("CredentialRule").field("name", &self.name).field("destination", &self.destination).field("kind", &kind).finish()
    }
}

impl CredentialRule {
    pub fn bearer(name: String, destination: Authority, token: String) -> Self {
        Self { name, destination, kind: CredentialKind::Bearer { token } }
    }
    pub fn sigv4(name: String, destination: Authority, region: String, service: String, creds: AwsCredentials) -> Self {
        Self { name, destination, kind: CredentialKind::Sigv4 { region, service, creds } }
    }
    fn applies_to(&self, a: &Authority) -> bool {
        // Scheme is not compared: the credential belongs to the host and port.
        self.destination.host == a.host && self.destination.port == a.port
    }
}

/// Deployment-wide egress settings, shared by every agent.
#[derive(Debug, Clone, Default)]
pub struct EgressPolicy {
    pub allow: Vec<AllowRule>,
    pub credentials: Vec<CredentialRule>,
    pub via: Option<(String, String)>,
}

impl EgressPolicy {
    pub fn new(allow: Vec<String>, credentials: Vec<CredentialRule>, via: Option<(String, String)>) -> Result<Self> {
        let allow = allow.iter().map(|s| AllowRule::parse(s)).collect::<Result<Vec<_>>>()?;
        Ok(Self { allow, credentials, via })
    }

    pub fn credential_names(&self) -> Vec<String> {
        self.credentials.iter().map(|c| c.name.clone()).collect()
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome {
    Sent,
    Denied,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct CallRecord {
    pub seq: u32,
    pub method: String,
    pub scheme: String,
    pub authority: String,
    /// Path only; the query string is left out because it can carry content.
    pub path: String,
    pub outcome: CallOutcome,
    pub status: Option<u16>,
    pub duration_ms: f64,
    pub credential: Option<String>,
    pub error: Option<String>,
}

/// Per-agent gate. Lives in the agent's Store and so sees only that agent's traffic.
pub struct EgressGate {
    policy: Arc<EgressPolicy>,
    allowed: Vec<AllowRule>,
    calls: Arc<Mutex<Vec<CallRecord>>>,
    seq: u32,
}

impl EgressGate {
    pub fn new(policy: Arc<EgressPolicy>, agent_authorities: &[Authority]) -> Self {
        let mut allowed = policy.allow.clone();
        allowed.extend(agent_authorities.iter().map(AllowRule::exact));
        Self { policy, allowed, calls: Arc::new(Mutex::new(Vec::new())), seq: 0 }
    }

    /// Records of calls made since the last drain (one turn's worth).
    pub fn drain_calls(&self) -> Vec<CallRecord> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }

    pub fn is_allowed(&self, a: &Authority) -> bool {
        self.allowed.iter().any(|r| r.matches(a))
    }

    fn credential_for(&self, a: &Authority) -> Option<CredentialRule> {
        self.policy.credentials.iter().find(|c| c.applies_to(a)).cloned()
    }
}

fn push(calls: &Arc<Mutex<Vec<CallRecord>>>, r: CallRecord) {
    calls.lock().unwrap().push(r);
}

fn denial_response(authority: &str) -> http::Response<WasiBody> {
    let body = http_body_util::Full::new(Bytes::from(format!(
        "egress denied: destination '{authority}' is not allowed for this agent"
    )))
    .map_err(|never| match never {})
    .boxed_unsync();
    http::Response::builder()
        .status(http::StatusCode::FORBIDDEN)
        .header("content-type", "text/plain")
        .header("x-taffee-egress", "denied")
        .body(body)
        .expect("static response is valid")
}

/// Strip guest credentials and attach the host's. Buffers the body only for SigV4, which
/// signs over the payload hash.
async fn apply_credential(
    request: http::Request<WasiBody>,
    rule: &CredentialRule,
    authority: &Authority,
) -> std::result::Result<http::Request<WasiBody>, HttpError> {
    let (mut parts, body) = request.into_parts();
    for h in GUEST_SIGNATURE_HEADERS {
        parts.headers.remove(*h);
    }
    match &rule.kind {
        CredentialKind::Bearer { token } => {
            let value = http::HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| HttpError::InternalError(Some("invalid bearer token".into())))?;
            parts.headers.insert(http::header::AUTHORIZATION, value);
            Ok(http::Request::from_parts(parts, body))
        }
        CredentialKind::Sigv4 { region, service, creds } => {
            let bytes = body.collect().await?.to_bytes();
            let amz_date = sigv4::now_amz_date();
            let payload = sigv4::sha256_hex(&bytes);
            let host = authority.host_header();
            let mut signed: Vec<(String, String)> = vec![
                ("host".into(), host.clone()),
                ("x-amz-date".into(), amz_date.clone()),
                ("x-amz-content-sha256".into(), payload.clone()),
            ];
            if let Some(ct) = parts.headers.get(http::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()) {
                signed.push(("content-type".into(), ct.to_string()));
            }
            if let Some(tok) = &creds.session_token {
                signed.push(("x-amz-security-token".into(), tok.clone()));
            }
            let sig = sigv4::sign(
                &SigningInput {
                    method: parts.method.as_str(),
                    path: parts.uri.path(),
                    query: parts.uri.query(),
                    headers: &signed,
                    payload_sha256: &payload,
                    region,
                    service,
                    amz_date: &amz_date,
                },
                creds,
            );
            let bad = |_| HttpError::InternalError(Some("invalid signing header".into()));
            for (k, v) in &signed {
                let name = http::HeaderName::from_bytes(k.as_bytes()).map_err(|_| HttpError::InternalError(None))?;
                parts.headers.insert(name, http::HeaderValue::from_str(v).map_err(bad)?);
            }
            parts.headers.insert(http::header::AUTHORIZATION, http::HeaderValue::from_str(&sig.authorization).map_err(bad)?);
            parts.headers.insert(http::header::CONTENT_LENGTH, http::HeaderValue::from(bytes.len()));
            let body = http_body_util::Full::new(bytes).map_err(|never| match never {}).boxed_unsync();
            Ok(http::Request::from_parts(parts, body))
        }
    }
}

impl WasiHttpHooks for EgressGate {
    fn send_request(
        &mut self,
        request: http::Request<WasiBody>,
        options: Option<RequestOptions>,
        fut: Box<dyn std::future::Future<Output = std::result::Result<(), HttpError>> + Send>,
    ) -> Box<
        dyn std::future::Future<
                Output = std::result::Result<
                    (http::Response<WasiBody>, Box<dyn std::future::Future<Output = std::result::Result<(), HttpError>> + Send>),
                    HttpError,
                >,
            > + Send,
    > {
        _ = fut;
        self.seq += 1;
        let seq = self.seq;
        let start = Instant::now();
        let calls = self.calls.clone();
        let method = request.method().to_string();
        let path = request.uri().path().to_string();
        let authority = Authority::from_uri(request.uri());
        let display = authority.as_ref().map(|a| a.host_header()).unwrap_or_default();
        let scheme = authority.as_ref().map(|a| a.scheme.clone()).unwrap_or_default();

        let record = move |outcome, status, credential: Option<String>, error: Option<String>| CallRecord {
            seq,
            method: method.clone(),
            scheme: scheme.clone(),
            authority: display.clone(),
            path: path.clone(),
            outcome,
            status,
            duration_ms: start.elapsed().as_secs_f64() * 1000.0,
            credential,
            error,
        };

        let Some(authority) = authority.filter(|a| self.is_allowed(a)) else {
            let shown = record(CallOutcome::Denied, Some(403), None, None);
            tracing::warn!(authority = %shown.authority, "egress denied");
            let authority_text = shown.authority.clone();
            push(&calls, shown);
            return Box::new(async move {
                // Drain the guest's request body before answering: dropping it early aborts
                // the componentize-js event loop before JavaScript sees the 403.
                let mut body = request.into_body();
                while let Some(frame) = body.frame().await {
                    if frame.is_err() {
                        break;
                    }
                }
                let resp = denial_response(&authority_text);
                let io = Box::new(async { Ok(()) }) as Box<dyn std::future::Future<Output = _> + Send>;
                Ok((resp, io))
            });
        };

        let credential = self.credential_for(&authority);
        let via = self.policy.via.clone();
        Box::new(async move {
            let cred_name = credential.as_ref().map(|c| c.name.clone());
            let mut request = request;
            if let Some(rule) = &credential {
                request = match apply_credential(request, rule, &authority).await {
                    Ok(r) => r,
                    Err(e) => {
                        push(&calls, record(CallOutcome::Error, None, cred_name, Some(format!("{e:?}"))));
                        return Err(e);
                    }
                };
            }
            if let Some((via_addr, host_header)) = &via {
                let pq = request.uri().path_and_query().map(|p| p.as_str().to_string()).unwrap_or_else(|| "/".into());
                if let Ok(rewritten) = format!("http://{via_addr}{pq}").parse::<http::Uri>() {
                    *request.uri_mut() = rewritten;
                    if let Ok(v) = http::HeaderValue::from_str(host_header) {
                        request.headers_mut().insert(http::header::HOST, v);
                    }
                }
            }
            match wasmtime_wasi_http::default_send_request(request, options).await {
                Ok((res, io)) => {
                    push(&calls, record(CallOutcome::Sent, Some(res.status().as_u16()), cred_name, None));
                    Ok((res.map(BodyExt::boxed_unsync), Box::new(io) as Box<dyn std::future::Future<Output = _> + Send>))
                }
                Err(e) => {
                    push(&calls, record(CallOutcome::Error, None, cred_name, Some(format!("{e:?}"))));
                    Err(e)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_body() -> WasiBody {
        http_body_util::Empty::<Bytes>::new().map_err(|never| match never {}).boxed_unsync()
    }

    fn auth(url: &str) -> Authority {
        Authority::from_url(url).unwrap()
    }

    #[test]
    fn allow_rules_parse_and_match() {
        let r = AllowRule::parse("api.example.com").unwrap();
        assert!(r.matches(&auth("https://API.example.com/x")));
        assert!(r.matches(&auth("http://api.example.com:8080")));
        assert!(!r.matches(&auth("https://api.example.com.attacker.com")));

        let r = AllowRule::parse("127.0.0.1:8000").unwrap();
        assert!(r.matches(&auth("http://127.0.0.1:8000")));
        assert!(!r.matches(&auth("http://127.0.0.1:8001")));

        let r = AllowRule::parse("https://secure.example.com").unwrap();
        assert!(r.matches(&auth("https://secure.example.com")));
        assert!(!r.matches(&auth("http://secure.example.com")));

        assert!(AllowRule::parse("*").unwrap().matches(&auth("http://anything:1")));
        assert!(AllowRule::parse("").is_err());
        assert!(AllowRule::parse("host/path").is_err());
    }

    #[tokio::test]
    async fn denies_unlisted_destination_and_records_it() {
        let policy = Arc::new(EgressPolicy::default());
        let mut gate = EgressGate::new(policy, &[auth("http://127.0.0.1:8000")]);
        for uri in ["http://127.0.0.1:8001/v1/converse", "http://127.0.0.1.attacker.com:8000/steal"] {
            let req = http::Request::builder().uri(uri).body(empty_body()).unwrap();
            let fut = gate.send_request(req, None, Box::new(async { Ok(()) }));
            let (resp, _io) = Box::into_pin(fut).await.expect("denial is a response, not an error");
            assert_eq!(resp.status(), http::StatusCode::FORBIDDEN);
            assert_eq!(resp.headers().get("x-taffee-egress").unwrap(), "denied");
        }
        let calls = gate.drain_calls();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|c| c.outcome == CallOutcome::Denied));
        assert!(gate.drain_calls().is_empty(), "drain resets per turn");
    }

    #[tokio::test]
    async fn bearer_injection_replaces_guest_authorization() {
        let rule = CredentialRule::bearer("mcp".into(), auth("http://mcp:8765"), "host-secret-token".into());
        let req = http::Request::builder()
            .uri("http://mcp:8765/mcp")
            .header("authorization", "Bearer guest-supplied")
            .body(empty_body())
            .unwrap();
        let out = apply_credential(req, &rule, &auth("http://mcp:8765")).await.unwrap();
        assert_eq!(out.headers().get("authorization").unwrap(), "Bearer host-secret-token");
    }

    #[tokio::test]
    async fn sigv4_injection_resigns_with_host_credentials() {
        let creds = AwsCredentials {
            access_key_id: "AKIDHOSTKEY".into(),
            secret_access_key: "host-secret".into(),
            session_token: Some("host-session".into()),
        };
        let a = auth("https://bedrock-runtime.us-east-1.amazonaws.com");
        let rule = CredentialRule::sigv4("bedrock".into(), a.clone(), "us-east-1".into(), "bedrock".into(), creds);
        let body = http_body_util::Full::new(Bytes::from_static(b"{\"messages\":[]}")).map_err(|never| match never {}).boxed_unsync();
        let req = http::Request::builder()
            .method("POST")
            .uri("https://bedrock-runtime.us-east-1.amazonaws.com/model/m%3A0/converse")
            .header("authorization", "AWS4-HMAC-SHA256 Credential=TAFFEE_PLACEHOLDER/...")
            .header("x-amz-security-token", "guest-token")
            .header("content-type", "application/json")
            .body(body)
            .unwrap();
        let out = apply_credential(req, &rule, &a).await.unwrap();
        let authz = out.headers().get("authorization").unwrap().to_str().unwrap();
        assert!(authz.starts_with("AWS4-HMAC-SHA256 Credential=AKIDHOSTKEY/"));
        assert!(authz.contains("/us-east-1/bedrock/aws4_request"));
        assert!(authz.contains("SignedHeaders=content-type;host;x-amz-content-sha256;x-amz-date;x-amz-security-token"));
        assert_eq!(out.headers().get("x-amz-security-token").unwrap(), "host-session");
        assert_eq!(out.headers().get("content-length").unwrap(), "15");
        let sent = out.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&sent[..], b"{\"messages\":[]}");
    }

    #[test]
    fn credential_rule_debug_hides_secret() {
        let rule = CredentialRule::bearer("x".into(), auth("http://h"), "super-secret-value".into());
        assert!(!format!("{rule:?}").contains("super-secret-value"));
    }
}
