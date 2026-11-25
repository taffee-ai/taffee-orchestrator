//! density: how many resident agents fit, and what each turn costs, on the shipped runtime.
//!
//! Ramps the number of resident (long-lived, conversation-holding) agents through the levels
//! in DENSITY_LEVELS. At each level every new agent runs a first turn against the model
//! endpoint, all agents stay resident, and process PSS is sampled. Every agent then runs a
//! second turn, which checks it kept its conversation and measures CPU per turn.
//!
//! Env: DENSITY_COMPONENT, DENSITY_ENDPOINT (default http://127.0.0.1:8000),
//!      DENSITY_LEVELS (default 1,10,50,100), DENSITY_MIN_AVAIL_MB (default 700),
//!      DENSITY_CONCURRENCY (turns in flight, default 16)

use anyhow::{bail, Result};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use taffee_orchestrator::metrics::{mem_available_mb, process_cpu_ms, MemorySample};
use taffee_orchestrator::{AgentSpec, Orchestrator, OrchestratorConfig};
use tokio::sync::Semaphore;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

async fn turn_all(orch: &Arc<Orchestrator>, ids: &[String], prompt: &str, concurrency: usize) -> Result<Vec<serde_json::Value>> {
    let sem = Arc::new(Semaphore::new(concurrency));
    let mut tasks = tokio::task::JoinSet::new();
    for id in ids {
        let (orch, id, sem, prompt) = (orch.clone(), id.clone(), sem.clone(), prompt.to_string());
        tasks.spawn(async move {
            let _p = sem.acquire_owned().await.unwrap();
            orch.turn(&id, &prompt).await
        });
    }
    let mut out = Vec::new();
    while let Some(r) = tasks.join_next().await {
        let r = r?.map_err(|e| anyhow::anyhow!("turn failed: {e}"))?;
        if r.outcome != "ok" {
            bail!("agent {} turn {} outcome {}: {:?}", r.agent_id, r.turn, r.outcome, r.error);
        }
        out.push(json!({ "message_count": r.message_count, "turn": r.turn }));
    }
    Ok(out)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let levels: Vec<usize> = env_or("DENSITY_LEVELS", "1,10,50,100").split(',').map(|s| s.trim().parse()).collect::<Result<_, _>>()?;
    let max = *levels.iter().max().unwrap_or(&1);
    let min_avail: u64 = env_or("DENSITY_MIN_AVAIL_MB", "700").parse()?;
    let concurrency: usize = env_or("DENSITY_CONCURRENCY", "16").parse()?;
    let endpoint = env_or("DENSITY_ENDPOINT", "http://127.0.0.1:8000");

    let mut cfg = OrchestratorConfig::default();
    cfg.component = PathBuf::from(env_or("DENSITY_COMPONENT", "../strand-agent/dist/strand-agent.wasm"));
    cfg.max_agents = max;
    cfg.default_spec.model.endpoint = Some(endpoint.clone());

    let before_load = MemorySample::current();
    let orch = Orchestrator::new(&cfg)?;
    let loaded = MemorySample::current();
    println!(
        "component {} bytes loaded in {:?}; process PSS {:.1} MB -> {:.1} MB",
        orch.runtime().component_bytes,
        orch.runtime().load_duration,
        before_load.pss_mb(),
        loaded.pss_mb()
    );

    // CPU per turn, one agent, sequential (no contention).
    let probe = orch.create_agent(AgentSpec::default(), None, None).await?;
    orch.turn(&probe.agent_id, "warm up").await?;
    let n_seq = 20;
    let (cpu0, t0) = (process_cpu_ms(), Instant::now());
    for i in 0..n_seq {
        let r = orch.turn(&probe.agent_id, &format!("sequential turn {i}")).await?;
        if r.outcome != "ok" {
            bail!("sequential turn failed: {:?}", r.error);
        }
    }
    let seq_cpu_per_turn = (process_cpu_ms() - cpu0) / n_seq as f64;
    let seq_wall_per_turn = t0.elapsed().as_secs_f64() * 1000.0 / n_seq as f64;
    orch.delete_agent(&probe.agent_id).await?;
    println!("sequential: {seq_cpu_per_turn:.1} CPU-ms/turn, {seq_wall_per_turn:.1} ms wall/turn (history grows each turn)");

    let mut ids: Vec<String> = Vec::new();
    let mut rows = Vec::new();
    let mut base: Option<(usize, MemorySample)> = None;
    let mut stopped = None;
    println!("\n{:>7} {:>11} {:>11} {:>13} {:>14} {:>12} {:>10}", "agents", "PSS MB", "RSS MB", "marginal MB", "turn1 wall ms", "CPU-ms/turn", "avail MB");
    for &n in &levels {
        if let Some(avail) = mem_available_mb() {
            if avail < min_avail {
                stopped = Some(format!("MemAvailable {avail} MB < guard {min_avail} MB before level {n}"));
                break;
            }
        }
        let mut new_ids = Vec::new();
        while ids.len() + new_ids.len() < n {
            new_ids.push(orch.create_agent(AgentSpec::default(), None, None).await?.agent_id);
        }
        let t = Instant::now();
        let cpu_a = process_cpu_ms();
        turn_all(&orch, &new_ids, "Hello from a resident agent", concurrency).await?;
        let cpu_first = process_cpu_ms() - cpu_a;
        let turn1_wall = if new_ids.is_empty() { 0.0 } else { t.elapsed().as_secs_f64() * 1000.0 };
        ids.extend(new_ids.iter().cloned());

        let sample = MemorySample::current();
        let marginal = match base {
            Some((bn, b)) if n > bn => (sample.pss_mb() - b.pss_mb()) / (n - bn) as f64,
            _ => f64::NAN,
        };
        if base.is_none() {
            base = Some((n, sample));
        }

        // Second turn for every resident agent: proves residency kept history and measures
        // CPU per turn under concurrency.
        let cpu_b = process_cpu_ms();
        let second = turn_all(&orch, &ids, "Second turn", concurrency).await?;
        let cpu_per_turn = (process_cpu_ms() - cpu_b) / ids.len().max(1) as f64;
        let history_ok = second.iter().all(|v| v["message_count"].as_u64().unwrap_or(0) >= 4);
        if !history_ok {
            bail!("an agent lost its conversation between turns at level {n}");
        }
        let avail = mem_available_mb().unwrap_or(0);
        println!(
            "{:>7} {:>11.1} {:>11.1} {:>13} {:>14.0} {:>12.1} {:>10}",
            n,
            sample.pss_mb(),
            sample.rss_mb(),
            if marginal.is_nan() { "-".to_string() } else { format!("{marginal:.2}") },
            turn1_wall,
            cpu_per_turn,
            avail
        );
        rows.push(json!({
            "agents": n,
            "pss_mb": sample.pss_mb(),
            "rss_mb": sample.rss_mb(),
            "private_dirty_mb": sample.private_dirty_mb(),
            "marginal_pss_mb_per_agent": if marginal.is_nan() { serde_json::Value::Null } else { json!(marginal) },
            "first_turn_cpu_ms_total": cpu_first,
            "second_turn_cpu_ms_per_agent": cpu_per_turn,
            "mem_available_mb": avail,
        }));
    }
    if let Some(s) = &stopped {
        println!("stopped early: {s}");
    }
    let summary = json!({
        "component_bytes": orch.runtime().component_bytes,
        "load_ms": orch.runtime().load_duration.as_secs_f64() * 1000.0,
        "pss_before_load_mb": before_load.pss_mb(),
        "pss_after_load_mb": loaded.pss_mb(),
        "sequential_cpu_ms_per_turn": seq_cpu_per_turn,
        "sequential_wall_ms_per_turn": seq_wall_per_turn,
        "levels": rows,
        "stopped_early": stopped,
    });
    println!("\nDENSITY_RESULT {summary}");
    Ok(())
}
