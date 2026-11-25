# Testing

All builds and tests run on the Ubuntu test box inside the `taffee-orch-builder:dev` image. From the Mac:

```bash
scripts/remote.sh                                   # everything below except density
RUN_DENSITY=1 DENSITY_LEVELS=1,10,50,100 scripts/remote.sh
```

The published density benchmark runs with `scripts/bench-density.sh` on any machine with Docker, and checks the result against [`benchmarks/density-reference.json`](benchmarks/density-reference.json). See [benchmarks/density.md](benchmarks/density.md).

Each run writes logs to `docs/evidence/<date>-<label>/` on the test machine and copies them back (the label is the machine's neutral name in `scripts/boxes.local.sh`, e.g. `taffee-localtest-node1`):

- `node-modules.log` (installed npm packages match `package-lock.json`), `guest-build.log`, `component.sha256`
- `mock-test.log`, `runner-unit.log`, `runner-integration.log`
- `conformance.log`, `work-engine.log`
- `density.log`, when the density bench runs

## What each test proves

| Test | Proves | Roadmap |
|---|---|---|
| **Host unit tests** (`cargo test --lib`) | | |
| `sigv4::aws_suite_get_vanilla` | the signer matches AWS's published `get-vanilla` vector (canonical request hash and signature) | M1.7 |
| `sigv4::*` (date, URI, query, redaction) | date formatting, double-encoded segments for model ids with `:`, sorted query, no secret in `Debug` | M1.7 |
| `egress::allow_rules_parse_and_match` | host, host:port, scheme and `*` rules; suffix attacks (`api.example.com.attacker.com`) do not match | M1.8 |
| `egress::denies_unlisted_destination_and_records_it` | the gate answers 403 itself, records the denial, and records reset per turn | M1.8, M1.10 |
| `egress::bearer_injection_replaces_guest_authorization` | a guest-supplied `Authorization` is replaced | M1.7 |
| `egress::sigv4_injection_resigns_with_host_credentials` | guest signature and token headers are removed, the host signs over the exact body bytes, and `content-length` matches | M1.7 |
| `spec::*` | defaults merge, validation (provider, endpoint scheme, unknown tool, zero limits, duplicate MCP names, unknown fields), authority normalization, digest ignores seed history | M1.6 |
| `config::*` | credential secrets come from env, a missing secret fails at startup, unknown and invalid config is rejected, tokens-file format | M1.7, M1.12 |
| `pool::*` | pooled engine builds; admission check accepts the real component and rejects oversized or malformed ones | M1.2 |
| **Integration tests** (real `strand-agent.wasm`, mocks on :8000 and :8001) | | |
| `http_request_tool_fetches_a_page_as_text` | the agent's `http_request` fetches a page through the host, HTML comes back as text with link targets | general tasks |
| `calculator_tool_roundtrip` | full Converse, tool-use and tool-result loop through the orchestrator; call records | M1.1, M1.10 |
| `agents_keep_history_and_are_isolated` | an agent keeps its conversation across turns; two agents do not share it | M1.1 |
| `seeded_history_and_system_prompt_reach_the_model` | `spec.messages` and `systemPrompt` arrive at the model | M1.6 |
| `failed_turn_rolls_back_history` | a model failure gives `turn_failed`, leaves no dangling user message, and the next turn works | M1.5 |
| `sdk_turn_limit_stops_a_runaway_tool_loop` | `limits.turns` caps model calls for a model that never stops asking for tools | M1.6 |
| `guest_sends_only_placeholder_credentials` | without a host rule, the guest's signature uses the placeholder key, so the guest holds no real key | M1.7 |
| `host_resigns_model_calls_with_sigv4` | with a rule, every model call arrives signed with the host's key and never the placeholder | M1.7 |
| `mcp_tools_work_with_host_injected_token` | the guest's MCP client (SSE responses, session id) lists and calls a tool, and every MCP request carries the host's token | M1.9, M1.7 |
| `mcp_without_credential_fails_the_spec_cleanly` | an MCP server that refuses the agent gives `spec_invalid`, not a crash | M1.5, M1.9 |
| `egress_to_unlisted_destination_is_denied_and_recorded` | an agent cannot reach a destination outside its allow list, and a denial fails the turn without killing the agent | M1.8 |
| `turn_deadline_removes_the_agent` | a turn waiting on a slow model stops at the deadline, and the agent is removed and counted | M1.4, M1.5 |
| `second_concurrent_turn_on_one_agent_is_busy` | one turn at a time per agent (`busy`) | M1.5 |
| `capacity_is_enforced_and_released` | `at_capacity` at the pool limit; delete frees the slot | M1.3 |
| `idle_agents_are_evicted` | idle eviction | M1.12 |
| `turn_records_hold_no_content` | the JSONL record has requester, intent id and outcome, and neither the prompt nor the answer | M1.10 |
| `non_js_guest_implementing_the_contract_runs` | a hand-written WAT component that exports `taffee:agent/worker@0.1.0` runs as an agent | M1.11 |
| `guest_past_its_memory_limit_is_removed` | the 64 MB per-agent limit refuses growth; the trapping agent comes back as `agent_lost` (502) and is removed | M1.4, M1.5 |
| `cpu_bound_guest_is_preempted_and_does_not_starve_others` | an infinite-loop guest is stopped at its deadline, and on a single-threaded runtime a timer still fires on time while it spins | M1.4 |
| `http_api_end_to_end` | auth (401 without or with a wrong token), create, turn, records, list, stats, 400 and 404 errors, `invalid_spec`, delete, one-shot run | M1.1, M1.12 |
| **Work manifest unit tests** (`work::*`) | JCS canonical form (RFC 8785 key order, ECMAScript numbers), references and pinning, digest ignores who asked and how, unknown fields refused and `x-*` kept, name and version resolution, CEL checks with kind functions, deliver schema check, answer merge | M5 |
| **Conformance** (`tests/conformance.rs`) | all 32 vectors in `docs/spec/work-manifest/vectors/` pass, and every rule proved by vectors (47) is covered by passing ones. Sentence vectors go through the real normalizer role (strand-agent.wasm against the mock) | M5 |
| **Engine end to end** (`tests/work_test.rs`) | a sentence runs to COMPLETED with all checks passing, host-computed coverage 6/6 and one proposal not run; a worker that cites a line that does not exist gets FAILED and no answer; a planned run splits into three narrowed workers; an over-spending plan is rejected (WM-DLG-02) and falls back to one worker; a plan whose workers repeat the parent's parameters unchanged is accepted, and so is one that spends its whole offer, while one that changes `symbol` is rejected (named once) and falls back to one worker; usage covers the normalizer as well as the work; the same sentence runs to COMPLETED with every role on an `openai-chat` model, and the worker sees the file contents; a question that is not about code runs as `task.general@1` and the worker gets every tool; "book a flight" (`task.act@1`, irreversible) takes two deliberate steps, the readback then the approval, before it runs; a missing symbol is NOT_FOUND before any worker runs; over HTTP, a dry run returns the readback and the frozen manifest sent back with `reply: receipt` gets 202 and then COMPLETED | M5 |
| **Mock tests** (`mock-bedrock/test_server.py`) | the mock's Converse, tool, concurrency, MCP-over-SSE, header recording and loop behaviours, and OpenAI-style chat completions (a tool round trip) | test fixtures |
| **Live work** (`scripts/live-work.sh`, on the box, needs a key file) | one sentence through `/v1/work` with every role on a real chat model; the reply and a redacted log go to the evidence folder | B2 |
| **Image smoke** (`scripts/smoke-image.sh`, on the box) | the runtime image starts, becomes healthy, enforces API tokens, and runs create, two turns, stats and delete against the mock | M1.13 |
| **Density bench** (`density`; published and rerunnable with `scripts/bench-density.sh`, see [benchmarks/density.md](benchmarks/density.md)) | resident agents per host, marginal PSS per agent, CPU per turn; the rerun checks memory against the published runs | M1.14 |

## Not covered yet

- Real Bedrock. The SigV4 signer is verified against AWS's test vector and against the mock, but not against AWS itself (no credentials on the test box).
- Real taffee-context MCP. The guest's client is tested against the mock's SSE responses, which follow FastMCP's format.
- A restart of `orchestratord` with resident agents (M3).
- The runtime image under the taffee-core compose stack.
- `/v1/work` in the runtime image: the smoke test checks the image ships the kinds, but runs only the agent API.
- The work roles against a real model, beyond one live run: on 2026-11-17 `claude-haiku-5.5` via a model gateway took the PaymentService sentence to COMPLETED with all checks passing, planned as 4 parallel workers ($0.058, 12 model calls, 13 s) (`evidence/2026-11-17-taffee-localtest-node2/live-work.json`). The vector README asks for the normalizer to pass its vectors on 5 runs; that has not been done.
