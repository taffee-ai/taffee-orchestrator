# taffee-orchestrator roadmap

**Owner:** orchestrator pillar. **Written:** 2026-11-15. **Scope assumption:** transport security (WireGuard tunnels, per-agent network identity) is provided later by a separate component. This roadmap covers the orchestrator with Wasm isolation and no network security of its own.

Status words follow taffee-core's rule: a row is **Done** only when code exists and a test on the test box exercised it. The evidence for Done rows is in `docs/evidence/`.

## 1. What the product promises

The TaffeeAI.com site and the taffee-core docs promise the following about the orchestrator.

| # | Promise | Where it is made |
|---|---|---|
| W1 | Many isolated agents per host, a few MB each, in Wasmtime 49 with Wizer snapshots and copy-on-write pooling | site, orchestrator page |
| W2 | A per-agent memory ceiling (64 MB StoreLimits) | site |
| W3 | Zero API keys in the sandbox: the host attaches SigV4 or Bearer credentials | site, both pages |
| W4 | Every outbound call is mediated by the host (destination check) | site |
| W5 | Open to any agent loop that implements the guest contract, not only Strands | orchestrator page |
| W6 | Agents reach taffee-context MCP and qualified tools through the host | site diagram |
| W7 | `execute_agent(session_id, prompt, history)`: a caller-facing execution API | orchestrator page sequence |
| W8 | "Packaging next": a service that runs in the Taffee stack | site status chip |
| W9 | Bounded execution: iteration, time and resource limits (core R01, R02, R08) | core roadmap |
| W10 | Every call leaves a record without content (core runtime view) | core runtime view |
| W11 | Wired into AuthZEN decisions, single-use credentials and delegation (Stage 4) | site Stage 4, core |
| W12 | Intent records, planner, verification and promotion | core |
| W13 | Outcome verification and OTel traces (Stage 5, O07, I05) | site Stage 5, core |

## 2. What was missing on 2026-11-15

Measured against W1–W13, the starting code (copied from an earlier prototype) had:

- **No service.** Only a one-shot CLI and benchmark binaries. W7 and W8 were not met.
- **No host credential injection.** The guest signed with fake keys; nothing on the host could sign. The site's claim of host-side signing (W3) was not true of the code.
- **No CPU preemption.** A guest in a tight loop blocked a host thread indefinitely (W9).
- **No pooling on the real path.** Each worker built its own engine and recompiled or deserialized the component (W1 was only true inside the bench binaries).
- **No MCP or tool wiring** (W6), no spec (model, tools, limits), and no turn limits.
- **Bugs:** the egress-denial flag never reset, the scheme was never checked, a failed turn left a dangling user message (real Bedrock rejects the next turn), guest stdio went to the host, and `ramp.rs` no longer matched the guest's export.
- **Stale claims:** the site's 3.58 MB per agent came from the older one-shot guest, the site still describes `wasi:http/incoming-handler`, and the docs still described WireGuard phases.

## 3. Milestones

### M1: Orchestrator service (this change)

The smallest thing that makes W1–W10 true end to end.

| ID | Item | Core roadmap | Status |
|---|---|---|---|
| M1.1 | `orchestratord` HTTP service: agents (create, list, get, delete), turns, one-shot runs, stats, health. See `api.md` | R03, I03 | Done |
| M1.2 | One shared pooled engine and pre-linked component for every agent; compiled-code cache keyed by engine and content hash | W1 | Done |
| M1.3 | Admission control: capacity from the pool, `429` when full, slot released on delete | R10 | Done |
| M1.4 | Turn deadlines: epoch interruption for computing guests, a wall timeout for waiting guests, cooperative yield every 10 ms so one spinning agent cannot starve others | R01 | Done |
| M1.5 | Typed outcomes: `ok`, `turn_failed` (agent kept, history rolled back), `timeout` and `agent_lost` (agent removed, caller told), `busy`, `at_capacity`, `invalid_spec` | R02 | Done |
| M1.6 | Agent spec: model, system prompt, built-in tools, MCP servers, SDK limits (turns, tokens), tool-output cap, seed history | R01, R04, R08, W7 | Done |
| M1.7 | Host credential injection: AWS SigV4 re-signing and Bearer tokens, per destination, secrets read from env or file and never sent to a guest | W3, G07 | Done |
| M1.8 | Per-agent egress: an agent may reach only the endpoints its spec names plus a deployment allow list. Denials are answered by the host and recorded | W4, G05 | Done |
| M1.9 | MCP tools in the guest: Streamable HTTP client (JSON or SSE), tools listed at agent start and exposed to the model | W6, P09 | Done |
| M1.10 | Turn records with no content: destination, status, timing, credential name, usage, requester, intent id; per-agent history and JSONL file | W10, G10 | Done |
| M1.11 | Guest contract `taffee:agent/worker@0.1.0`, proven with a non-JS (hand-written WAT) guest | W5 | Done |
| M1.12 | Idle eviction and API bearer tokens (requester recorded) | G01 | Done |
| M1.13 | Builder and runtime images, CI script, compose service and catalog entry for taffee-core `infra/` | W8, I01, I03 | Done (compose not yet merged into taffee-core) |
| M1.14 | Density bench on the shipped runtime (resident agents, marginal PSS, CPU per turn) | W1 | Done; see §3a. Current figures and a rerunnable benchmark: [benchmarks/density.md](benchmarks/density.md) |

