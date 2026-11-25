# Guest contract: `taffee:agent/worker@0.1.0`

Any WebAssembly component can be an orchestrator agent if it exports this interface ([strand-agent/wit/world.wit](../strand-agent/wit/world.wit)):

```wit
package taffee:agent@0.1.0;
interface worker {
  run-turn: func(input: string) -> string;
}
```

The language does not matter. The reference guest is a Strands TypeScript agent built with componentize-js. The integration tests also run a hand-written WAT guest.

## Lifecycle

- One instance is one agent. The host instantiates it when the agent is created and calls `run-turn` once per user turn, never concurrently on one instance.
- Module-level state persists between calls. That is where the conversation lives.
- Anything a pre-initializer (Wizer) runs at module top level is shared by every instance cloned from the snapshot. Per-instance values (ids, random seeds, clients) must be created on the first call.
- The host drops the instance on a trap, a deadline, invalid output, deletion or idle eviction. There is no shutdown call.

## Input

```json
{ "prompt": "…", "spec": { "model": {…}, "systemPrompt": "…", "tools": [], "mcpServers": [], "limits": {}, "messages": [], "output": { "schema": {…} } } }
```

`spec.model.provider` selects how the guest talks to the model: `bedrock` (the SDK's `BedrockModel`, Converse) or `openai-chat` (a small chat-completions model in `agent.ts` built on the SDK's public `Model` class, using plain `fetch`; no client library). Either way the guest sends placeholder credentials, and the host replaces them.

`spec.tools` picks built-in tools: `calculator`, `http_request` (plain `fetch`; Strands' vended `http_request` uses `AbortSignal.timeout`/`any`, which the sandbox's JavaScript engine lacks), `notebook` and `subagent` (Strands vended-tools), and `ask_model` (any model by name through the chat gateway). Tools that need an operating system (shell, Python, files) are not available in the sandbox.

`spec.output` asks for structured output. The guest adds a built-in tool, `submit`, whose input schema is `output.schema` (a JSON Schema with `type: object`). Whatever the model passes to `submit` comes back as `submitted`. The work engine uses this for every model role (normalizer, planner, worker), and the host validates what comes back; the guest does not.

The host sends the spec on every turn, with `messages` (seed history) on the first turn only. The guest builds its agent from the first turn's spec. A later turn whose spec (excluding `messages`) differs gets a `409 spec_conflict` reply.

## Output

A JSON object:

| field | |
|---|---|
| `httpStatus` | 200 on success; 400 `spec_invalid`, 409 `spec_conflict`, 502 `turn_failed` |
| `status` | `"ok"` or `"error"` |
| `errorKind` | on errors: `spec_invalid`, `spec_conflict`, `turn_failed` |
| `result` | the answer text |
| `stopReason` | from the loop, e.g. `endTurn`, `limitTurns` |
| `usage` | token usage for this turn, if known |
| `messageCount` | conversation length after the turn |
| `message` | error text |
| `submitted` | the last input the model passed to `submit` in this turn, or `null` (only with `spec.output`) |

Output that is not a JSON object is treated as a broken instance, and the agent is removed.

## What the host provides

Imports: `wasi:clocks`, `wasi:random`, `wasi:cli` stdio (discarded unless `guest_stdio` is on) and `wasi:http/outgoing-handler`. There is no filesystem, no environment variables, no sockets other than HTTP through the host, and no secrets.

Outbound HTTP goes through the host's egress gate. Requests to destinations the agent's spec does not name are answered by the host with `403` and the header `x-taffee-egress: denied`. For destinations the host holds a credential for, the host removes the guest's `Authorization` and `x-amz-*` signature headers and attaches its own, so a guest uses placeholder credentials.

## Limits

- Memory: 64 MB of linear memory per agent by default. Growing past it traps, and the agent is removed.
- Time: one deadline per turn. A guest that is computing is stopped by epoch interruption. A guest that is waiting on the network is stopped by the host timeout. Running guests yield to the host scheduler every 10 ms.
- Loop: the reference guest passes `limits.turns`, `totalTokens` and `outputTokens` to the Strands SDK, which ends the turn with a `limit…` stop reason.

## Why not `wasi:http/incoming-handler`

The componentize-js fetch event is one-shot per instance: a second request fails with "respondWith can't be called twice". A plain export can be called repeatedly on the same instance, which a long-lived agent needs.
