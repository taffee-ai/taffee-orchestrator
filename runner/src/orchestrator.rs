//! The orchestrator: owns every agent on the host.
//!
//! An agent is one long-lived Wasm instance bound to one spec. The orchestrator admits agents
//! up to the pool's capacity, runs one turn at a time per agent under a deadline, replaces
//! nothing silently (an agent whose instance fails is removed and the caller is told), evicts
//! idle agents, and records every turn.

use crate::config::OrchestratorConfig;
use crate::egress::{CallRecord, EgressGate, EgressPolicy};
use crate::metrics::MemorySample;
use crate::pool::PoolConfigOptions;
use crate::records::{unix_ms, RecordSink, TurnRecord};
use crate::runtime::{AgentWorker, Runtime, RuntimeOptions, TurnFailure};
use crate::spec::AgentSpec;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};

/// A request's intent record (taffee-core). Carried and recorded; this
/// orchestrator does not plan from it yet.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Intent {
    #[serde(default)]
    pub id: Option<String>,
    pub objective: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_criteria: Option<Value>,
}

#[derive(Debug)]
pub enum OrchError {
    NotFound,
    AtCapacity(usize),
    Busy,
    InvalidSpec(String),
    Timeout(Vec<CallRecord>),
    AgentLost(String, Vec<CallRecord>),
    Internal(String),
}

impl OrchError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::AtCapacity(_) => "at_capacity",
            Self::Busy => "busy",
            Self::InvalidSpec(_) => "invalid_spec",
            Self::Timeout(_) => "timeout",
            Self::AgentLost(..) => "agent_lost",
            Self::Internal(_) => "internal",
        }
    }
    pub fn http_status(&self) -> u16 {
        match self {
            Self::NotFound => 404,
            Self::AtCapacity(_) => 429,
            Self::Busy => 409,
            Self::InvalidSpec(_) => 400,
            Self::Timeout(_) => 504,
            Self::AgentLost(..) => 502,
            Self::Internal(_) => 500,
        }
    }
    pub fn calls(&self) -> &[CallRecord] {
        match self {
            Self::Timeout(c) | Self::AgentLost(_, c) => c,
            _ => &[],
        }
    }
}

impl std::fmt::Display for OrchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("agent not found"),
            Self::AtCapacity(n) => write!(f, "at capacity: {n} agents are running"),
            Self::Busy => f.write_str("agent is busy with another turn"),
            Self::InvalidSpec(m) => write!(f, "invalid spec: {m}"),
            Self::Timeout(_) => f.write_str("turn deadline exceeded; the agent was removed"),
            Self::AgentLost(m, _) => write!(f, "agent instance failed and was removed: {m}"),
            Self::Internal(m) => write!(f, "internal error: {m}"),
        }
    }
}

impl std::error::Error for OrchError {}

#[derive(Debug, Clone, Serialize)]
pub struct AgentInfo {
    pub agent_id: String,
    pub spec_digest: String,
    pub created_at_ms: u64,
    pub last_used_ms: u64,
    pub turns: u64,
    pub busy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requester: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<Intent>,
    pub spec: AgentSpec,
}

#[derive(Debug, Clone, Serialize)]
pub struct TurnResponse {
    pub agent_id: String,
    pub turn: u64,
    /// ok, or the guest's errorKind (turn_failed, spec_conflict, spec_invalid)
    pub outcome: String,
    pub result: Option<String>,
    pub stop_reason: Option<String>,
    pub usage: Option<Value>,
    pub message_count: Option<u64>,
    pub error: Option<String>,
    /// What the model passed to the `submit` tool, when the spec asked for structured output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submitted: Option<Value>,
    pub calls: Vec<CallRecord>,
    pub duration_ms: f64,
}

struct AgentSlot {
    id: String,
    spec: AgentSpec,
    spec_digest: String,
    created_at_ms: u64,
    requester: Option<String>,
    intent: Option<Intent>,
    worker: tokio::sync::Mutex<AgentWorker>,
    last_used: Mutex<(Instant, u64)>,
    turns: AtomicU64,
    history: Mutex<VecDeque<TurnRecord>>,
    _permit: OwnedSemaphorePermit,
}

