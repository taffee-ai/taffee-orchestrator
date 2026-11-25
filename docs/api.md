# orchestratord HTTP API

Base URL: `http://127.0.0.1:8780` by default. JSON in and out. When API tokens are configured, every `/v1` request needs `Authorization: Bearer <token>`, and the token's name is recorded as the requester.

## Agents

An agent is one long-lived Wasm instance bound to one **spec**. It keeps its conversation between turns, runs one turn at a time, and is removed when it is deleted, idle past the TTL, or its instance fails.

### `POST /v1/agents` → `201`

```json
{
  "spec": {
    "model": { "provider": "bedrock", "modelId": "…", "region": "us-east-1", "endpoint": "https://…", "maxTokens": 1024, "temperature": 0.2 },
    "systemPrompt": "You are …",
    "tools": ["calculator"],
    "mcpServers": [{ "name": "context", "url": "http://mcp:8765/mcp", "tools": ["semantic_search"] }],
    "limits": { "turns": 20, "totalTokens": 200000, "outputTokens": 20000, "toolOutputChars": 16000 },
    "messages": [{ "role": "user", "content": [{ "text": "…" }] }, { "role": "assistant", "content": [{ "text": "…" }] }]
  },
  "intent": { "objective": "…", "riskClass": "low", "constraints": {}, "acceptanceCriteria": {} }
}
```

`tools` names built-in tools: `calculator`; `http_request` (any URL; HTML comes back as text); `notebook` and `subagent` (Strands' own: scratch notes, and helper agents whose results the agent combines); `ask_model` (any model by name through the agent's chat gateway; needs `openai-chat`). Work-manifest workers get all of them unless their role names its own. There is no restriction on destinations when the deployment sets `egress.allow: ["*"]`; credentials are still attached by the host only for configured destinations.

`model.provider` is `bedrock` (Converse; the default) or `openai-chat` (OpenAI-style chat completions, which covers OpenRouter, LiteLLM and most gateways). For `openai-chat`, `endpoint` is the base URL up to the version (for example `https://model-gateway.example/v1`) and `modelId` is the provider's model name. The key is never in the spec: the host attaches it per destination from `credentials` (a `bearer` rule for chat providers).

Every field is optional. Unset spec fields come from the deployment's `default_spec`. Unknown fields are rejected. The agent may reach the model endpoint and MCP URLs its spec names, plus the deployment's `egress.allow` list, and nothing else. `messages` seeds the conversation (for example, when restoring a session) and is sent to the agent once.

Response: `AgentInfo` (`agent_id`, `spec_digest`, `created_at_ms`, `last_used_ms`, `turns`, `busy`, `requester`, `intent`, `spec`). Errors: `400 invalid_spec`, `429 at_capacity`.

### `GET /v1/agents`, `GET /v1/agents/{id}`, `DELETE /v1/agents/{id}` (`204`)

### `POST /v1/agents/{id}/turns`

```json
{ "prompt": "Calculate 6 * 7" }
```

`200` with a `TurnResponse`:

```json
{
  "agent_id": "ag-…",
  "turn": 1,
  "outcome": "ok",
  "result": "The calculation result is 42.",
  "stop_reason": "endTurn",
  "usage": { "inputTokens": 65, "outputTokens": 30, "totalTokens": 95 },
  "message_count": 4,
  "error": null,
  "calls": [
    { "seq": 1, "method": "POST", "scheme": "http", "authority": "127.0.0.1:8000", "path": "/model/…/converse",
      "outcome": "sent", "status": 200, "duration_ms": 3.1, "credential": "bedrock", "error": null }
  ],
  "duration_ms": 61.4
}
```

`outcome` other than `ok` while still `200`:

| outcome | meaning | agent |
|---|---|---|
| `turn_failed` | the model or a tool failed (`error` says why). This turn's messages are rolled back | kept, usable |
| `spec_invalid` | the agent could not start from its spec, for example an MCP server refused it | kept, but every turn fails the same way; delete it |

HTTP errors:

| status | `error` | meaning |
|---|---|---|
| 400 | `bad_request` | empty or oversized prompt, malformed body |
| 404 | `not_found` | no such agent |
| 409 | `busy` | another turn is running on this agent |
| 502 | `agent_lost` | the instance trapped (for example, it hit its memory limit). The agent is removed |
| 504 | `timeout` | the turn passed the deadline. The agent is removed |

The `calls` array is also present on `502` and `504` bodies.

### `GET /v1/agents/{id}/turns`

