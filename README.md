# taffee-orchestrator

The Taffee orchestrator runs many long-lived, isolated AI agents on one host. Each agent is a WASI 0.2 component in Wasmtime 49. The reference agent is an unmodified [Strands Agents](https://github.com/strands-agents/harness-sdk) TypeScript agent, compiled to WebAssembly. Agents never hold credentials: the host attaches them to outgoing calls, per destination.

Every request runs through [two loops](docs/two-loops.md): the orchestrator's outer loop around the request, and each agent's inner loop inside it.

Callers can drive agents directly, or send **work**: a request written as a sentence or as a structured [work manifest](docs/spec/work-manifest/README.md). The orchestrator turns it into a checked contract, plans it, runs it on agents, checks the answer, and replies with a status the caller can rely on.

## Status

Early. Two milestones are built and tested: the orchestrator service (M1) and work manifests (M5). See [docs/roadmap.md](docs/roadmap.md).

- Models: Bedrock (Converse) or any OpenAI-style chat endpoint (OpenRouter, LiteLLM, ...). CI runs against a mock model; `scripts/live-work.sh` runs one request against a real one.
- Inputs for work come from fixture files or checked-out directories; fetching from git is not built.
- Not production-ready: receipts live in memory, and there is no governance layer yet.
- Density: each resident agent adds about 4.2–4.6 MB of memory; 100 agents fit in under 500 MB. See the [density benchmark](docs/benchmarks/density.md), which you can rerun with one command to check the numbers.

## What's here

| Path | What it is |
|---|---|
| `runner/` | The Rust host (crate `taffee-orchestrator`): the shared Wasmtime engine and per-agent instances, egress with host-held credentials (SigV4 and Bearer), the agent registry, the work engine (`src/work/`) and the HTTP API. Binaries: `orchestratord` (service), `taffee-run` (one-shot CLI), `density` (memory bench) |
| `strand-agent/` | The reference guest: a Strands agent behind the `taffee:agent/worker@0.1.0` contract, built with esbuild and ComponentizeJS |
| `mock-bedrock/` | A mock model (Bedrock Converse) and a minimal MCP server, used by the tests |
| `docker/` | The builder image and the runtime image |
| `deploy/` | A compose service, an example config and a catalog entry, to be merged into the taffee-core stack |
| `docs/` | The roadmap, the [API](docs/api.md), the [guest contract](docs/guest-contract.md), [testing](docs/testing.md), the work manifest [spec](docs/spec/work-manifest/README.md), and test evidence |

## Build and test

Everything builds and tests inside Docker, on a separate test machine. Nothing builds on your workstation.

Clone with submodules: the Strands SDK source is pinned at a commit under `upstream/strands-harness-sdk`.

```
git clone --recurse-submodules https://github.com/taffee-ai/taffee-orchestrator.git
```

1. Copy `scripts/boxes.example.sh` to `scripts/boxes.local.sh` (git-ignored) and describe your test machines: an ssh destination, an optional key, and a neutral label for evidence.
2. Run the full suite:

   ```
   scripts/remote.sh                  # the default machine
   TAFFEE_BOX=node2 scripts/remote.sh # another machine from boxes.local.sh
   ```

   This syncs the source, builds the `taffee-orch-builder:dev` image if needed, and runs `scripts/ci.sh` in it: the guest build, the mock tests, the host unit and integration tests, the work-manifest conformance vectors, the work-engine tests, and release binaries. Logs come back to `docs/evidence/<date>-<label>/`.
3. On the test machine, `scripts/smoke-image.sh` builds the runtime image `taffee-orchestrator:dev` and tests it in containers on a private network.

`RUN_DENSITY=1 scripts/remote.sh` also runs the density bench. To run it on your own machine (Docker only) and check the result against the published figures, use `scripts/bench-density.sh`; see [docs/benchmarks/density.md](docs/benchmarks/density.md).

## Calling it

`orchestratord` listens on `127.0.0.1:8780` by default. With API tokens configured, every `/v1` request needs `Authorization: Bearer <token>`.

```
POST /v1/work
{ "manifest": "0.1",
  "want": "If I rename PaymentService to PaymentGateway in payments-service, what breaks?",
  "dry_run": true }
```

A dry run returns the frozen manifest and what was assumed. Send it back without `dry_run` to run it. The full API is in [docs/api.md](docs/api.md).

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
