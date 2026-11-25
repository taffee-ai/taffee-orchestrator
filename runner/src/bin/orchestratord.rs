//! orchestratord: the taffee-orchestrator HTTP service.

use anyhow::Result;
use taffee_orchestrator::work::engine::Engine;
use taffee_orchestrator::{api, Orchestrator, OrchestratorConfig};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let cfg = OrchestratorConfig::from_env()?;
    tracing::info!(
        component = %cfg.component.display(),
        max_agents = cfg.max_agents,
        memory_limit_mb = cfg.memory_limit_bytes / (1024 * 1024),
        turn_timeout_secs = cfg.turn_timeout.as_secs(),
        credentials = ?cfg.egress.credential_names(),
        api_auth = !cfg.api_tokens.is_empty(),
        "starting orchestratord"
    );
    if cfg.api_tokens.is_empty() {
        tracing::warn!("no API tokens configured: the API is open to anyone who can reach {}", cfg.listen);
    }

    let orch = Orchestrator::new(&cfg)?;
    let rt = orch.runtime();
    tracing::info!(load_ms = rt.load_duration.as_millis() as u64, bytes = rt.component_bytes, "component ready");
    let _reaper = orch.spawn_reaper();

    let work = match &cfg.work {
        Some(w) => {
            let e = Engine::new(orch.clone(), w, &cfg.default_spec)?;
            tracing::info!(
                kinds = e.registry.entries().count(),
                normalizer = e.roles.normalizer.is_some(),
                planner = e.roles.planner.is_some(),
                "work manifest API enabled"
            );
            Some(e)
        }
        None => None,
    };
    let app = api::router_with_work(orch, work, &cfg.api_tokens);
    let listener = tokio::net::TcpListener::bind(cfg.listen).await?;
    tracing::info!("listening on http://{}", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
