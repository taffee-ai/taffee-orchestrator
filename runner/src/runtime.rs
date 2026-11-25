//! The shared runtime: one pooled Engine and one pre-linked component for every agent on the
//! host, plus the per-agent worker that owns one long-lived instance.

use crate::egress::{CallRecord, EgressGate};
use crate::pool::{self, ComponentMetrics, PoolConfigOptions};
use anyhow::{anyhow, bail, Context, Result};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use wasmtime::component::{Component, ComponentExportIndex, InstancePre, Linker, ResourceTable, TypedFunc};
use wasmtime::{Engine, Store, UpdateDeadline};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpView};

/// The export every agent guest provides (strand-agent/wit/world.wit).
pub const WORKER_INTERFACE: &str = "taffee:agent/worker@0.1.0";
pub const RUN_TURN: &str = "run-turn";

/// How often the epoch advances. Running guests yield to the scheduler at every tick, and
/// turn deadlines are checked at the same points.
pub const EPOCH_TICK: Duration = Duration::from_millis(10);

pub struct HostState {
    pub table: ResourceTable,
    pub wasi: WasiCtx,
    pub http: WasiHttpCtx,
    pub gate: EgressGate,
    pub limits: wasmtime::StoreLimits,
    /// Set for the duration of one turn; checked at every epoch tick.
    pub deadline: Option<Instant>,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

impl WasiHttpView for HostState {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView { ctx: &mut self.http, table: &mut self.table, hooks: &mut self.gate }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    pub pool: PoolConfigOptions,
    /// Where compiled `.cwasm` files are cached. Defaults to next to the component.
    pub cache_dir: Option<PathBuf>,
    /// Pass the guest's stdout/stderr through to the host's (debugging only).
    pub guest_stdio: bool,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self { pool: PoolConfigOptions::default(), cache_dir: None, guest_stdio: false }
    }
}

pub struct Runtime {
    engine: Engine,
    pre: InstancePre<HostState>,
    run_turn: ComponentExportIndex,
    opts: RuntimeOptions,
    pub metrics: Option<ComponentMetrics>,
    pub component_bytes: usize,
    pub load_duration: Duration,
    stop_ticker: Arc<AtomicBool>,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop_ticker.store(true, Ordering::Relaxed);
    }
}

fn engine_hash(engine: &Engine) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    engine.precompile_compatibility_hash().hash(&mut h);
    format!("{:016x}", h.finish())
}

impl Runtime {
    /// Load a component file: admission check, then a compiled-code cache keyed by the
    /// engine's compatibility hash and the component's content hash.
    pub fn load(path: &Path, opts: RuntimeOptions) -> Result<Arc<Self>> {
        let start = Instant::now();
        let bytes = std::fs::read(path).with_context(|| format!("reading component {}", path.display()))?;
        let metrics = pool::inspect_and_validate_component(&bytes)?;
        let mut opts = opts;
        opts.pool = opts.pool.clone().fit(&metrics);
        let engine = pool::create_pooled_engine(&opts.pool)?;

        let content = &crate::sigv4::sha256_hex(&bytes)[..12];
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("component");
        let dir = opts.cache_dir.clone().unwrap_or_else(|| path.parent().map(Path::to_path_buf).unwrap_or_default());
        let cache = dir.join(format!("{stem}.{content}.{}.cwasm", engine_hash(&engine)));

        let component = match cache.exists().then(|| unsafe { Component::deserialize_file(&engine, &cache) }) {
            Some(Ok(c)) => c,
            _ => {
                let c = Component::new(&engine, &bytes)
                    .map_err(anyhow::Error::from)
                    .with_context(|| format!("compiling {}", path.display()))?;
                if let Ok(serialized) = c.serialize() {
                    let _ = std::fs::create_dir_all(&dir);
                    let tmp = cache.with_extension("cwasm.tmp");
                    if std::fs::write(&tmp, serialized).is_ok() {
                        let _ = std::fs::rename(&tmp, &cache);
                    }
                }
                c
            }
        };
        Self::from_component(engine, component, opts, Some(metrics), bytes.len(), start)
    }

    /// Build from an in-memory component (binary or WAT text). Used for test guests.
    pub fn from_bytes(bytes: &[u8], opts: RuntimeOptions) -> Result<Arc<Self>> {
        let start = Instant::now();
        let engine = pool::create_pooled_engine(&opts.pool)?;
        let component = Component::new(&engine, bytes)?;
        Self::from_component(engine, component, opts, None, bytes.len(), start)
    }

    fn from_component(
        engine: Engine,
        component: Component,
        opts: RuntimeOptions,
        metrics: Option<ComponentMetrics>,
        component_bytes: usize,
        start: Instant,
    ) -> Result<Arc<Self>> {
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)?;
        let pre = linker.instantiate_pre(&component)?;
        let iface = component
            .get_export_index(None, WORKER_INTERFACE)
            .ok_or_else(|| anyhow!("component does not export {WORKER_INTERFACE}"))?;
        let run_turn = component
            .get_export_index(Some(&iface), RUN_TURN)
            .ok_or_else(|| anyhow!("{WORKER_INTERFACE} does not export {RUN_TURN}"))?;

