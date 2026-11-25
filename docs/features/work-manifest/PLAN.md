# Plan: the orchestrator speaks the work manifest

Spec: [`docs/spec/work-manifest/`](../../spec/work-manifest/README.md). This plan turns the M1 orchestrator (callers create agents and drive turns) into one that takes a manifest and returns a reply (roadmap M5).

## The split that decides everything

| Host (Rust, deterministic) | Wasm guests (models) |
|---|---|
| intake, schema and field checks, name and version resolution | **normalizer agent**: a sentence becomes a kind, parameters and input names |
| freeze: kind defaults, policy caps, `assumed`, JCS digest | **outer-plan agent**: splits the work into worker manifests |
| admit: approval, policy, budget | **worker agents**: do the work, submit an answer and the files they searched |
| narrowing checks on every worker manifest | |
| coverage, computed from the pinned inputs | |
| CEL checks and kind functions, which read the pinned inputs | |
| status rules and reply assembly; receipts | |

No model decides an outcome. Guests return structured data through a `submit` tool, and the host validates every piece of it.

## Modules (`runner/src/work/`)

| Module | Does |
|---|---|
| `jcs.rs` | RFC 8785 canonical JSON, `sha256:` digests |
| `refs.rs` | `name[@version][#sha256:…]` parsing; the pinned check |
| `manifest.rs` | the manifest type; work fields; validation (unknown fields, `x-*`) |
| `kind.rs` | kind type, registry (a directory of YAML), kind digest |
| `context.rs` | resolver (names to repositories, versions to commits), policy per principal, prices |
| `inputs.rs` | input store: pinned repositories (a fixture file, or a checked-out directory) |
| `normalize.rs` | sparse request to frozen manifest, or a refusal; the sentence-normalizer trait |
| `admit.rs` | approval, policy and budget; dry run |
| `checks.rs` | CEL over answer, coverage, given, parameters and usage; kind functions; the implicit `deliver` check |
| `reply.rs` | the reply document; status rules (WM-RES-02) |
| `delegate.rs` | narrowing rules (WM-DLG-*) |
| `engine.rs` | runs a frozen manifest: plan, worker manifests, workers, merge, checks, reply; receipts |
| `agents.rs` | builds the `AgentSpec` and prompt for each role, and parses each `submit` |

API: `POST /v1/work` (a manifest, returns a reply), `GET /v1/work/{receipt}`. `/v1/agents` and `/v1/runs` stay available for operators.

Guest: `AgentSpec.output = { schema }` adds a built-in `submit` tool with that input schema. Its input comes back as `submitted` in the turn reply. This is configuration in `agent.ts`; nothing in the SDK changes.

Mock model: system prompts carry a `TAFFEE-ROLE: normalizer|planner|worker` line. The mock acts each role deterministically: it parses the sentence, splits by top directory, and greps a fixture repository. The keyword `hallucinate` makes a worker cite a line that does not exist.

## Status (2026-11-16)

W1 to W4 are built and pass on the test box (Docker): 41 unit tests, the 19 M1 integration tests unchanged, 32 of 32 conformance vectors with all 47 vector-proved rules covered, and 6 of 6 engine tests. The runtime image ships the kinds at `/opt/taffee/kinds`. See `docs/evidence/2026-11-16-taffee-localtest-node1/`.

## Steps and exit criteria

| Step | Builds | Done when |
|---|---|---|
| **W1** intake, freeze, admit | jcs, refs, manifest, kind, context, normalize (structured requests), admit, delegate | `tests/conformance.rs` passes every `normalize`, `handle` and `delegate` vector that has no sentence in it |
| **W2** checks and reply, one worker | inputs, checks, reply, engine (single worker), agents (worker role), guest `submit`, mock worker | every `result` vector passes; an integration test runs PaymentService end to end through `/v1/work` against the mock and gets COMPLETED, and the `hallucinate` run gets FAILED |
| **W3** outer plan | agents (planner role), worker manifests with narrowing, budget split, merge | a planned run splits into three workers that each pass the narrowing checks, and the merged answer is COMPLETED; a planner that over-spends is refused and the run falls back to one worker |
| **W4** sentence normalizer | agents (normalizer role), mock normalizer | every sentence vector passes (N-01, R-02, R-03, R-07, R-08, R-09, R-11), and the readback over HTTP shows `assumed` |

Each step runs `scripts/remote.sh` (Docker on the test box) and keeps the M1 tests green.

## Decisions made here

- **Coverage is computed by the host.** For a kind that declares `coverage`, the host lists the files under `parameters.paths` at the pinned commit, and workers only report which files they searched. A worker's own count is ignored. This adds rule WM-CHK-06 and changes the `result` vectors to `work.searched`.
- **Worker manifests always have `delegation.depth: 0`.** `depth` counts hand-offs to another orchestrator, not the inner loop. The narrowing rules still apply in full. WM-DLG-04 is reworded to say so.
- **Name resolution is static configuration for now** (`resolve.names`, `resolve.versions`), and so is the input store (a fixture or a directory per pinned ref). Fetching from git is out of scope.
- **Usage in USD comes from a configured price per 1,000 tokens.** A worker's token budget is its USD share divided by that price.
- **If the planner fails or proposes worker manifests that don't narrow,** the run falls back to a single worker carrying the whole manifest, and the reply's trace records it.

## Not in this build

Fetching inputs from git, idempotency by `id` (a SHOULD), cancel, progress, verifiers other than CEL, persistence of receipts across restarts.
