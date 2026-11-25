//! Deployment configuration for `orchestratord`.
//!
//! Read from a JSON file (`TAFFEE_ORCH_CONFIG`), then individual `TAFFEE_ORCH_*` environment
//! variables override it. Secrets never appear in the file: credential rules name the
//! environment variable or file that holds each secret, and they are read at startup.

use crate::egress::{CredentialRule, EgressPolicy};
use crate::sigv4::AwsCredentials;
use crate::spec::{AgentSpec, Authority};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct FileConfig {
    pub listen: Option<String>,
    pub component: Option<PathBuf>,
    pub cache_dir: Option<PathBuf>,
    pub max_agents: Option<usize>,
    pub memory_limit_mb: Option<usize>,
    pub turn_timeout_secs: Option<u64>,
    pub idle_ttl_secs: Option<u64>,
    pub records_path: Option<PathBuf>,
    pub turn_history: Option<usize>,
    pub guest_stdio: Option<bool>,
    #[serde(default)]
    pub default_spec: Option<AgentSpec>,
    #[serde(default)]
    pub egress: EgressFileConfig,
    #[serde(default)]
    pub credentials: Vec<CredentialFileRule>,
    /// File with lines of `name token`, the same format taffee-context's MCP server uses.
    pub api_tokens_file: Option<PathBuf>,
    /// The work manifest API (`/v1/work`). Absent: only the agent API is served.
    #[serde(default)]
    pub work: Option<crate::work::engine::WorkConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct EgressFileConfig {
    /// Extra destinations every agent may reach, beyond those its own spec names.
    /// Entries: `host`, `host:port`, `scheme://host:port`, or `*` for any.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Optional route: send allowed requests to this `host:port` with this Host header. A
    /// generic hook for a later tunnel provider (for example a WireGuard gateway).
    pub via: Option<(String, String)>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialFileRule {
    Bearer {
        name: String,
        /// URL or `host:port` the token is attached for.
        destination: String,
        token_env: Option<String>,
        token_file: Option<PathBuf>,
    },
    AwsSigv4 {
        name: String,
        destination: String,
        region: String,
        service: String,
        access_key_id_env: String,
        secret_access_key_env: String,
        session_token_env: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    pub listen: SocketAddr,
    pub component: PathBuf,
    pub cache_dir: Option<PathBuf>,
    pub max_agents: usize,
    pub memory_limit_bytes: usize,
    pub turn_timeout: Duration,
    pub idle_ttl: Duration,
    pub records_path: Option<PathBuf>,
    pub turn_history: usize,
    pub guest_stdio: bool,
    pub default_spec: AgentSpec,
    pub egress: EgressPolicy,
    /// (name, token). Empty means the API is open (local development only).
    pub api_tokens: Vec<(String, String)>,
    pub work: Option<crate::work::engine::WorkConfig>,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8780".parse().unwrap(),
            component: PathBuf::from("../strand-agent/dist/strand-agent.wasm"),
            cache_dir: None,
            max_agents: 100,
            memory_limit_bytes: 64 * 1024 * 1024,
            turn_timeout: Duration::from_secs(120),
            idle_ttl: Duration::from_secs(15 * 60),
            records_path: None,
            turn_history: 100,
            guest_stdio: false,
            default_spec: AgentSpec::default(),
            egress: EgressPolicy::default(),
            api_tokens: Vec::new(),
            work: None,
        }
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn parse_destination(dest: &str) -> Result<Authority> {
    if dest.contains("://") {
        Authority::from_url(dest).map_err(|e| anyhow::anyhow!(e))
    } else {
        Authority::from_url(&format!("http://{dest}")).map_err(|e| anyhow::anyhow!(e))
    }
}

fn read_secret(env_name: Option<&str>, file: Option<&PathBuf>, what: &str) -> Result<String> {
    if let Some(name) = env_name {
        return env(name).with_context(|| format!("{what}: environment variable {name} is not set"));
    }
    if let Some(path) = file {
        let s = std::fs::read_to_string(path).with_context(|| format!("{what}: reading {}", path.display()))?;
        let s = s.trim().to_string();
        if s.is_empty() {
            bail!("{what}: {} is empty", path.display());
        }
        return Ok(s);
    }
    bail!("{what}: set token_env or token_file")
}

pub fn load_tokens_file(path: &PathBuf) -> Result<Vec<(String, String)>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next(), parts.next()) {
            (Some(name), Some(token), None) if token.len() >= 16 => out.push((name.to_string(), token.to_string())),
            _ => bail!("{}:{}: expected `name token` with a token of at least 16 characters", path.display(), i + 1),
        }
    }
    Ok(out)
}

