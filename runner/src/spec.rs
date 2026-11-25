//! The agent spec: what an agent is (model, tools, limits). It is sent to the guest on every
//! turn, binds on the first one, and also decides which destinations the agent may reach.
//! Mirrors `AgentSpec` in strand-agent/src/agent.ts.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerSpec {
    pub name: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LimitsSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_output_chars: Option<u32>,
}

/// Structured output: the guest adds a built-in `submit` tool with this input schema, and
/// returns what the model submitted as `submitted` in the turn reply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputSpec {
    pub schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSpec {
    #[serde(default)]
    pub model: ModelSpec,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServerSpec>,
    #[serde(default)]
    pub limits: LimitsSpec,
    /// Prior conversation to seed the agent with (Bedrock message shape). Sent on the first
    /// turn only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<OutputSpec>,
}

/// Built-in tools the guest knows. `http_request`, `notebook` and `subagent` are Strands'
/// own (vended-tools); `ask_model` calls any model by name through the agent's chat gateway.
pub const BUILTIN_TOOLS: &[&str] = &["calculator", "http_request", "notebook", "subagent", "ask_model"];
/// `bedrock`: Converse. `openai-chat`: OpenAI-style chat completions (OpenRouter, LiteLLM
/// and most gateways); `endpoint` is the base URL up to the version.
pub const MODEL_PROVIDERS: &[&str] = &["bedrock", "openai-chat"];
const MAX_SYSTEM_PROMPT: usize = 64 * 1024;
const MAX_SEED_MESSAGES: usize = 500;

impl AgentSpec {
    /// Fill unset fields from the deployment's defaults. Collections set on the request win
    /// whole; they are not merged element by element.
    pub fn with_defaults(mut self, d: &AgentSpec) -> AgentSpec {
        let m = &mut self.model;
        m.provider = m.provider.take().or_else(|| d.model.provider.clone());
        m.model_id = m.model_id.take().or_else(|| d.model.model_id.clone());
        m.region = m.region.take().or_else(|| d.model.region.clone());
        m.endpoint = m.endpoint.take().or_else(|| d.model.endpoint.clone());
        m.max_tokens = m.max_tokens.or(d.model.max_tokens);
        m.temperature = m.temperature.or(d.model.temperature);
        if self.system_prompt.is_none() {
            self.system_prompt = d.system_prompt.clone();
        }
        if self.tools.is_none() {
            self.tools = d.tools.clone();
        }
        if self.mcp_servers.is_empty() {
            self.mcp_servers = d.mcp_servers.clone();
        }
        let l = &mut self.limits;
        l.turns = l.turns.or(d.limits.turns);
        l.total_tokens = l.total_tokens.or(d.limits.total_tokens);
        l.output_tokens = l.output_tokens.or(d.limits.output_tokens);
        l.tool_output_chars = l.tool_output_chars.or(d.limits.tool_output_chars);
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if let Some(p) = &self.model.provider {
            if !MODEL_PROVIDERS.contains(&p.as_str()) {
                return Err(format!("model.provider '{p}' is not supported (supported: {})", MODEL_PROVIDERS.join(", ")));
            }
        }
        if let Some(e) = &self.model.endpoint {
            Authority::from_url(e).map_err(|err| format!("model.endpoint: {err}"))?;
        }
        if let Some(tools) = &self.tools {
            for t in tools {
                if !BUILTIN_TOOLS.contains(&t.as_str()) {
                    return Err(format!("unknown built-in tool '{t}' (known: {})", BUILTIN_TOOLS.join(", ")));
                }
            }
        }
        let mut names = std::collections::HashSet::new();
        for s in &self.mcp_servers {
            if s.name.is_empty() || !names.insert(s.name.as_str()) {
                return Err(format!("mcpServers: empty or duplicate name '{}'", s.name));
            }
            Authority::from_url(&s.url).map_err(|err| format!("mcpServers[{}].url: {err}", s.name))?;
        }
        if self.system_prompt.as_ref().is_some_and(|p| p.len() > MAX_SYSTEM_PROMPT) {
            return Err(format!("systemPrompt is longer than {MAX_SYSTEM_PROMPT} bytes"));
        }
        if self.messages.as_ref().is_some_and(|m| m.len() > MAX_SEED_MESSAGES) {
            return Err(format!("messages has more than {MAX_SEED_MESSAGES} entries"));
        }
        if let Some(o) = &self.output {
            if o.schema.get("type").and_then(Value::as_str) != Some("object") {
                return Err("output.schema must be a JSON Schema with type object".into());
            }
        }
        if self.limits.turns == Some(0) || self.limits.total_tokens == Some(0) || self.limits.output_tokens == Some(0) {
            return Err("limits must be positive when set".into());
        }
        Ok(())
    }

