// taffee-orchestrator reference guest: an unmodified Strands TypeScript agent behind the
// `taffee:agent/worker@0.1.0` contract (wit/world.wit, docs/guest-contract.md).
//
// Rules this file keeps:
// - No secrets in the guest. Model and MCP calls go out with placeholder credentials; the
//   host strips them and injects real ones for the destinations it has rules for.
// - Nothing is created at module top level that must differ per instance: top level runs
//   once during Wizer pre-warming and is then shared by every instance cloned from the
//   snapshot.
// - Only SDK configuration and wrapper code here; nothing underneath the SDK is changed.

import { Agent, BedrockModel, FunctionTool, Model } from "@strands-agents/sdk";
// Strands' own generic tools (vended-tools), from the pinned SDK source the bundle already uses.
// (Its http_request uses AbortSignal.timeout/any, which the sandbox's JS engine lacks; ours is below.)
import { makeNotebook } from "../../upstream/strands-harness-sdk/strands-ts/src/vended-tools/notebook/index.js";
import { makeSubagent } from "../../upstream/strands-harness-sdk/strands-ts/src/vended-tools/subagent/index.js";

const DEFAULT_ENDPOINT = "http://127.0.0.1:8000";
const DEFAULT_MODEL_ID = "anthropic.claude-3-sonnet";
const DEFAULT_TOOL_OUTPUT_CHARS = 16_000;

// ---------------------------------------------------------------------------------------
// Turn input / agent spec (mirrors runner/src/spec.rs)
// ---------------------------------------------------------------------------------------

interface ModelSpec {
  /** "bedrock" (Converse, the default) or "openai-chat" (OpenAI-style chat completions:
   *  OpenRouter, LiteLLM and most gateways). */
  provider?: "bedrock" | "openai-chat";
  modelId?: string;
  region?: string;
  endpoint?: string;
  maxTokens?: number;
  temperature?: number;
}

interface McpServerSpec {
  name: string;
  url: string;
  /** Only expose these tool names from the server (default: all it lists). */
  tools?: string[];
}

interface LimitsSpec {
  /** Model turns (model calls) per user turn; enforced by the SDK. */
  turns?: number;
  totalTokens?: number;
  outputTokens?: number;
  /** Tool output is truncated to this many characters before it reaches the model. */
  toolOutputChars?: number;
}

interface AgentSpec {
  model?: ModelSpec;
  systemPrompt?: string;
  /** Built-in tools by name: calculator, http_request, notebook, subagent, ask_model. */
  tools?: string[];
  mcpServers?: McpServerSpec[];
  limits?: LimitsSpec;
  /** Prior conversation to start from (Bedrock message shape). */
  messages?: unknown[];
  /** Structured output: adds a built-in `submit` tool with this input schema. */
  output?: OutputSpec;
}

interface OutputSpec {
  schema: Record<string, unknown>;
  description?: string;
}

interface TurnInput {
  prompt?: string;
  spec?: AgentSpec;
  /** Legacy field from an earlier runner. Same as spec.model.endpoint. */
  endpoint?: string;
}

// ---------------------------------------------------------------------------------------
// OpenAI-style chat completions model (plain fetch, no client library)
// ---------------------------------------------------------------------------------------

interface ChatModelConfig {
  modelId?: string;
  maxTokens?: number;
  temperature?: number;
  /** Base URL up to and including the version, e.g. https://openrouter.ai/api/v1 */
  endpoint?: string;
}

/** Strands message blocks -> chat messages. Tool results follow the assistant turn that asked. */
function toChatMessages(messages: any[], systemPrompt: unknown): any[] {
  const out: any[] = [];
  const system = typeof systemPrompt === "string"
    ? systemPrompt
    : Array.isArray(systemPrompt) ? systemPrompt.map((b: any) => b?.text ?? "").join("\n") : "";
  if (system) out.push({ role: "system", content: system });
  for (const m of messages) {
    const texts: string[] = [];
    const toolCalls: any[] = [];
    const results: any[] = [];
    for (const b of m?.content ?? []) {
      if (b?.type === "textBlock") texts.push(b.text);
      else if (b?.type === "toolUseBlock") {
        toolCalls.push({ id: b.toolUseId, type: "function", function: { name: b.name, arguments: JSON.stringify(b.input ?? {}) } });
      } else if (b?.type === "toolResultBlock") {
        const text = (b.content ?? [])
          .map((c: any) => c?.type === "textBlock" ? c.text : c?.type === "jsonBlock" ? JSON.stringify(c.json) : "")
          .join("\n");
        results.push({ role: "tool", tool_call_id: b.toolUseId, content: b.status === "error" ? `Error: ${text}` : text });
      }
    }
    if (m?.role === "assistant") {
      const msg: any = { role: "assistant", content: texts.join("\n") || null };
      if (toolCalls.length) msg.tool_calls = toolCalls;
      out.push(msg);
    } else {
      out.push(...results);
      if (texts.length) out.push({ role: "user", content: texts.join("\n") });
    }
  }
  return out;
}

