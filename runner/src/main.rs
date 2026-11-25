//! taffee-run: run one agent turn from the command line (no server).
//!
//!   taffee-run [component.wasm] [prompt]
//!   BEDROCK_ENDPOINT=http://127.0.0.1:8000 taffee-run

use anyhow::{bail, Result};
use std::path::PathBuf;
use taffee_orchestrator::{AgentSpec, Orchestrator, OrchestratorConfig};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let mut args = std::env::args().skip(1);
    let mut cfg = OrchestratorConfig::default();
    cfg.component = args.next().map(PathBuf::from).unwrap_or(cfg.component);
    cfg.max_agents = 1;
    let prompt = args.next().unwrap_or_else(|| "Calculate 21 * 2 using calculator tool and return the result".into());
    let mut spec = AgentSpec::default();
    spec.model.endpoint = Some(std::env::var("BEDROCK_ENDPOINT").unwrap_or_else(|_| "http://127.0.0.1:8000".into()));

    let orch = Orchestrator::new(&cfg)?;
    println!("component ready in {:?}", orch.runtime().load_duration);
    match orch.run_once(spec, &prompt, None, None).await {
        Ok(r) => {
            println!("{}", serde_json::to_string_pretty(&r)?);
            if r.outcome != "ok" {
                bail!("turn outcome: {}", r.outcome);
            }
            Ok(())
        }
        Err(e) => bail!("{e}"),
    }
}