The agent's recent turn records (newest last). A record has the same fields as a turn response minus `result` and `error`, plus `spec_digest`, `requester` and `intent_id`. It never contains prompt or answer text.

## One-shot runs

### `POST /v1/runs`

```json
{ "prompt": "…", "spec": { … }, "intent": { … } }
```

Creates an agent, runs one turn, deletes it, and returns the `TurnResponse`. Pass prior history in `spec.messages` to run a stateless turn over a conversation the caller holds.

## Work

The caller-facing interface: one manifest in, one reply out. The spec is [`spec/work-manifest/`](spec/work-manifest/README.md). These routes exist when the orchestrator has a `work` configuration (or `TAFFEE_ORCH_KINDS_DIR` is set).

### `POST /v1/work` → `200`, or `202` when the reply is `ACCEPTED`

Body: a work manifest, sparse or frozen. The caller's principal is the token name (`user:alice`; a name with no `kind:` prefix becomes `service:<name>`), or `user:anonymous` when the API is open.

```json
{ "manifest": "0.1", "want": "If I rename PaymentService to PaymentGateway in payments-service, what breaks?" }
```

Every outcome is a reply document with HTTP 200, including `REFUSED`: the reply carries the reason, the rule ids it breaks, and questions or a counter-offer. `dry_run: true` returns `READY` with the frozen manifest. Sending that manifest back, with `approved` set when the work has effects, runs it. `reply: "receipt"` returns `202 ACCEPTED` with a `receipt`.

### `GET /v1/work/{receipt}` → `200`

The final reply, or `ACCEPTED` while the work runs. `404` for an unknown receipt. Receipts are kept in memory and do not survive a restart.

### The `work` configuration

| Field | Default | |
|---|---|---|
| `kinds_dir` | required | directory of intent kinds (YAML or JSON); the image ships `/opt/taffee/kinds` |
| `principal` | `orchestrator:local` | `by` on manifests this orchestrator hands on |
| `resolve.names`, `resolve.versions`, `resolve.default_version` | empty, empty, `main` | short name → full names; `name@version` → commit |
| `policy.principals`, `policy.default` | default: read-only, $1, 30m | per principal: `max_effects`, `max_usd`, `max_time` |
| `inputs` | empty | pinned ref → fixture (`.yaml`) or checked-out directory, read by checks and coverage |
| `usd_per_1k_tokens` | 0.003 | turns token usage into `usage.usd`, and a worker's usd into its token budget |
| `max_workers`, `worker_turns` | 4, 30 | |
| `roles.normalizer` | absent: sentences are refused as unsupported | agent spec for reading sentences |
| `roles.planner` | absent: one worker per manifest | agent spec for the outer plan |
| `roles.worker` | the default spec | agent spec for workers; give it the MCP tools that read inputs |

## Operations

- `GET /healthz` → `{"ok": true}`. No authentication.
- `GET /v1/stats`: agents (active, capacity, created, removed after failure, evicted, rejected), turns (ok, failed, timed out), process memory (RSS, PSS, private dirty), component facts, credential rule names, limits.

## Configuration

Read from `TAFFEE_ORCH_CONFIG` (JSON; see `deploy/orchestrator.example.json`). These environment variables override it:

| Variable | Default |
|---|---|
| `TAFFEE_ORCH_LISTEN` | `127.0.0.1:8780` |
| `TAFFEE_ORCH_COMPONENT` | `../strand-agent/dist/strand-agent.wasm` |
| `TAFFEE_ORCH_CACHE_DIR` | next to the component |
| `TAFFEE_ORCH_MAX_AGENTS` | 100 |
| `TAFFEE_ORCH_MEMORY_LIMIT_MB` | 64 (per agent) |
| `TAFFEE_ORCH_TURN_TIMEOUT_SECS` | 120 |
| `TAFFEE_ORCH_IDLE_TTL_SECS` | 900 |
| `TAFFEE_ORCH_RECORDS` | unset (no JSONL file) |
| `TAFFEE_ORCH_API_TOKENS_FILE` | unset (API open; logged as a warning) |
| `TAFFEE_ORCH_MODEL_ENDPOINT`, `TAFFEE_ORCH_MODEL_ID` | set the default spec's model |
| `TAFFEE_ORCH_EGRESS_ALLOW` | comma-separated extra allow entries |
| `TAFFEE_ORCH_GUEST_STDIO` | `false` |
| `TAFFEE_ORCH_KINDS_DIR` | unset; set it to enable `/v1/work` with default `work` settings |

Credential rules are in the config file only. Each rule names a destination and the environment variable (or file) that holds the secret; the secret is read at startup and never logged or sent to a guest.