/** One non-streaming POST per model call. The guest sends a placeholder key; the host
 *  replaces it with the real one for this endpoint, so no secret enters the sandbox. */
class ChatCompletionsModel extends Model<ChatModelConfig> {
  private config: ChatModelConfig;

  constructor(config: ChatModelConfig) {
    super();
    this.config = { ...config };
  }

  updateConfig(config: ChatModelConfig): void {
    this.config = { ...this.config, ...config };
  }

  getConfig(): ChatModelConfig {
    return this.config;
  }

  async *stream(messages: any[], options?: any): AsyncIterable<any> {
    const body: any = {
      model: this.config.modelId,
      messages: toChatMessages(messages, options?.systemPrompt),
      stream: false,
    };
    if (this.config.maxTokens !== undefined) body.max_tokens = this.config.maxTokens;
    if (this.config.temperature !== undefined) body.temperature = this.config.temperature;
    const tools = (options?.toolSpecs ?? []).map((t: any) => ({
      type: "function",
      function: { name: t.name, description: t.description ?? "", parameters: t.inputSchema ?? { type: "object", properties: {} } },
    }));
    if (tools.length) body.tools = tools;

    const url = `${(this.config.endpoint ?? "").replace(/\/+$/, "")}/chat/completions`;
    const res = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: "Bearer TAFFEE_PLACEHOLDER" },
      body: JSON.stringify(body),
    });
    const text = await res.text();
    if (!res.ok) throw new Error(`model call failed: HTTP ${res.status}: ${text.slice(0, 300)}`);
    const data = JSON.parse(text);
    const choice = data?.choices?.[0] ?? {};
    const msg = choice.message ?? {};
    const calls: any[] = Array.isArray(msg.tool_calls) ? msg.tool_calls : [];

    yield { type: "modelMessageStartEvent", role: "assistant" };
    const content = typeof msg.content === "string" ? msg.content : "";
    if (content) {
      yield { type: "modelContentBlockStartEvent" };
      yield { type: "modelContentBlockDeltaEvent", delta: { type: "textDelta", text: content } };
      yield { type: "modelContentBlockStopEvent" };
    }
    for (let i = 0; i < calls.length; i++) {
      const c = calls[i];
      yield {
        type: "modelContentBlockStartEvent",
        start: { type: "toolUseStart", name: c?.function?.name ?? "", toolUseId: c?.id || `call_${Date.now()}_${i}` },
      };
      yield { type: "modelContentBlockDeltaEvent", delta: { type: "toolUseInputDelta", input: c?.function?.arguments || "{}" } };
      yield { type: "modelContentBlockStopEvent" };
    }
    const stopReason = calls.length ? "toolUse" : choice.finish_reason === "length" ? "maxTokens" : "endTurn";
    yield { type: "modelMessageStopEvent", stopReason };
    const u = data?.usage;
    if (u) {
      const input = u.prompt_tokens ?? 0;
      const output = u.completion_tokens ?? 0;
      yield { type: "modelMetadataEvent", usage: { inputTokens: input, outputTokens: output, totalTokens: u.total_tokens ?? input + output } };
    }
  }
}

// ---------------------------------------------------------------------------------------
// Built-in tools
// ---------------------------------------------------------------------------------------

