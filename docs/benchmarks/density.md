# Density benchmark

How much memory and CPU each long-lived agent costs on one host, measured on the shipped runtime, with a script you can rerun on your own machine to check our numbers.

**Result (2026-11-22, two machines):** each additional resident agent adds **4.2–4.6 MB** of process memory (PSS). 100 resident agents fit in **472–498 MB**. A turn costs about **50–60 ms of CPU**.

## Results

Each agent is the reference Strands agent ([`strand-agent/`](../../strand-agent/)), compiled to a WASI 0.2 component and running in its own Wasmtime instance. The agents stay resident and keep their conversation. Memory is sampled after each agent's first turn. Every agent then takes a second turn, which checks that it kept its history.

| Resident agents | Process PSS, node1 | Process PSS, node2 | Added per agent, node1 | Added per agent, node2 | CPU per turn, node1 | CPU per turn, node2 |
|---|---|---|---|---|---|---|
| 1 | 39.7 MB | 39.7 MB | – | – | 40 ms | 30 ms |
| 10 | 77.5 MB | 81.0 MB | 4.20 MB | 4.58 MB | 49 ms | 48 ms |
| 50 | 247.4 MB | 260.5 MB | 4.24 MB | 4.51 MB | 49 ms | 53 ms |
| 100 | 472.3 MB | 498.4 MB | 4.37 MB | 4.63 MB | 51 ms | 61 ms |

| | node1 | node2 |
|---|---|---|
| CPU | Intel Core i5-2520M, 2 cores, 4 threads | Intel Core i3-1000NG4, 2 cores, 4 threads |
| Memory | 5.7 GB | 16 GB; Docker VM with 4 vCPUs and 8 GB |
| Host | Ubuntu 20.04, Docker Engine, kernel 5.15 | macOS 15, Docker Desktop, kernel 7.0 (linuxkit) |
| Other load | other services running, mostly idle | about a dozen other containers running |
| Component load from the compiled cache | 285 ms | 109 ms |
| One agent, sequential turns | 55.5 CPU-ms per turn | 45.5 CPU-ms per turn |
| RSS at 100 agents | 1,204 MB | 1,207 MB |

Raw output: [`node1`](../evidence/2026-11-22-density-taffee-localtest-node1/) and [`node2`](../evidence/2026-11-22-density-taffee-localtest-node2/). The figures in [`density-reference.json`](density-reference.json) are what the check compares against.

## What the numbers mean

- **PSS is the figure to plan with.** RSS counts every page an agent maps, so the code and snapshot pages all agents share are counted once per agent. PSS divides each shared page by the number of processes mapping it. "Added per agent" is the growth in process PSS from the first level to this one, divided by the number of agents added.
- **Small conversations.** Each agent holds 4 messages after its two turns. Memory grows with conversation history, so agents holding long conversations cost more than these figures.
- **A mock model.** The model is a local mock that answers immediately, so the bench measures the agents and the host, not the model, the network or tools. In production an agent spends most of its life waiting for the model, holding memory but no CPU.
- **CPU depends on the machine.** The agent loop runs in a JavaScript interpreter inside Wasm, so CPU, not memory, limits throughput. At 50–60 CPU-ms per turn, four hardware threads give roughly 70 turns per second at saturation.
- **The component compiles once.** The first load of a new component compiles it: 16–21 s on these machines, and the compiler leaves about 420 MB in that process. The service caches the compiled result on a volume (see [`deploy/`](../../deploy/)), so later starts load it in under a second. The bench does the same: a warm-up run compiles and caches it, and the measured run starts a fresh process that loads the cache.

To size a host for short conversations: about 40 MB, plus 4.6 MB per resident agent, plus headroom for longer histories. Each agent is capped at 64 MB of Wasm memory ([guest contract](../guest-contract.md)); that is a ceiling, not a typical size.

## Rerun it

You need Docker and about 7 GB of free disk: the toolchain image is 3.3 GB and the build caches about 3.5 GB. Nothing is installed on your machine: the script builds a toolchain image and does everything inside containers.

```bash
git clone --recurse-submodules https://github.com/taffee-ai/taffee-orchestrator.git
cd taffee-orchestrator
scripts/bench-density.sh
```

The first run takes 15–30 minutes: it builds the toolchain image, the agent component, the bench binary, and compiles the component once. Later runs reuse the caches.

The script:

1. Installs the guest's npm dependencies from `strand-agent/package-lock.json` and stops if anything installed differs from it.
2. Builds the agent component and the `density` binary (Rust, with `--locked` against `runner/Cargo.lock`).
3. Starts the mock model, runs the warm-up, then the bench at 1, 10, 50 and 100 agents.
4. Checks your results against the published ones and prints `RESULT: PASS` or `RESULT: FAIL`.

Results go to `docs/evidence/<date>-density-local/`: `density.log` (the table), `density.json`, `density-check.log`, `environment.txt`, `component.sha256` and the build logs.

Options:

| Variable | Default | Effect |
|---|---|---|
| `BENCH_LABEL` | `local` | Name of the results folder |
| `DENSITY_LEVELS` | `1,10,50,100` | Agent counts to measure. The check needs the four published levels; add more to go further, for example `1,10,50,100,200` |

The bench stops early if the machine's free memory drops below 700 MB, and reports that as a failure. 100 agents need about 1.3 GB of free memory for the bench process.

On an arm64 machine (for example Apple silicon), Docker emulates x86_64. Memory figures stay comparable; CPU figures do not.

## How the check works

[`scripts/bench-density-check.py`](../../scripts/bench-density-check.py) passes your run when:

- every level ran and every agent kept its conversation between turns;
- the component loaded from the compiled cache, so the measured process does not include the compiler;
- at each published level, process PSS and the memory added per agent are inside the range the published runs span, widened by 20%.

Our two machines differ by up to 8% on these figures, with different kernels and Docker setups; the 20% allows for machines further apart than ours. CPU and timings are printed beside ours for comparison but not checked.

The component's hash is printed but not checked: the guest build is not byte-for-byte reproducible. Three builds of the same source on 2026-11-22 gave three different hashes (sizes within 35 bytes of each other). What the script does pin and check are the inputs: the npm dependencies against the lock file before the build, and the Rust dependencies with `--locked`.

If your run fails, please open an issue with your results folder.

## How the published numbers were made

With `scripts/bench-density.sh` itself, on two x86_64 machines, from this repository's contents at the commit that added this page. A run checks against the published runs, so the two runs that define the reference have no check log of their own.