impl AgentSlot {
    fn info(&self, busy: bool) -> AgentInfo {
        let mut spec = self.spec.clone();
        spec.messages = None;
        AgentInfo {
            agent_id: self.id.clone(),
            spec_digest: self.spec_digest.clone(),
            created_at_ms: self.created_at_ms,
            last_used_ms: self.last_used.lock().unwrap().1,
            turns: self.turns.load(Ordering::Relaxed),
            busy,
            requester: self.requester.clone(),
            intent: self.intent.clone(),
            spec,
        }
    }
}

#[derive(Default)]
struct Counters {
    agents_created: AtomicU64,
    agents_removed_failed: AtomicU64,
    agents_evicted_idle: AtomicU64,
    turns_ok: AtomicU64,
    turns_failed: AtomicU64,
    turns_timed_out: AtomicU64,
    rejected_capacity: AtomicU64,
}

pub struct Orchestrator {
    runtime: Arc<Runtime>,
    policy: Arc<EgressPolicy>,
    default_spec: AgentSpec,
    turn_timeout: Duration,
    idle_ttl: Duration,
    turn_history: usize,
    agents: RwLock<HashMap<String, Arc<AgentSlot>>>,
    capacity: Arc<Semaphore>,
    counters: Counters,
    sink: RecordSink,
    started: Instant,
}

impl Orchestrator {
    pub fn new(cfg: &OrchestratorConfig) -> anyhow::Result<Arc<Self>> {
        let runtime = Runtime::load(
            &cfg.component,
            RuntimeOptions {
                pool: PoolConfigOptions { max_agents: cfg.max_agents, max_memory_bytes: cfg.memory_limit_bytes, ..Default::default() },
                cache_dir: cfg.cache_dir.clone(),
                guest_stdio: cfg.guest_stdio,
            },
        )?;
        Self::with_runtime(runtime, cfg)
    }