function calculatorTool(): FunctionTool {
  return new FunctionTool({
    name: "calculator",
    description: "Calculates arithmetic expression, e.g. '21 * 2'",
    inputSchema: {
      type: "object",
      properties: {
        expression: { type: "string", description: "The mathematical expression to evaluate" },
      },
      required: ["expression"],
    },
    callback: (input: any) => {
      const expr = String(input?.expression || "").trim();
      const match = expr.match(/^(-?\d+(?:\.\d+)?)\s*([\+\-\*\/])\s*(-?\d+(?:\.\d+)?)$/);
      if (!match) throw new Error(`unsupported expression: ${expr.slice(0, 200)}`);
      const a = parseFloat(match[1]);
      const b = parseFloat(match[3]);
      switch (match[2]) {
        case "+": return String(a + b);
        case "-": return String(a - b);
        case "*": return String(a * b);
        default:
          if (b === 0) throw new Error("division by zero");
          return String(a / b);
      }
    },
  });
}

/** HTML to readable text: drop scripts, styles and tags, keep link targets, squeeze whitespace. */
function htmlToText(html: string): string {
  return html
    .replace(/<(script|style|noscript|svg)[\s\S]*?<\/\1>/gi, " ")
    .replace(/<a\s[^>]*href=["']([^"']+)["'][^>]*>([\s\S]*?)<\/a>/gi, (_m, href, text) => `${text} (${href})`)
    .replace(/<(br|\/p|\/div|\/li|\/tr|\/h[1-6])[^>]*>/gi, "\n")
    .replace(/<[^>]+>/g, " ")
    .replace(/&nbsp;/g, " ").replace(/&amp;/g, "&").replace(/&lt;/g, "<").replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"').replace(/&#39;/g, "'")
    .replace(/[ \t]+/g, " ").replace(/\n\s*\n+/g, "\n").trim();
}

/** Any HTTP request, with plain fetch (the host enforces the turn's deadline). HTML comes back
 *  as text unless `raw` is set. Credentials, if any, are attached by the host per destination. */
function httpRequestTool(): FunctionTool {
  return new FunctionTool({
    name: "http_request",
    description: "Make an HTTP request to any URL (GET, POST, ...) and get the status and body. " +
      "HTML pages come back as readable text with link targets; set raw to get the HTML itself.",
    inputSchema: {
      type: "object",
      properties: {
        method: { type: "string", description: "GET, POST, PUT, PATCH, DELETE, HEAD (default GET)" },
        url: { type: "string", description: "The full URL" },
        headers: { type: "object", description: "Request headers", additionalProperties: { type: "string" } },
        body: { type: "string", description: "Request body (for POST, PUT, PATCH)" },
        raw: { type: "boolean", description: "Return HTML as is instead of as text" },
      },
      required: ["url"],
    },
    callback: async (input: any) => {
      const method = String(input?.method || "GET").toUpperCase();
      const init: any = { method, headers: { "user-agent": "taffee-agent/0.1", ...(input?.headers || {}) } };
      if (input?.body !== undefined && method !== "GET" && method !== "HEAD") init.body = String(input.body);
      const res = await fetch(String(input?.url), init);
      const type = res.headers.get("content-type") || "";
      let body = method === "HEAD" ? "" : await res.text();
      if (!input?.raw && type.includes("html")) body = htmlToText(body);
      return JSON.stringify({ status: res.status, contentType: type, body });
    },
  });
}

/** Ask any model, by name, through the agent's own chat gateway (OpenRouter, LiteLLM,
 *  and most gateways). The host attaches the gateway's key; the guest never sees it. */
function askModelTool(m: ModelSpec): FunctionTool | null {
  if ((m.provider || "bedrock") !== "openai-chat") return null;
  const url = `${(m.endpoint || `${DEFAULT_ENDPOINT}/v1`).replace(/\/+$/, "")}/chat/completions`;
  return new FunctionTool({
    name: "ask_model",
    description: "Ask any LLM, by model name, and get its text reply. Use it for a second opinion, a specialist " +
      "model, or to compare and combine answers from several models.",
    inputSchema: {
      type: "object",
      properties: {
        model: { type: "string", description: "Model name as the gateway knows it, e.g. claude-haiku-5.5" },
        prompt: { type: "string", description: "The question or instruction" },
        system: { type: "string", description: "Optional system prompt" },
        max_tokens: { type: "number", description: "Optional cap on the reply's LLM tokens" },
      },
      required: ["model", "prompt"],
    },
    callback: async (input: any) => {
      const messages: any[] = [];
      if (input?.system) messages.push({ role: "system", content: String(input.system) });
      messages.push({ role: "user", content: String(input?.prompt ?? "") });
      const body: any = { model: String(input?.model ?? ""), messages, stream: false };
      if (typeof input?.max_tokens === "number") body.max_tokens = input.max_tokens;
      const res = await fetch(url, {
        method: "POST",
        headers: { "content-type": "application/json", authorization: "Bearer TAFFEE_PLACEHOLDER" },
        body: JSON.stringify(body),
      });
      const text = await res.text();
      if (!res.ok) throw new Error(`ask_model: HTTP ${res.status}: ${text.slice(0, 300)}`);
      const data = JSON.parse(text);
      return String(data?.choices?.[0]?.message?.content ?? "");
    },
  });
}