        let stop = Arc::new(AtomicBool::new(false));
        {
            let engine = engine.clone();
            let stop = stop.clone();
            std::thread::Builder::new().name("epoch-ticker".into()).spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(EPOCH_TICK);
                    engine.increment_epoch();
                }
            })?;
        }

        Ok(Arc::new(Self {
            engine,
            pre,
            run_turn,
            opts,
            metrics,
            component_bytes,
            load_duration: start.elapsed(),
            stop_ticker: stop,
        }))
    }

    pub fn max_agents(&self) -> usize {
        self.opts.pool.max_agents
    }

    /// Instantiate one agent. The instance is not called until its first turn.
    pub async fn spawn(self: &Arc<Self>, gate: EgressGate) -> Result<AgentWorker> {
        let mut wasi = WasiCtxBuilder::new();
        if self.opts.guest_stdio {
            wasi.inherit_stdout().inherit_stderr();
        }
        let mut store = Store::new(
            &self.engine,
            HostState {
                table: ResourceTable::new(),
                wasi: wasi.build(),
                http: WasiHttpCtx::new(),
                gate,
                limits: pool::create_store_limits(&self.opts.pool),
                deadline: None,
            },
        );
        store.limiter(|s| &mut s.limits);
        store.epoch_deadline_callback(|ctx| {
            if ctx.data().deadline.is_some_and(|d| Instant::now() >= d) {
                return Err(wasmtime::Error::new(TurnDeadline));
            }
            Ok(UpdateDeadline::Yield(1))
        });
        store.set_epoch_deadline(1);

        let start = Instant::now();
        let instance = self
            .pre
            .instantiate_async(&mut store)
            .await
            .map_err(anyhow::Error::from)
            .context("instantiating agent")?;
        let run_turn = instance.get_typed_func::<(String,), (String,)>(&mut store, &self.run_turn)?;
        Ok(AgentWorker { run_turn, store: Some(store), instantiate_duration: start.elapsed() })
    }
}

/// Marker error raised from the epoch callback when a turn runs past its deadline.
#[derive(Debug)]
pub struct TurnDeadline;
impl std::fmt::Display for TurnDeadline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("turn deadline exceeded")
    }
}
impl std::error::Error for TurnDeadline {}

/// Why a turn did not return guest output. After any of these the instance is unusable.
#[derive(Debug)]
pub enum TurnFailure {
    /// The turn passed its deadline (guest computing or waiting on the network).
    Timeout,
    /// The guest trapped (including running out of its memory limit).
    Trap(String),
    /// The guest returned something that is not the contract's JSON.
    BadOutput(String),
}

impl std::fmt::Display for TurnFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => f.write_str("turn deadline exceeded"),
            Self::Trap(m) => write!(f, "guest trapped: {m}"),
            Self::BadOutput(m) => write!(f, "guest returned invalid output: {m}"),
        }
    }
}

pub struct TurnOutput {
    /// The guest's JSON reply.
    pub output: serde_json::Value,
    pub calls: Vec<CallRecord>,
    pub duration: Duration,
}

/// One long-lived agent instance. `&mut self` on `invoke` means one turn at a time.
pub struct AgentWorker {
    run_turn: TypedFunc<(String,), (String,)>,
    store: Option<Store<HostState>>,
    pub instantiate_duration: Duration,
}

impl AgentWorker {
    pub fn is_poisoned(&self) -> bool {
        self.store.is_none()
    }

    /// Run one turn. On `Err` the instance has been dropped and the worker is poisoned;
    /// the returned call records still describe what it did before failing.
    pub async fn invoke(&mut self, input: &str, timeout: Duration) -> Result<TurnOutput, (TurnFailure, Vec<CallRecord>)> {
        let Some(mut store) = self.store.take() else {
            return Err((TurnFailure::Trap("worker already poisoned".into()), Vec::new()));
        };
        let start = Instant::now();
        store.data_mut().deadline = Some(start + timeout);
        store.set_epoch_deadline(1);

        // Two limits, one deadline: the epoch callback stops a guest that is computing, and
        // this timeout stops a guest that is waiting in a host call (for example on a slow
        // model), where no epoch check runs.
        let result = tokio::time::timeout(
            timeout,
            self.run_turn.call_async(&mut store, (input.to_string(),)),
        )
        .await;
        store.data_mut().deadline = None;
        let calls = store.data().gate.drain_calls();

        let failure = match result {
            Err(_) => TurnFailure::Timeout,
            Ok(Err(e)) if e.downcast_ref::<TurnDeadline>().is_some() || e.chain().any(|c| c.to_string().contains("turn deadline exceeded")) => {
                TurnFailure::Timeout
            }
            Ok(Err(e)) => TurnFailure::Trap(format!("{e:#}")),
            Ok(Ok((text,))) => match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) if v.is_object() => {
                    self.store = Some(store);
                    return Ok(TurnOutput { output: v, calls, duration: start.elapsed() });
                }
                Ok(_) => TurnFailure::BadOutput("not a JSON object".into()),
                Err(e) => TurnFailure::BadOutput(e.to_string()),
            },
        };
        drop(store);
        Err((failure, calls))
    }
}

/// Fail fast with a clear message if a component path is wrong.
pub fn require_file(path: &Path) -> Result<()> {
    if !path.is_file() {
        bail!("component not found at {}", path.display());
    }
    Ok(())
}
