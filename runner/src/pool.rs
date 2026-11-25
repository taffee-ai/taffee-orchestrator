//! Pooled Wasmtime engine and admission check for agent components.
//!
//! Every agent is one component instance from one shared, pre-compiled component. The
//! pooling allocator reserves fixed slots up front and maps each instance's linear memory
//! copy-on-write from the component's Wizer snapshot, so the shared engine and SDK pages are
//! paid for once per host.
//!
//! Measured on strand-agent.wasm (componentize-js 0.23): 4 core modules, ~33 core instances,
//! 2 memories, 2 tables, initial linear memory 513 pages (32 MB). Slot sizes below are the
//! larger of these defaults and what `inspect_and_validate_component` finds.

use anyhow::{bail, Result};
use wasmparser::{Chunk, Parser, Payload, TypeRef};
use wasmtime::{
    Config, Engine, InstanceAllocationStrategy, PoolingAllocationConfig,
    StoreLimits, StoreLimitsBuilder,
};

#[derive(Debug, Clone)]
pub struct PoolConfigOptions {
    pub max_agents: usize,
    pub max_memory_bytes: usize,
    pub core_instances_per_component: u32,
    pub memories_per_component: u32,
    pub tables_per_component: u32,
}

impl Default for PoolConfigOptions {
    fn default() -> Self {
        Self {
            max_agents: 50,
            max_memory_bytes: 64 * 1024 * 1024,
            core_instances_per_component: 40,
            memories_per_component: 2,
            tables_per_component: 4,
        }
    }
}

impl PoolConfigOptions {
    /// Grow the per-component slot sizes to fit a component's measured needs.
    pub fn fit(mut self, m: &ComponentMetrics) -> Self {
        self.core_instances_per_component = self.core_instances_per_component.max(m.core_instances as u32 + 2);
        self.memories_per_component = self.memories_per_component.max(m.memories as u32);
        self.tables_per_component = self.tables_per_component.max(m.tables as u32);
        self
    }
}

/// Engine settings shared by every agent: component model, async, epoch interruption (turn
/// deadlines and fair scheduling) and the pooling allocator.
pub fn create_pooled_engine(opts: &PoolConfigOptions) -> Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    #[allow(deprecated)]
    config.async_support(true);
    config.epoch_interruption(true);
    config.memory_reservation(opts.max_memory_bytes as u64);

    let n = opts.max_agents as u32;
    let mut pool = PoolingAllocationConfig::default();
    pool.total_component_instances(n);
    pool.max_core_instances_per_component(opts.core_instances_per_component);
    pool.total_core_instances(n * opts.core_instances_per_component);
    pool.max_memories_per_component(opts.memories_per_component);
    pool.total_memories(n * opts.memories_per_component);
    pool.max_tables_per_component(opts.tables_per_component);
    pool.total_tables(n * opts.tables_per_component);
    pool.total_stacks(n);
    pool.max_memory_size(opts.max_memory_bytes);
    // Return a finished agent's dirty pages promptly.
    pool.decommit_batch_size(1);
    config.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));

    Ok(Engine::new(&config)?)
}

/// Per-store ceilings. Memory is the hard per-agent limit.
pub fn create_store_limits(opts: &PoolConfigOptions) -> StoreLimits {
    StoreLimitsBuilder::new()
        .memory_size(opts.max_memory_bytes)
        .memories(opts.memories_per_component as usize)
        .tables(opts.tables_per_component as usize)
        .instances(opts.core_instances_per_component as usize)
        .build()
}

#[derive(Debug, Default, Clone)]
pub struct ComponentMetrics {
    pub core_modules: usize,
    pub core_instances: usize,
    pub memories: usize,
    pub tables: usize,
    pub max_initial_memory_pages: u64,
}