/** Built-in tools by name. Each is made per agent (nothing per-instance at module level).
 *  A factory may return null when the tool does not apply (ask_model without a chat gateway). */
const BUILTIN_TOOLS: Record<string, (m: ModelSpec) => any> = {
  calculator: () => calculatorTool(),
  http_request: () => httpRequestTool(),
  notebook: () => makeNotebook(),
  subagent: () => makeSubagent(),
  ask_model: (m) => askModelTool(m),
};

// The model hands its result to the host through this tool. The host validates it; the guest
// only keeps the last submission of the current turn.
interface SubmitBox {
  value: unknown;
}

function submitTool(output: OutputSpec, box: SubmitBox): FunctionTool {
  return new FunctionTool({
    name: "submit",
    description: output.description ||
      "Submit your result. Call this exactly once, when you are finished, with the complete result.",
    inputSchema: output.schema as any,
    callback: (input: any) => {
      box.value = input ?? null;
      return "Submitted. Reply with one short sentence and stop.";
    },
  });
}

// ---------------------------------------------------------------------------------------
// Minimal MCP client (Streamable HTTP transport, JSON or SSE responses)
//
// Kept to the three calls an agent needs (initialize, tools/list, tools/call) so it runs on
// the guest's fetch without depending on EventSource or Node streams. The host attaches the
// Authorization header for the server's authority; the guest never sees the token.
// ---------------------------------------------------------------------------------------

const MCP_PROTOCOL_VERSION = "2025-06-18";

class McpHttpClient {
  private sessionId: string | null = null;
  private nextId = 1;
  constructor(readonly name: string, readonly url: string) {}

  async initialize(): Promise<void> {
    await this.rpc("initialize", {
      protocolVersion: MCP_PROTOCOL_VERSION,
      capabilities: {},
      clientInfo: { name: "taffee-orchestrator-guest", version: "0.1.0" },
    });
    await this.rpc("notifications/initialized", {}, true);
  }

  async listTools(): Promise<any[]> {
    const result = await this.rpc("tools/list", {});
    return Array.isArray(result?.tools) ? result.tools : [];
  }

  async callTool(name: string, args: unknown): Promise<any> {
    return this.rpc("tools/call", { name, arguments: args ?? {} });
  }

  private async rpc(method: string, params: unknown, notification = false): Promise<any> {
    const headers: Record<string, string> = {
      "content-type": "application/json",
      accept: "application/json, text/event-stream",
      "mcp-protocol-version": MCP_PROTOCOL_VERSION,
    };
    if (this.sessionId) headers["mcp-session-id"] = this.sessionId;
    const id = notification ? undefined : this.nextId++;
    const payload = notification
      ? { jsonrpc: "2.0", method, params }
      : { jsonrpc: "2.0", id, method, params };

    const res = await fetch(this.url, { method: "POST", headers, body: JSON.stringify(payload) });
    const sid = res.headers.get("mcp-session-id");
    if (sid) this.sessionId = sid;
    const text = await res.text();
    if (notification) return undefined;
    if (!res.ok) {
      throw new Error(`MCP ${this.name} ${method}: HTTP ${res.status} ${text.slice(0, 200)}`);
    }

    let message: any = null;
    const contentType = res.headers.get("content-type") || "";
    if (contentType.includes("text/event-stream")) {
      for (const line of text.split(/\r?\n/)) {
        if (!line.startsWith("data:")) continue;
        try {
          const candidate = JSON.parse(line.slice(5).trim());
          if (candidate && candidate.id === id) message = candidate;
        } catch {
          // keep-alive or partial frame; ignore
        }
      }
    } else {
      message = JSON.parse(text);
    }
    if (!message) throw new Error(`MCP ${this.name} ${method}: no response for request ${id}`);
    if (message.error) {
      throw new Error(`MCP ${this.name} ${method}: ${message.error.message || "error"}`);
    }
    return message.result;
  }
}

