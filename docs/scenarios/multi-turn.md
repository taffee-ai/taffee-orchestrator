# Multi-turn scenarios against the running service

**Written:** 2026-11-17. Three scenarios that test the orchestrator the way a caller uses it: as a running service, over several calls, with a real model (a hosted model gateway, `claude-haiku-5.5`). They run against the service `scripts/deploy-node.sh` starts on `taffee-localtest-node2`, from a workstation, with the caller's API bearer token.

"Multi-turn" means two things here, and the scenarios cover both:

- **Between the caller and the orchestrator:** clarifying questions, a readback, confirmation, approval, follow-up work. The work manifest is built for callers that cannot hold a chat, so each call is complete in itself.
- **Inside an agent:** the model calls tools repeatedly (search, read, search again) before it answers.

Order: C first (almost all built), then A, then B (most new code).

## C. Clarify, then confirm

A caller sends something ambiguous and is guided to a contract, one complete call at a time.

| Step | Caller sends | Pass when |
|---|---|---|
| 1 | "What breaks if I rename the payment class?" (no repo, no new name) | `REFUSED`, reason `unclear`, with a question and options for each gap (which repository, which symbol, the new name). Nothing runs; usage near zero |
| 2 | The same request with the answers: `want` plus `given.repo` and `parameters`, `dry_run: true` | `READY`: the frozen manifest, its digest, and `assumed` listing every default filled in |
| 3 | The frozen manifest back, `reply: "receipt"` | `202 ACCEPTED` with a receipt and the same digest |
| 4 | `GET /v1/work/{receipt}` until it changes | `COMPLETED`, all checks passing; answer equals the known set |
| 5 | The first entry of `proposals` from step 4 | it is accepted as a new request (`READY` on a dry run) |

To build: a script that walks the steps. Check that a real model's normalizer asks instead of guessing in step 1; if it guesses, tighten its prompt. Run step 1 five times (the vector README asks for 5 runs).

## A. Ask the codebase: a real tool loop on a real repository

A caller holds a conversation with one agent about a real repository. The agent answers by calling taffee-context's MCP tools (code search and graph queries), many calls per answer, and remembers earlier turns.

| Turn | Caller asks | Pass when |
|---|---|---|
| 1 | "Where is request validation done?" | an answer citing files (and lines where the tool gives them); several tool calls in the turn's records |
| 2 | "Which of those paths skip it?" | the answer builds on turn 1 without the caller repeating it |
| 3 | "What breaks if I rename `Depends`?" | file-level answer; the cited files exist in the repository |
| all | — | the agent stays within its turn and LLM-token limits; the MCP bearer token never reaches the agent (the host attaches it); every call is in the records with its credential name |

Uses `/v1/agents` (one agent, three turns), the guest's MCP client and the host's Bearer injection, all built (M1).

To decide or build:

- **Reaching the MCP server.** The orchestrator must be able to reach the code-search MCP server, for example by joining the same private container network and calling its `/mcp` endpoint by service name. The MCP server's port does not need to be published on the host.
- **An MCP bearer token for the orchestrator**, issued by the MCP server and held by the host as a Bearer credential rule (the agent never sees it).
- **Which repository it serves.** The MCP server indexes one repository at a pinned commit; confirm which one with the server's own tools before writing the expected answers.
- **A citation check.** For the agent API there is no kind and no host check; the script checks that every cited file exists in the repository at that commit.

## B. The rename, done for real

The impact analysis turns into a change: approved, applied as a patch, and checked by the host.

| Step | Caller sends | Pass when |
|---|---|---|
| 1 | "If I rename PaymentService to PaymentGateway in payments-service, what breaks?" | `COMPLETED` with the known answer, plus a proposal to update the docs mention |
| 2 | "Do the rename" (kind `code.rename@1`) | `REFUSED`, reason `needs_approval`: the change is `reversible`, above what the caller approved by default |
| 3 | The same, with `approved: "reversible"` | `COMPLETED`: the answer is a patch. Host checks: it applies to the pinned commit; afterwards no line declares, imports or calls the old name; every line the impact analysis found is changed |
| 4 | The docs proposal from step 1 | `COMPLETED`, or `PARTIAL` if the kind allows it |

To build: the `code.rename@1` kind (the registry has only a stub, used by the vectors), workers that return a unified diff through `submit`, and kind functions that apply the diff to the pinned input in memory and check the result. The pinned input is never written to; the patch is the deliverable.

## Not in these scenarios

Real repositories as work-manifest inputs (B4), receipts that survive a restart (B9), and governance of keys per agent.