### 3a. M1 evidence (test box, 2026-11-16 UTC, `docs/evidence/2026-11-16-taffee-localtest-node1/`)

- **Tests:** 22 host unit tests, 19 integration tests (real `strand-agent.wasm`, mock model and MCP) and 7 mock tests pass, all inside the builder image. The runtime image (156 MB) passes the smoke test: health check, 401 without a token, create, two turns with history, stats, delete.
- **Density, persistent agents:** each agent holds its conversation (4 messages after 2 turns). Ubuntu box: i5-2520M with 4 threads, 5.7 GB RAM, other stacks running.

| Resident agents | Process PSS | Marginal PSS per agent | CPU per turn |
|---|---|---|---|
| 1 | 39 MB | — | 40 ms |
| 10 | 76 MB | 4.07 MB | 49 ms |
| 50 | 241 MB | 4.11 MB | 49 ms |
| 100 | 460 MB | 4.25 MB | 51 ms |
| 200 | 898 MB | 4.31 MB | 52 ms |
| 300 | 1,372 MB | 4.46 MB | 54 ms |

  The marginal cost grows slowly with conversation size; it is about 4.1–4.5 MB per resident agent, under the 5 MB target. RSS is higher than PSS because every agent maps the same snapshot pages; PSS divides shared pages by their mappers and is the figure to plan with. A single agent's turns take about 58 CPU-ms sequentially: the interpreter, not memory, bounds throughput. Four hardware threads give about 70 turns per second at saturation on this box.
- **Startup:** the component loads in 263 ms from the compiled cache. Without a cache (a new container), it takes 14 s to compile, so the compose service keeps the cache on a volume.

### M2: Governed execution (next)

Turns the orchestrator into the enforcement point of the runtime view: nothing runs without a decision.

| ID | Item | Depends on |
|---|---|---|
| M2.1 | `Authorizer` trait in the orchestrator with an allow-all dev implementation, called before each agent create and each tool or model call (AuthZEN shape: subject, action, resource, context) | contract tests (governance) |
| M2.2 | Replace static credential rules with the govsec broker: one short-lived credential per action, requested per call | govsec broker |
| M2.3 | Capability resolve: tools named by capability id are resolved through the capabilities registry (`resolve()` with digest check, label floor and revocation), and the call record names digest, label and revocation sequence | registry HTTP service (S-008) |
| M2.4 | Delegation envelope passed into sub-agents so a child can only narrow its parent | governance |
| M2.5 | Settle who writes the call record (open point 3 of the runtime view). Proposal: the orchestrator writes it, since it makes the call | umbrella decision |

### M3: Durable sessions

| ID | Item |
|---|---|
| M3.1 | Evict idle agents to disk and restore on the next turn: save the conversation via the SDK, release the slot, rehydrate with `messages`. The seed-history path built in M1.6 is the restore half |
| M3.2 | Checkpoint before side-effecting tool calls so a lost agent can resume without repeating them (R11) |
| M3.3 | Agents survive an orchestrator restart |

### M4: Density 2 (CPU, not RAM, is the limit)

| ID | Item |
|---|---|
| M4.1 | Tools as separate Rust components, one short-lived instance per call, aiming at under 1 MB of deterministic memory per long-lived agent. Moves deterministic work out of the JS interpreter |
| M4.2 | A Rust agent loop guest on the same contract (Path B). The WAT guest in the tests shows the contract does not depend on JS |
| M4.3 | ConverseStream (streaming responses) through the guest |
| M4.4 | Repeat the density bench at 1,000+ agents on a 16–64 GB host |

### M5: Intent and planning

Intent intake with entity authentication, planner, parts-and-gaps, verification and promotion, status events. M1 carries and records an intent record but does not plan from it.

The inbound interface for M5 is the **work manifest**: [`spec/work-manifest/`](spec/work-manifest/README.md) (draft 0.1). It defines what the orchestrator accepts from any caller and what it returns. The outer plan's input is the frozen manifest's work fields.

**Built (2026-11-16)**, per [`features/work-manifest/PLAN.md`](features/work-manifest/PLAN.md): `POST /v1/work` and `GET /v1/work/{receipt}`; intake, freezing and admission (W1); host-run checks and the reply with one worker (W2); the outer plan with narrowed worker manifests (W3); the sentence normalizer (W4). Evidence: `evidence/2026-11-16-taffee-localtest-node1/conformance.log` (32 of 32 vectors) and `work-engine.log` (6 of 6). Still open: inputs fetched from git (today a fixture or a checked-out directory per pinned ref), verifiers other than CEL, receipts that survive a restart, and running the roles against a real model.

### M6: Outcomes and operations

Outcome verification against target state (O07), OpenTelemetry traces across turns and calls (I05), fleet health.

### M7: Channels (requirement, 2026-11-18)

People ask through Slack, WhatsApp and other messaging apps, and agents can send messages there. A channel is a caller or a tool: bridges outside the orchestrator turn messages into work requests (receipts for long tasks; the readback and the approval become Confirm and Approve buttons), and agents send messages with host-held bot credentials under `task.act@1` until governance allows finer rules. Memory across messages needs M3.

### Outside this roadmap

WireGuard or any other transport security. The egress gate keeps a generic `via` route so a tunnel provider can plug in without changing the agent path.
