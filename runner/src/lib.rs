//! taffee-orchestrator: many long-lived, isolated Wasm agents per host on Wasmtime 49.
//!
//! - [`runtime`]: one pooled engine and pre-linked component shared by all agents; per-agent
//!   instances with memory limits and epoch-based turn deadlines.
//! - [`egress`]: the host side of every guest HTTP call: per-agent destination allow list,
//!   host-held credential injection (Bearer, AWS SigV4) and call records.
//! - [`orchestrator`]: the agent registry: admission, one turn at a time per agent, idle
//!   eviction, turn records.
//! - [`api`]: the HTTP API served by `orchestratord`.
//! - [`work`]: the work manifest: intake, freezing, admission, checks, replies, and the
//!   engine that runs a frozen manifest through the outer plan and the inner loop.

pub mod api;
pub mod config;
pub mod egress;
pub mod metrics;
pub mod orchestrator;
pub mod pool;
pub mod records;
pub mod runtime;
pub mod sigv4;
pub mod spec;
pub mod work;

pub use config::OrchestratorConfig;
pub use orchestrator::{Intent, OrchError, Orchestrator, TurnResponse};
pub use spec::AgentSpec;