/// Inspects and validates a Wasm component using `wasmparser`.
/// Rejects components whose initial linear memory exceeds 48 MB (768 pages).
/// Fails closed on any parse error or truncated payload.
pub fn inspect_and_validate_component(wasm_bytes: &[u8]) -> Result<ComponentMetrics> {
    const MAX_INITIAL_PAGES: u64 = 768; // 48 MB (768 * 64KB)

    if wasm_bytes.len() < 8 {
        bail!("Admission check failed: Wasm binary is too small (< 8 bytes)");
    }

    let mut metrics = ComponentMetrics::default();
    let mut stack = Vec::new();
    let mut cur = Parser::new(0);
    let mut offset = 0;

    loop {
        let chunk = cur
            .parse(&wasm_bytes[offset..], true)
            .map_err(|e| anyhow::anyhow!("Admission check failed: Wasm parsing error at offset {}: {}", offset, e))?;

        match chunk {
            Chunk::Parsed { consumed, payload } => {
                offset += consumed;
                match payload {
                    Payload::ModuleSection { parser, .. } => {
                        metrics.core_modules += 1;
                        stack.push(cur);
                        cur = parser;
                    }
                    Payload::ComponentSection { parser, .. } => {
                        stack.push(cur);
                        cur = parser;
                    }
                    Payload::InstanceSection(reader) => {
                        metrics.core_instances += reader.count() as usize;
                    }
                    Payload::MemorySection(reader) => {
                        for mem in reader {
                            let mem = mem.map_err(|e| {
                                anyhow::anyhow!("Admission check failed: malformed memory section: {}", e)
                            })?;
                            metrics.memories += 1;
                            let initial = mem.initial;
                            if initial > metrics.max_initial_memory_pages {
                                metrics.max_initial_memory_pages = initial;
                            }
                            if initial > MAX_INITIAL_PAGES {
                                bail!(
                                    "Component linear memory exceeds admission ceiling: initial {} pages ({} MB) > allowed limit {} pages (48 MB)",
                                    initial,
                                    (initial * 64) / 1024,
                                    MAX_INITIAL_PAGES
                                );
                            }
                        }
                    }
                    Payload::ImportSection(reader) => {
                        for imps in reader {
                            let imps = imps.map_err(|e| {
                                anyhow::anyhow!("Admission check failed: malformed import section: {}", e)
                            })?;
                            let mut check_mem = |initial: u64| -> Result<()> {
                                metrics.memories += 1;
                                if initial > metrics.max_initial_memory_pages {
                                    metrics.max_initial_memory_pages = initial;
                                }
                                if initial > MAX_INITIAL_PAGES {
                                    bail!(
                                        "Component imported memory exceeds admission ceiling: initial {} pages ({} MB) > allowed limit {} pages (48 MB)",
                                        initial,
                                        (initial * 64) / 1024,
                                        MAX_INITIAL_PAGES
                                    );
                                }
                                Ok(())
                            };

                            match imps {
                                wasmparser::Imports::Single(_, imp) => {
                                    if let TypeRef::Memory(mem) = imp.ty {
                                        check_mem(mem.initial)?;
                                    }
                                }
                                wasmparser::Imports::Compact1 { items, .. } => {
                                    for item in items {
                                        let item = item.map_err(|e| anyhow::anyhow!("malformed compact import: {}", e))?;
                                        if let TypeRef::Memory(mem) = item.ty {
                                            check_mem(mem.initial)?;
                                        }
                                    }
                                }
                                wasmparser::Imports::Compact2 { ty, .. } => {
                                    if let TypeRef::Memory(mem) = ty {
                                        check_mem(mem.initial)?;
                                    }
                                }
                            }
                        }
                    }
                    Payload::TableSection(reader) => {
                        metrics.tables += reader.count() as usize;
                    }
                    Payload::End(_) => {
                        if let Some(parent) = stack.pop() {
                            cur = parent;
                        } else {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            Chunk::NeedMoreData(_) => {
                bail!("Admission check failed: unexpected truncated data in Wasm binary");
            }
        }
    }

    if metrics.memories == 0 {
        bail!("Admission check failed: no linear memory found in component");
    }

    Ok(metrics)
}

/// Validates that a component's initial linear memory requirement does not exceed the 48 MB safety threshold.
pub fn validate_component_memory(wasm_bytes: &[u8]) -> Result<()> {
    inspect_and_validate_component(wasm_bytes).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_create_pooled_engine() {
        let opts = PoolConfigOptions { max_agents: 10, ..Default::default() };
        let engine = create_pooled_engine(&opts);
        assert!(engine.is_ok(), "Engine creation with Pooling Allocation should succeed");
    }

    #[test]
    fn test_validate_component_memory_real_artifact() {
        let wasm_path = PathBuf::from("../strand-agent/dist/strand-agent.wasm");
        if !wasm_path.exists() {
            println!("Skipping real artifact test: strand-agent.wasm not found");
            return;
        }

        let bytes = std::fs::read(&wasm_path).expect("failed to read strand-agent.wasm");
        let metrics = inspect_and_validate_component(&bytes).expect("validation should succeed for real artifact");

        println!(
            "Empirical component metrics: modules={}, instances={}, memories={}, tables={}, max_initial_pages={} ({} MB)",
            metrics.core_modules,
            metrics.core_instances,
            metrics.memories,
            metrics.tables,
            metrics.max_initial_memory_pages,
            (metrics.max_initial_memory_pages * 64) / 1024
        );

        assert!(metrics.memories >= 1, "Must contain at least 1 linear memory");
        assert!(
            metrics.max_initial_memory_pages <= 768,
            "Initial memory must be <= 48 MB (768 pages), got {}",
            metrics.max_initial_memory_pages
        );
    }

    #[test]
    fn test_validate_component_memory_oversized_rejection() {
        // Construct a minimal Wasm module with 1 memory having initial=800 pages (50 MB)
        // Header: \0asm\x01\0\0\0
        // Section 5 (Memory): id=5, size=4, count=1, flags=0 (no max), initial=800 (0xa0, 0x06 in LEB128)
        let mut wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        wasm.push(5); // Memory section id
        wasm.push(4); // section length
        wasm.push(1); // count of memories = 1
        wasm.push(0); // flags: limits without maximum
        wasm.push(0xa0); // 800 in LEB128 (800 & 0x7f | 0x80 = 0x20 | 0x80 = 0xa0)
        wasm.push(0x06); // 800 >> 7 = 6

        let res = validate_component_memory(&wasm);
        assert!(res.is_err(), "Expected oversized memory to be rejected");
        let err = res.unwrap_err().to_string();
        assert!(
            err.contains("exceeds admission ceiling") && err.contains("800 pages"),
            "Error message should mention exceeding admission ceiling: {}",
            err
        );
    }

    #[test]
    fn test_validate_component_memory_malformed_fails_closed() {
        // Truncated Wasm header
        let malformed = vec![0x00, 0x61, 0x73];
        let res = validate_component_memory(&malformed);
        assert!(res.is_err(), "Malformed Wasm must fail closed");

        // Valid header but corrupt truncated section
        let corrupt_sec = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x05, 0x10, 0x01];
        let res = validate_component_memory(&corrupt_sec);
        assert!(res.is_err(), "Corrupt truncated section must fail closed");
    }
}