    pub fn with_runtime(runtime: Arc<Runtime>, cfg: &OrchestratorConfig) -> anyhow::Result<Arc<Self>> {
        let capacity = runtime.max_agents();
        Ok(Arc::new(Self {
            runtime,
            policy: Arc::new(cfg.egress.clone()),
            default_spec: cfg.default_spec.clone(),
            turn_timeout: cfg.turn_timeout,
            idle_ttl: cfg.idle_ttl,
            turn_history: cfg.turn_history,
            agents: RwLock::new(HashMap::new()),
            capacity: Arc::new(Semaphore::new(capacity)),
            counters: Counters::default(),
            sink: RecordSink::open(cfg.records_path.as_ref())?,
            started: Instant::now(),
        }))
    }

    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }

    pub async fn create_agent(
        &self,
        spec: AgentSpec,
        intent: Option<Intent>,
        requester: Option<String>,
    ) -> Result<AgentInfo, OrchError> {
        let spec = spec.with_defaults(&self.default_spec);
        spec.validate().map_err(OrchError::InvalidSpec)?;
        let permit = self.capacity.clone().try_acquire_owned().map_err(|_| {
            self.counters.rejected_capacity.fetch_add(1, Ordering::Relaxed);
            OrchError::AtCapacity(self.runtime.max_agents())
        })?;
        let gate = EgressGate::new(self.policy.clone(), &spec.authorities());
        let worker = self.runtime.spawn(gate).await.map_err(|e| OrchError::Internal(format!("{e:#}")))?;
        let intent = intent.map(|mut i| {
            i.id.get_or_insert_with(|| format!("intent-{}", uuid::Uuid::new_v4().simple()));
            i
        });
        let now = unix_ms();
        let slot = Arc::new(AgentSlot {
            id: format!("ag-{}", uuid::Uuid::new_v4().simple()),
            spec_digest: spec.digest(),
            spec,
            created_at_ms: now,
            requester,
            intent,
            worker: tokio::sync::Mutex::new(worker),
            last_used: Mutex::new((Instant::now(), now)),
            turns: AtomicU64::new(0),
            history: Mutex::new(VecDeque::new()),
            _permit: permit,
        });
        self.counters.agents_created.fetch_add(1, Ordering::Relaxed);
        let info = slot.info(false);
        self.agents.write().await.insert(slot.id.clone(), slot);
        Ok(info)
    }

    async fn slot(&self, id: &str) -> Result<Arc<AgentSlot>, OrchError> {
        self.agents.read().await.get(id).cloned().ok_or(OrchError::NotFound)
    }

    pub async fn agent(&self, id: &str) -> Result<AgentInfo, OrchError> {
        let slot = self.slot(id).await?;
        let busy = slot.worker.try_lock().is_err();
        Ok(slot.info(busy))
    }

    pub async fn list_agents(&self) -> Vec<AgentInfo> {
        let agents = self.agents.read().await;
        let mut out: Vec<AgentInfo> = agents.values().map(|s| s.info(s.worker.try_lock().is_err())).collect();
        out.sort_by_key(|a| a.created_at_ms);
        out
    }

    pub async fn delete_agent(&self, id: &str) -> Result<(), OrchError> {
        self.agents.write().await.remove(id).map(|_| ()).ok_or(OrchError::NotFound)
    }

    pub async fn turn_records(&self, id: &str) -> Result<Vec<TurnRecord>, OrchError> {
        let slot = self.slot(id).await?;
        let records = slot.history.lock().unwrap().iter().cloned().collect();
        Ok(records)
    }

    fn record(&self, slot: &AgentSlot, r: TurnRecord) {
        self.sink.write(&r);
        let mut h = slot.history.lock().unwrap();
        h.push_back(r);
        while h.len() > self.turn_history {
            h.pop_front();
        }
    }

    /// Run one turn on an agent. Turns on one agent never overlap: a second caller gets Busy.
    pub async fn turn(&self, id: &str, prompt: &str) -> Result<TurnResponse, OrchError> {
        let slot = self.slot(id).await?;
        let mut worker = slot.worker.try_lock().map_err(|_| OrchError::Busy)?;
        let turn = slot.turns.fetch_add(1, Ordering::Relaxed) + 1;

        // The spec goes with every turn (the guest checks it is unchanged); seed messages
        // only with the first.
        let mut spec = slot.spec.clone();
        if turn > 1 {
            spec.messages = None;
        }
        let input = json!({ "prompt": prompt, "spec": spec }).to_string();
        let started_at_ms = unix_ms();
        let result = worker.invoke(&input, self.turn_timeout).await;
        *slot.last_used.lock().unwrap() = (Instant::now(), unix_ms());

        let base = |outcome: &str, duration_ms: f64, calls: Vec<CallRecord>| TurnRecord {
            agent_id: slot.id.clone(),
            turn,
            spec_digest: slot.spec_digest.clone(),
            requester: slot.requester.clone(),
            intent_id: slot.intent.as_ref().and_then(|i| i.id.clone()),
            started_at_ms,
            duration_ms,
            outcome: outcome.to_string(),
            stop_reason: None,
            usage: None,
            calls,
        };

        match result {
            Ok(out) => {
                let o = &out.output;
                let ok = o["status"] == "ok";
                let outcome = if ok { "ok".to_string() } else { o["errorKind"].as_str().unwrap_or("turn_failed").to_string() };
                let duration_ms = out.duration.as_secs_f64() * 1000.0;
                let usage = o.get("usage").filter(|u| !u.is_null()).cloned();
                let stop_reason = o["stopReason"].as_str().map(String::from);
                let mut rec = base(&outcome, duration_ms, out.calls.clone());
                rec.stop_reason = stop_reason.clone();
                rec.usage = usage.clone();
                self.record(&slot, rec);
                if ok {
                    self.counters.turns_ok.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.counters.turns_failed.fetch_add(1, Ordering::Relaxed);
                }
                Ok(TurnResponse {
                    agent_id: slot.id.clone(),
                    turn,
                    outcome,
                    result: o["result"].as_str().map(String::from),
                    stop_reason,
                    usage,
                    message_count: o["messageCount"].as_u64(),
                    error: o["message"].as_str().map(String::from),
                    submitted: o.get("submitted").filter(|v| !v.is_null()).cloned(),
                    calls: out.calls,
                    duration_ms,
                })
            }
            Err((failure, calls)) => {
                drop(worker);
                self.agents.write().await.remove(&slot.id);
                self.counters.agents_removed_failed.fetch_add(1, Ordering::Relaxed);
                let (outcome, err) = match failure {
                    TurnFailure::Timeout => {
                        self.counters.turns_timed_out.fetch_add(1, Ordering::Relaxed);
                        ("timeout", OrchError::Timeout(calls.clone()))
                    }
                    TurnFailure::Trap(m) => {
                        self.counters.turns_failed.fetch_add(1, Ordering::Relaxed);
                        ("trap", OrchError::AgentLost(m, calls.clone()))
                    }
                    TurnFailure::BadOutput(m) => {
                        self.counters.turns_failed.fetch_add(1, Ordering::Relaxed);
                        ("bad_output", OrchError::AgentLost(format!("invalid guest output: {m}"), calls.clone()))
                    }
                };
                tracing::warn!(agent = %slot.id, turn, outcome, "agent removed after failed turn");
                self.record(&slot, base(outcome, unix_ms().saturating_sub(started_at_ms) as f64, calls));
                Err(err)
            }
        }
    }

    /// One-shot run: create an agent, run one turn, remove it.
    pub async fn run_once(
        &self,
        spec: AgentSpec,
        prompt: &str,
        intent: Option<Intent>,
        requester: Option<String>,
    ) -> Result<TurnResponse, OrchError> {
        let info = self.create_agent(spec, intent, requester).await?;
        let res = self.turn(&info.agent_id, prompt).await;
        let _ = self.delete_agent(&info.agent_id).await;
        res
    }

    /// Remove agents idle longer than the TTL. Busy agents are never evicted.
    pub async fn evict_idle(&self) -> usize {
        let now = Instant::now();
        let mut agents = self.agents.write().await;
        let stale: Vec<String> = agents
            .values()
            .filter(|s| s.worker.try_lock().is_ok() && now.duration_since(s.last_used.lock().unwrap().0) >= self.idle_ttl)
            .map(|s| s.id.clone())
            .collect();
        for id in &stale {
            agents.remove(id);
        }
        self.counters.agents_evicted_idle.fetch_add(stale.len() as u64, Ordering::Relaxed);
        stale.len()
    }

    pub fn spawn_reaper(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let weak = Arc::downgrade(self);
        let period = (self.idle_ttl / 4).clamp(Duration::from_secs(1), Duration::from_secs(30));
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(period);
            loop {
                tick.tick().await;
                let Some(o) = weak.upgrade() else { break };
                let n = o.evict_idle().await;
                if n > 0 {
                    tracing::info!(evicted = n, "evicted idle agents");
                }
            }
        })
    }

    pub async fn stats(&self) -> Value {
        let c = &self.counters;
        let active = self.agents.read().await.len();
        let mem = MemorySample::current();
        let m = self.runtime.metrics.as_ref();
        json!({
            "uptime_secs": self.started.elapsed().as_secs(),
            "agents": {
                "active": active,
                "capacity": self.runtime.max_agents(),
                "created": c.agents_created.load(Ordering::Relaxed),
                "removed_after_failure": c.agents_removed_failed.load(Ordering::Relaxed),
                "evicted_idle": c.agents_evicted_idle.load(Ordering::Relaxed),
                "rejected_at_capacity": c.rejected_capacity.load(Ordering::Relaxed),
            },
            "turns": {
                "ok": c.turns_ok.load(Ordering::Relaxed),
                "failed": c.turns_failed.load(Ordering::Relaxed),
                "timed_out": c.turns_timed_out.load(Ordering::Relaxed),
            },
            "process": {
                "rss_mb": mem.rss_mb(),
                "pss_mb": mem.pss_mb(),
                "private_dirty_mb": mem.private_dirty_mb(),
            },
            "component": {
                "bytes": self.runtime.component_bytes,
                "load_ms": self.runtime.load_duration.as_secs_f64() * 1000.0,
                "core_instances": m.map(|m| m.core_instances),
                "memories": m.map(|m| m.memories),
                "initial_memory_mb": m.map(|m| m.max_initial_memory_pages * 64 / 1024),
            },
            "egress": {
                "credentials": self.policy.credential_names(),
                "via": self.policy.via.as_ref().map(|v| v.0.clone()),
            },
            "limits": {
                "turn_timeout_secs": self.turn_timeout.as_secs(),
                "idle_ttl_secs": self.idle_ttl.as_secs(),
            }
        })
    }
}