impl OrchestratorConfig {
    pub fn from_env() -> Result<Self> {
        let file = match env("TAFFEE_ORCH_CONFIG") {
            Some(path) => {
                let text = std::fs::read_to_string(&path).with_context(|| format!("reading config {path}"))?;
                serde_json::from_str::<FileConfig>(&text).with_context(|| format!("parsing config {path}"))?
            }
            None => serde_json::from_str::<FileConfig>("{}")?,
        };
        Self::from_file_config(file)
    }

    pub fn from_file_config(f: FileConfig) -> Result<Self> {
        let mut c = OrchestratorConfig::default();
        if let Some(v) = env("TAFFEE_ORCH_LISTEN").or(f.listen) {
            c.listen = v.parse().with_context(|| format!("listen address '{v}'"))?;
        }
        if let Some(v) = env("TAFFEE_ORCH_COMPONENT").map(PathBuf::from).or(f.component) {
            c.component = v;
        }
        c.cache_dir = env("TAFFEE_ORCH_CACHE_DIR").map(PathBuf::from).or(f.cache_dir);
        if let Some(v) = env("TAFFEE_ORCH_MAX_AGENTS").map(|v| v.parse()).transpose()?.or(f.max_agents) {
            c.max_agents = v;
        }
        if let Some(v) = env("TAFFEE_ORCH_MEMORY_LIMIT_MB").map(|v| v.parse()).transpose()?.or(f.memory_limit_mb) {
            c.memory_limit_bytes = v * 1024 * 1024;
        }
        if let Some(v) = env("TAFFEE_ORCH_TURN_TIMEOUT_SECS").map(|v| v.parse()).transpose()?.or(f.turn_timeout_secs) {
            c.turn_timeout = Duration::from_secs(v);
        }
        if let Some(v) = env("TAFFEE_ORCH_IDLE_TTL_SECS").map(|v| v.parse()).transpose()?.or(f.idle_ttl_secs) {
            c.idle_ttl = Duration::from_secs(v);
        }
        c.records_path = env("TAFFEE_ORCH_RECORDS").map(PathBuf::from).or(f.records_path);
        if let Some(v) = f.turn_history {
            c.turn_history = v;
        }
        c.guest_stdio = env("TAFFEE_ORCH_GUEST_STDIO").map(|v| v == "1" || v == "true").or(f.guest_stdio).unwrap_or(false);

        let mut spec = f.default_spec.unwrap_or_default();
        if let Some(e) = env("TAFFEE_ORCH_MODEL_ENDPOINT") {
            spec.model.endpoint = Some(e);
        }
        if let Some(m) = env("TAFFEE_ORCH_MODEL_ID") {
            spec.model.model_id = Some(m);
        }
        spec.validate().map_err(|e| anyhow::anyhow!("default_spec: {e}"))?;
        c.default_spec = spec;

        let mut allow = f.egress.allow;
        if let Some(extra) = env("TAFFEE_ORCH_EGRESS_ALLOW") {
            allow.extend(extra.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
        }
        let mut rules = Vec::new();
        for r in f.credentials {
            rules.push(match r {
                CredentialFileRule::Bearer { name, destination, token_env, token_file } => {
                    let token = read_secret(token_env.as_deref(), token_file.as_ref(), &format!("credential '{name}'"))?;
                    CredentialRule::bearer(name, parse_destination(&destination)?, token)
                }
                CredentialFileRule::AwsSigv4 {
                    name,
                    destination,
                    region,
                    service,
                    access_key_id_env,
                    secret_access_key_env,
                    session_token_env,
                } => {
                    let what = format!("credential '{name}'");
                    let creds = AwsCredentials {
                        access_key_id: read_secret(Some(access_key_id_env.as_str()), None, &what)?,
                        secret_access_key: read_secret(Some(secret_access_key_env.as_str()), None, &what)?,
                        session_token: session_token_env.as_deref().and_then(env),
                    };
                    CredentialRule::sigv4(name, parse_destination(&destination)?, region, service, creds)
                }
            });
        }
        c.egress = EgressPolicy::new(allow, rules, f.egress.via)?;

        c.work = f.work;
        if let Some(dir) = env("TAFFEE_ORCH_KINDS_DIR") {
            let mut w = match c.work.take() {
                Some(w) => w,
                None => serde_json::from_value(serde_json::json!({ "kinds_dir": dir }))?,
            };
            w.kinds_dir = PathBuf::from(dir);
            c.work = Some(w);
        }

        let tokens_file = env("TAFFEE_ORCH_API_TOKENS_FILE").map(PathBuf::from).or(f.api_tokens_file);
        if let Some(p) = tokens_file {
            c.api_tokens = load_tokens_file(&p)?;
        }
        if c.max_agents == 0 {
            bail!("max_agents must be at least 1");
        }
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<OrchestratorConfig> {
        OrchestratorConfig::from_file_config(serde_json::from_str(json)?)
    }

    #[test]
    fn credential_rules_resolve_secrets_from_env() {
        std::env::set_var("TAFFEE_TEST_BEARER", "bearer-secret-value");
        std::env::set_var("TAFFEE_TEST_AKID", "AKIDFROMENV");
        std::env::set_var("TAFFEE_TEST_SECRET", "secret-from-env");
        let c = parse(
            r#"{
              "max_agents": 7,
              "egress": {"allow": ["api.example.com"]},
              "credentials": [
                {"kind": "bearer", "name": "mcp", "destination": "http://mcp:8765", "token_env": "TAFFEE_TEST_BEARER"},
                {"kind": "aws_sigv4", "name": "bedrock", "destination": "https://bedrock-runtime.us-east-1.amazonaws.com",
                 "region": "us-east-1", "service": "bedrock",
                 "access_key_id_env": "TAFFEE_TEST_AKID", "secret_access_key_env": "TAFFEE_TEST_SECRET"}
              ]
            }"#,
        )
        .unwrap();
        assert_eq!(c.max_agents, 7);
        assert_eq!(c.egress.credential_names(), vec!["mcp", "bedrock"]);
        assert_eq!(c.egress.credentials[0].destination.port, 8765);
        assert_eq!(c.egress.credentials[1].destination.port, 443);
        assert!(!format!("{:?}", c.egress).contains("bearer-secret-value"));
    }

    #[test]
    fn missing_secret_is_a_startup_error() {
        let err = parse(r#"{"credentials": [{"kind": "bearer", "name": "x", "destination": "h:1", "token_env": "TAFFEE_TEST_UNSET_VAR"}]}"#)
            .unwrap_err();
        assert!(err.to_string().contains("TAFFEE_TEST_UNSET_VAR"));
    }

    #[test]
    fn unknown_fields_and_bad_values_are_rejected() {
        assert!(parse(r#"{"max_agentz": 1}"#).is_err());
        assert!(parse(r#"{"max_agents": 0}"#).is_err());
        assert!(parse(r#"{"default_spec": {"tools": ["shell"]}}"#).is_err());
        assert!(parse(r#"{"egress": {"allow": ["bad/entry"]}}"#).is_err());
    }

    #[test]
    fn tokens_file_format() {
        let dir = std::env::temp_dir().join(format!("taffee-tokens-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("tokens");
        std::fs::write(&p, "# comment\nalice 0123456789abcdef0123\n\nbob fedcba9876543210fedc\n").unwrap();
        let t = load_tokens_file(&p).unwrap();
        assert_eq!(t.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), vec!["alice", "bob"]);
        std::fs::write(&p, "carol short\n").unwrap();
        assert!(load_tokens_file(&p).is_err());
    }
}