function truncate(text: string, max: number): string {
  if (text.length <= max) return text;
  return `${text.slice(0, max)}\n[truncated ${text.length - max} characters]`;
}

async function mcpTools(server: McpServerSpec, maxChars: number): Promise<FunctionTool[]> {
  const client = new McpHttpClient(server.name, server.url);
  await client.initialize();
  const listed = await client.listTools();
  const allow = server.tools ? new Set(server.tools) : null;
  return listed
    .filter((t) => t && typeof t.name === "string" && (!allow || allow.has(t.name)))
    .map(
      (t) =>
        new FunctionTool({
          name: t.name,
          description: String(t.description || `${t.name} (MCP server ${server.name})`).slice(0, 1000),
          inputSchema: t.inputSchema || { type: "object", properties: {} },
          callback: async (input: any) => {
            const result = await client.callTool(t.name, input);
            const text = (Array.isArray(result?.content) ? result.content : [])
              .map((c: any) => (c?.type === "text" ? String(c.text) : JSON.stringify(c)))
              .join("\n");
            if (result?.isError) throw new Error(truncate(text || "MCP tool error", 2000));
            return truncate(text, maxChars);
          },
        }),
    );
}

// ---------------------------------------------------------------------------------------
// Worker state: one long-lived Agent per instance
// ---------------------------------------------------------------------------------------

interface Worker {
  agent: Agent;
  specKey: string;
  spec: AgentSpec;
  workerId: string;
  requestCount: number;
  toolNames: string[];
  submitted: SubmitBox;
}

let current: Worker | null = null;

function newWorkerId(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes); // wasi:random at runtime
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

function normalizeSpec(input: TurnInput): AgentSpec {
  const spec: AgentSpec = { ...(input.spec || {}) };
  spec.model = { ...(spec.model || {}) };
  if (!spec.model.endpoint && input.endpoint) spec.model.endpoint = input.endpoint;
  return spec;
}

// The spec binds on the first turn; its identity excludes `messages`, which only seeds
// history and is not resent.
function specKeyOf(spec: AgentSpec): string {
  const { messages: _ignored, ...rest } = spec;
  return JSON.stringify(rest);
}

async function createWorker(spec: AgentSpec, specKey: string): Promise<Worker> {
  const m = spec.model || {};
  const provider = m.provider || "bedrock";
  if (provider !== "bedrock" && provider !== "openai-chat") {
    throw new Error(`unsupported model provider: ${m.provider}`);
  }
  const model = provider === "openai-chat"
    ? new ChatCompletionsModel({
        modelId: m.modelId || DEFAULT_MODEL_ID,
        endpoint: m.endpoint || `${DEFAULT_ENDPOINT}/v1`,
        ...(m.maxTokens !== undefined && { maxTokens: m.maxTokens }),
        ...(m.temperature !== undefined && { temperature: m.temperature }),
      })
    : new BedrockModel({
        modelId: m.modelId || DEFAULT_MODEL_ID,
        stream: false, // Converse (JSON). ConverseStream is a later gate.
        ...(m.maxTokens !== undefined && { maxTokens: m.maxTokens }),
        ...(m.temperature !== undefined && { temperature: m.temperature }),
        clientConfig: {
          region: m.region || "us-east-1",
          endpoint: m.endpoint || DEFAULT_ENDPOINT,
          // Placeholders. The host replaces the signature for destinations it holds keys for.
          credentials: { accessKeyId: "TAFFEE_PLACEHOLDER", secretAccessKey: "TAFFEE_PLACEHOLDER" },
        },
      });

  const maxChars = spec.limits?.toolOutputChars ?? DEFAULT_TOOL_OUTPUT_CHARS;
  const tools: FunctionTool[] = [];
  for (const name of spec.tools ?? ["calculator"]) {
    const make = BUILTIN_TOOLS[name];
    if (!make) throw new Error(`unknown built-in tool: ${name}`);
    const tool = make(m);
    if (tool) tools.push(tool);
  }
  for (const server of spec.mcpServers ?? []) {
    tools.push(...(await mcpTools(server, maxChars)));
  }
  const submitted: SubmitBox = { value: undefined };
  if (spec.output) {
    if (!spec.output.schema || (spec.output.schema as any).type !== "object") {
      throw new Error("output.schema must be a JSON Schema with type object");
    }
    tools.push(submitTool(spec.output, submitted));
  }

  const agent = new Agent({
    model,
    tools,
    printer: false,
    ...(spec.systemPrompt !== undefined && { systemPrompt: spec.systemPrompt }),
    ...(Array.isArray(spec.messages) && { messages: spec.messages as any }),
  });

  return {
    agent,
    spec,
    specKey,
    workerId: newWorkerId(),
    requestCount: 0,
    toolNames: tools.map((t) => t.name),
    submitted,
  };
}