    /// Destinations this agent's spec names. The agent may reach these and nothing else
    /// (plus the deployment's global allow list).
    pub fn authorities(&self) -> Vec<Authority> {
        let mut out = Vec::new();
        if let Some(e) = &self.model.endpoint {
            if let Ok(a) = Authority::from_url(e) {
                out.push(a);
            }
        }
        for s in &self.mcp_servers {
            if let Ok(a) = Authority::from_url(&s.url) {
                out.push(a);
            }
        }
        out
    }

    /// Stable digest of the binding part of the spec (everything except seed messages).
    pub fn digest(&self) -> String {
        let mut binding = self.clone();
        binding.messages = None;
        let json = serde_json::to_vec(&binding).unwrap_or_default();
        crate::sigv4::sha256_hex(&json)[..16].to_string()
    }
}

/// scheme + lowercase host + effective port.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Authority {
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

impl Authority {
    pub fn from_url(url: &str) -> Result<Self, String> {
        let uri: http::Uri = url.parse().map_err(|e| format!("invalid URL '{url}': {e}"))?;
        let scheme = uri.scheme_str().ok_or_else(|| format!("URL '{url}' has no scheme"))?.to_ascii_lowercase();
        if scheme != "http" && scheme != "https" {
            return Err(format!("URL '{url}' must be http or https"));
        }
        let host = uri.host().ok_or_else(|| format!("URL '{url}' has no host"))?.to_ascii_lowercase();
        let port = uri.port_u16().unwrap_or(if scheme == "https" { 443 } else { 80 });
        Ok(Self { scheme, host, port })
    }

    pub fn from_uri(uri: &http::Uri) -> Option<Self> {
        let scheme = uri.scheme_str().unwrap_or("http").to_ascii_lowercase();
        let host = uri.host()?.to_ascii_lowercase();
        let port = uri.port_u16().unwrap_or(if scheme == "https" { 443 } else { 80 });
        Some(Self { scheme, host, port })
    }

    /// `host` when the port is the scheme default, else `host:port`.
    pub fn host_header(&self) -> String {
        let default = if self.scheme == "https" { 443 } else { 80 };
        if self.port == default {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_fill_only_unset_fields() {
        let d: AgentSpec = serde_json::from_value(serde_json::json!({
            "model": {"modelId": "default-model", "endpoint": "http://mock:8000", "region": "eu-west-1"},
            "tools": ["calculator"],
            "limits": {"turns": 8}
        }))
        .unwrap();
        let s: AgentSpec = serde_json::from_value(serde_json::json!({
            "model": {"modelId": "mine"},
            "limits": {"totalTokens": 1000}
        }))
        .unwrap();
        let merged = s.with_defaults(&d);
        assert_eq!(merged.model.model_id.as_deref(), Some("mine"));
        assert_eq!(merged.model.endpoint.as_deref(), Some("http://mock:8000"));
        assert_eq!(merged.limits.turns, Some(8));
        assert_eq!(merged.limits.total_tokens, Some(1000));
        assert_eq!(merged.tools, Some(vec!["calculator".to_string()]));
    }

    #[test]
    fn validate_rejects_bad_specs() {
        let bad = |v: serde_json::Value| serde_json::from_value::<AgentSpec>(v).unwrap().validate().is_err();
        assert!(bad(serde_json::json!({"model": {"provider": "openai"}})));
        assert!(bad(serde_json::json!({"model": {"endpoint": "ftp://x"}})));
        assert!(bad(serde_json::json!({"tools": ["rm_rf"]})));
        assert!(bad(serde_json::json!({"limits": {"turns": 0}})));
        assert!(bad(serde_json::json!({"mcpServers": [{"name": "a", "url": "http://a"}, {"name": "a", "url": "http://b"}]})));
        assert!(serde_json::from_value::<AgentSpec>(serde_json::json!({"unknownField": 1})).is_err());
    }

    #[test]
    fn authorities_normalize_case_and_default_ports() {
        let a = Authority::from_url("HTTPS://Bedrock-Runtime.us-east-1.amazonaws.com/model/x").unwrap();
        assert_eq!(a, Authority { scheme: "https".into(), host: "bedrock-runtime.us-east-1.amazonaws.com".into(), port: 443 });
        assert_eq!(a.host_header(), "bedrock-runtime.us-east-1.amazonaws.com");
        let b = Authority::from_url("http://127.0.0.1:8000").unwrap();
        assert_eq!(b.host_header(), "127.0.0.1:8000");
    }

    #[test]
    fn digest_ignores_seed_messages() {
        let mut a = AgentSpec::default();
        a.system_prompt = Some("x".into());
        let mut b = a.clone();
        b.messages = Some(vec![serde_json::json!({"role": "user", "content": [{"text": "hi"}]})]);
        assert_eq!(a.digest(), b.digest());
        b.system_prompt = Some("y".into());
        assert_ne!(a.digest(), b.digest());
    }
}