function reply(httpStatus: number, payload: Record<string, unknown>): string {
  return JSON.stringify({ httpStatus, ...payload });
}

function invokeLimits(limits: LimitsSpec | undefined): Record<string, number> | undefined {
  if (!limits) return undefined;
  const out: Record<string, number> = {};
  if (limits.turns !== undefined) out.turns = limits.turns;
  if (limits.totalTokens !== undefined) out.totalTokens = limits.totalTokens;
  if (limits.outputTokens !== undefined) out.outputTokens = limits.outputTokens;
  return Object.keys(out).length ? out : undefined;
}

// Calls into one instance are serialized by the host (each export call borrows the Store
// mutably and runs to completion), so there is no in-guest queue.
async function runTurn(text: string): Promise<string> {
  let input: TurnInput;
  try {
    input = text ? JSON.parse(text) : {};
  } catch {
    input = { prompt: text };
  }
  const prompt = typeof input.prompt === "string" && input.prompt ? input.prompt : "Hello world";

  let w: Worker;
  try {
    const spec = normalizeSpec(input);
    const key = specKeyOf(spec);
    if (!current) {
      current = await createWorker(spec, key);
    } else if (input.spec && key !== current.specKey) {
      return reply(409, {
        status: "error",
        errorKind: "spec_conflict",
        message: "agent is already bound to a different spec",
        workerId: current.workerId,
      });
    }
    w = current;
  } catch (err: any) {
    return reply(400, {
      status: "error",
      errorKind: "spec_invalid",
      message: err?.message || String(err),
    });
  }

  w.requestCount += 1;
  const requestIndex = w.requestCount;
  const historyBefore = w.agent.messages.length;
  w.submitted.value = undefined;

  try {
    const limits = invokeLimits(w.spec.limits);
    const result = await w.agent.invoke(prompt, limits ? ({ limits } as any) : undefined);
    const metrics: any = result.metrics;
    const invocations = metrics?.agentInvocations;
    const usage = Array.isArray(invocations) && invocations.length
      ? invocations[invocations.length - 1]?.usage
      : metrics?.accumulatedUsage;

    return reply(200, {
      status: "ok",
      result: result.toString(),
      stopReason: result.stopReason || "endTurn",
      usage: usage ?? null,
      // Counts only; no conversation content beyond the answer itself.
      workerId: w.workerId,
      requestIndex,
      messageCount: w.agent.messages.length,
      tools: w.toolNames,
      submitted: w.submitted.value ?? null,
    });
  } catch (err: any) {
    // Roll back this turn's partial history so the conversation still alternates
    // user/assistant; Bedrock rejects two user messages in a row.
    if (w.agent.messages.length > historyBefore) {
      w.agent.messages.splice(historyBefore);
    }
    return reply(502, {
      status: "error",
      errorKind: "turn_failed",
      message: err?.message || String(err),
      workerId: w.workerId,
      requestIndex,
      messageCount: w.agent.messages.length,
    });
  }
}

// Implements `taffee:agent/worker@0.1.0`. componentize-js runs the event loop to resolution
// for async exports, so outbound fetch works inside a call and the export is re-entrant
// across calls on the same instance.
export const worker = {
  runTurn,
};
