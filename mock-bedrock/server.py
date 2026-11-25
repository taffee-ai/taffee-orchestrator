#!/usr/bin/env python3
"""
mock-bedrock/server.py
Lightweight HTTP mock server mimicking the AWS Bedrock Converse API.
Implements POST /model/{modelId}/converse with support for:
- Standard single-turn text responses (stopReason: "end_turn")
- Tool invocation turns (stopReason: "tool_use")
- Handling of toolResult messages to complete multi-turn interactions
- Configurable synthetic latency
- Health check endpoint (GET /ping)
"""

import argparse
import json
import re
import sys
import time
import threading
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler


def chat_to_converse(chat):
    """OpenAI-style chat request -> Bedrock Converse request (system, messages, toolConfig)."""
    system, messages = [], []

    def user_msg():
        if not messages or messages[-1]["role"] != "user":
            messages.append({"role": "user", "content": []})
        return messages[-1]

    for m in chat.get("messages", []):
        role, content = m.get("role"), m.get("content")
        text = content if isinstance(content, str) else " ".join(
            p.get("text", "") for p in (content or []) if isinstance(p, dict))
        if role == "system":
            system.append({"text": text})
        elif role == "user":
            user_msg()["content"].append({"text": text})
        elif role == "tool":
            user_msg()["content"].append({"toolResult": {
                "toolUseId": m.get("tool_call_id"), "status": "success", "content": [{"text": text}]}})
        elif role == "assistant":
            blocks = [{"text": text}] if text else []
            for tc in m.get("tool_calls") or []:
                fn = tc.get("function", {})
                try:
                    args = json.loads(fn.get("arguments") or "{}")
                except ValueError:
                    args = {}
                blocks.append({"toolUse": {"toolUseId": tc.get("id"), "name": fn.get("name"), "input": args}})
            messages.append({"role": "assistant", "content": blocks})
    tools = [{"toolSpec": {"name": t["function"]["name"], "description": t["function"].get("description", ""),
                           "inputSchema": {"json": t["function"].get("parameters", {})}}}
             for t in chat.get("tools", []) if t.get("type") == "function"]
    return {"system": system, "messages": messages, "toolConfig": {"tools": tools}}


def converse_to_chat(resp, model_id):
    """Bedrock Converse response -> OpenAI-style chat completion."""
    blocks = resp.get("output", {}).get("message", {}).get("content", [])
    text = "".join(b.get("text", "") for b in blocks if "text" in b)
    calls = [{"id": b["toolUse"]["toolUseId"], "type": "function",
              "function": {"name": b["toolUse"]["name"], "arguments": json.dumps(b["toolUse"].get("input", {}))}}
             for b in blocks if "toolUse" in b]
    message = {"role": "assistant", "content": text or None}
    if calls:
        message["tool_calls"] = calls
    u = resp.get("usage", {})
    return {
        "id": "chatcmpl-mock", "object": "chat.completion", "model": model_id,
        "choices": [{"index": 0, "message": message,
                     "finish_reason": "tool_calls" if calls else ("length" if resp.get("stopReason") == "max_tokens" else "stop")}],
        "usage": {"prompt_tokens": u.get("inputTokens", 0), "completion_tokens": u.get("outputTokens", 0),
                  "total_tokens": u.get("totalTokens", 0)},
    }


class BedrockConverseHandler(BaseHTTPRequestHandler):
    latency_ms = 0
    active_requests = 0
    max_active_requests = 0
    request_history = []
    mcp_history = []
    mcp_token = None
    # A repository the "worker" role reads: {"files": {path: {"lines": n, "text": {"<n>": line}}}}.
    fixture = None
    lock = threading.Lock()

    def log_message(self, format, *args):
        # Override to keep logs clean during high concurrency testing
        sys.stderr.write(f"[{self.log_date_time_string()}] {self.address_string()} {format % args}\n")

    def do_GET(self):
        if self.path == "/page":  # a small HTML page for the http_request tool test
            page = (b"<html><head><style>h1{color:red}</style><script>var x=1;</script></head><body>"
                    b"<h1>Hello page</h1><p>Read <a href=\"https://example.com/more\">more here</a>.</p></body></html>")
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(page)))
            self.end_headers()
            self.wfile.write(page)
            return
        if self.path in ("/ping", "/health"):
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(b'{"status":"ok"}')
            return

        if self.path == "/stats":
            with self.lock:
                stats = {
                    "active_requests": BedrockConverseHandler.active_requests,
                    "max_active_requests": BedrockConverseHandler.max_active_requests
                }
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(stats).encode("utf-8"))
            return

        if self.path == "/requests":
            with self.lock:
                history_copy = list(BedrockConverseHandler.request_history)
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps({"count": len(history_copy), "requests": history_copy}).encode("utf-8"))
            return

        if self.path == "/mcp_requests":
            with self.lock:
                mcp_copy = list(BedrockConverseHandler.mcp_history)
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps({"count": len(mcp_copy), "requests": mcp_copy}).encode("utf-8"))
            return

        self.send_response(404)
        self.end_headers()

    def do_POST(self):
        if self.path in ("/reset_stats", "/reset"):
            with self.lock:
                BedrockConverseHandler.active_requests = 0
                BedrockConverseHandler.max_active_requests = 0
                BedrockConverseHandler.request_history.clear()
                BedrockConverseHandler.mcp_history.clear()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(b'{"status":"reset"}')
            return

        if self.path == "/mcp":
            self._handle_mcp()
            return

        if self.path == "/fixture":
            length = int(self.headers.get("Content-Length", 0))
            BedrockConverseHandler.fixture = json.loads(self.rfile.read(length).decode("utf-8"))
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(b'{"status":"fixture loaded"}')
            return

        # Track active concurrency
        with self.lock:
            BedrockConverseHandler.active_requests += 1
            if BedrockConverseHandler.active_requests > BedrockConverseHandler.max_active_requests:
                BedrockConverseHandler.max_active_requests = BedrockConverseHandler.active_requests

        try:
            # Match /model/{modelId}/converse (Bedrock) or .../chat/completions (OpenAI-style chat).
            # A chat request is translated to a Converse body, answered by the same logic, and
            # translated back, so every scripted behaviour works for both formats.
            match = re.match(r"^/model/([^/]+)/converse", self.path)
            chat = self.path.rstrip("/").endswith("/chat/completions")
            if not match and not chat:
                self.send_response(404)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps({"message": f"Resource not found: {self.path}"}).encode("utf-8"))
                return

            content_length = int(self.headers.get("Content-Length", 0))
            raw_body = self.rfile.read(content_length)

            try:
                body = json.loads(raw_body.decode("utf-8"))
            except Exception as e:
                self.send_response(400)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps({"message": f"Invalid JSON body: {str(e)}"}).encode("utf-8"))
                return
            if chat:
                model_id = body.get("model") or "chat-model"
                body = chat_to_converse(body)
            else:
                model_id = match.group(1)
            with self.lock:
                BedrockConverseHandler.request_history.append({
                    "modelId": model_id,
                    "messages": body.get("messages", []),
                    "toolConfig": body.get("toolConfig"),
                    "system": body.get("system"),
                    "authorization": self.headers.get("Authorization"),
                    "amzDate": self.headers.get("X-Amz-Date"),
                })

            # Simulate synthetic latency if configured
            if self.latency_ms > 0:
                time.sleep(self.latency_ms / 1000.0)
            # Prompt keyword "slowpoke" holds the reply (used to test turn deadlines).
            if "slowpoke" in json.dumps(body.get("messages", [])[-1:]):
                time.sleep(8)

            # Prompt keyword "explode" makes the model fail (used to test turn rollback).
            if "explode" in json.dumps(body.get("messages", [])[-1:]):
                err = json.dumps({"message": "mock model failure"}).encode("utf-8")
                self.send_response(500)
                self.send_header("Content-Type", "application/json")
                self.send_header("x-amzn-ErrorType", "InternalServerException")
                self.send_header("Content-Length", str(len(err)))
                self.end_headers()
                self.wfile.write(err)
                return

            response_data = self._generate_converse_response(model_id, body)
            if chat:
                response_data = converse_to_chat(response_data, model_id)
            resp_bytes = json.dumps(response_data).encode("utf-8")

            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(resp_bytes)))
            self.end_headers()
            self.wfile.write(resp_bytes)
        finally:
            with BedrockConverseHandler.lock:
                BedrockConverseHandler.active_requests -= 1

    def _generate_converse_response(self, model_id, body):
        messages = body.get("messages", [])
        tool_config = body.get("toolConfig", {})
        available_tools = tool_config.get("tools", [])

        if not messages:
            return {
                "output": {
                    "message": {
                        "role": "assistant",
                        "content": [{"text": "Hello! I am mock-bedrock."}]
                    }
                },
                "stopReason": "end_turn",
                "usage": {"inputTokens": 5, "outputTokens": 10, "totalTokens": 15},
                "metrics": {"latencyMs": self.latency_ms}
            }

        last_message = messages[-1]
        last_role = last_message.get("role", "")
        last_content = last_message.get("content", [])

        # Check if the last message contains a toolResult
        has_tool_result = any("toolResult" in block for block in last_content)
        # Text of the latest user prompt (not a toolResult message).
        last_user_text = next(
            (" ".join(b["text"] for b in m.get("content", []) if "text" in b)
             for m in reversed(messages)
             if m.get("role") == "user" and any("text" in b for b in m.get("content", []))),
            "",
        ).lower()
        # Work-manifest roles (runner/src/work/agents.rs): answer through the submit tool.
        system_text = " ".join(b.get("text", "") for b in (body.get("system") or []) if isinstance(b, dict))
        role = re.search(r"TAFFEE-ROLE: (\w+)", system_text)
        if role and not has_tool_result:
            raw_user = next(
                (" ".join(b["text"] for b in m.get("content", []) if "text" in b)
                 for m in reversed(messages) if m.get("role") == "user" and any("text" in b for b in m.get("content", []))),
                "",
            )
            submitted = self._role(role.group(1), system_text, raw_user)
            return {
                "output": {"message": {"role": "assistant", "content": [
                    {"toolUse": {"toolUseId": f"tooluse_submit_{len(messages)}", "name": "submit", "input": submitted}}]}},
                "stopReason": "tool_use",
                "usage": {"inputTokens": 400, "outputTokens": 100, "totalTokens": 500},
                "metrics": {"latencyMs": self.latency_ms}
            }

        if has_tool_result and "loop forever" in last_user_text:
            return {
                "output": {"message": {"role": "assistant", "content": [
                    {"toolUse": {"toolUseId": f"tooluse_loop_{len(messages)}", "name": "calculator", "input": {"expression": "1 + 1"}}}]}},
                "stopReason": "tool_use",
                "usage": {"inputTokens": 20, "outputTokens": 15, "totalTokens": 35},
                "metrics": {"latencyMs": self.latency_ms}
            }
        if has_tool_result:
            # Multi-turn continuation after tool execution
            result_text = "completed"
            tool_name = None
            if len(messages) >= 2:
                for block in messages[-2].get("content", []):
                    if "toolUse" in block:
                        tool_name = block["toolUse"].get("name")
            for block in last_content:
                if "toolResult" in block:
                    tr = block["toolResult"]
                    content = tr.get("content", [])
                    for c in content:
                        if "text" in c:
                            result_text = c["text"]
                        elif "json" in c:
                            result_text = json.dumps(c["json"])
                    if tr.get("status") == "error":
                        result_text = "error: " + result_text
            if tool_name and tool_name != "calculator":
                reply = f"Tool {tool_name} returned: {result_text}"
            else:
                reply = f"The calculation result is {result_text}."

            return {
                "output": {
                    "message": {
                        "role": "assistant",
                        "content": [
                            {"text": reply}
                        ]
                    }
                },
                "stopReason": "end_turn",
                "usage": {"inputTokens": 45, "outputTokens": 15, "totalTokens": 60},
                "metrics": {"latencyMs": self.latency_ms}
            }

        # Check user prompt text
        prompt_text = ""
        for block in last_content:
            if "text" in block:
                prompt_text += block["text"] + " "

        prompt_lower = prompt_text.lower()

        # If tools are available and user asks for math/calculator/weather
        calculator_tool = next((t.get("toolSpec", {}).get("name") for t in available_tools if t.get("toolSpec", {}).get("name") == "calculator"), None)

        if calculator_tool and any(w in prompt_lower for w in ["calculate", "calculator", "*", "+", "-", "/", "math", "21 * 2"]):
            # Extract expression if present, default to "21 * 2"
            math_match = re.search(r"(\d+\s*[\+\-\*\/]\s*\d+)", prompt_text)
            expr = math_match.group(1) if math_match else "21 * 2"
            return {
                "output": {
                    "message": {
                        "role": "assistant",
                        "content": [
                            {
                                "toolUse": {
                                    "toolUseId": "tooluse_calc_1001",
                                    "name": "calculator",
                                    "input": {
                                        "expression": expr
                                    }
                                }
                            }
                        ]
                    }
                },
                "stopReason": "tool_use",
                "usage": {"inputTokens": 20, "outputTokens": 15, "totalTokens": 35},
                "metrics": {"latencyMs": self.latency_ms}
            }

        tool_names = [t.get("toolSpec", {}).get("name") for t in available_tools]
        # "fetchpage" asks for the http_request tool on the mock's own /page.
        if "http_request" in tool_names and "fetchpage" in prompt_lower:
            return {
                "output": {"message": {"role": "assistant", "content": [{"toolUse": {
                    "toolUseId": "tooluse_http_3001", "name": "http_request",
                    "input": {"url": "http://127.0.0.1:8000/page"}}}]}},
                "stopReason": "tool_use",
                "usage": {"inputTokens": 20, "outputTokens": 15, "totalTokens": 35},
                "metrics": {"latencyMs": self.latency_ms}
            }
        if "search_code" in tool_names and "search" in prompt_lower:
            query = prompt_text.lower().split("search", 1)[1].strip(" :?.") or "anything"
            return {
                "output": {
                    "message": {
                        "role": "assistant",
                        "content": [{"toolUse": {"toolUseId": "tooluse_mcp_2001", "name": "search_code", "input": {"query": query}}}]
                    }
                },
                "stopReason": "tool_use",
                "usage": {"inputTokens": 20, "outputTokens": 15, "totalTokens": 35},
                "metrics": {"latencyMs": self.latency_ms}
            }

        # "loop forever" keeps asking for the calculator (used to test the SDK turn limit).
        if calculator_tool and "loop forever" in prompt_lower:
            return {
                "output": {"message": {"role": "assistant", "content": [
                    {"toolUse": {"toolUseId": f"tooluse_loop_{len(messages)}", "name": "calculator", "input": {"expression": "1 + 1"}}}]}},
                "stopReason": "tool_use",
                "usage": {"inputTokens": 20, "outputTokens": 15, "totalTokens": 35},
                "metrics": {"latencyMs": self.latency_ms}
            }

        # Default standard turn
        return {
            "output": {
                "message": {
                    "role": "assistant",
                    "content": [
                        {"text": f"Echo from mock Bedrock ({model_id}): {prompt_text.strip()}"}
                    ]
                }
            },
            "stopReason": "end_turn",
            "usage": {"inputTokens": len(prompt_text.split()), "outputTokens": 12, "totalTokens": len(prompt_text.split()) + 12},
            "metrics": {"latencyMs": self.latency_ms}
        }


    # ---- Work-manifest roles. Deterministic stand-ins for a model doing each job. ----

    @staticmethod
    def _section(text, title):
        """Lines "- x" under "<title>:" until the next blank or unindented line."""
        out, inside = [], False
        for line in text.split("\n"):
            if line.strip() == f"{title}:":
                inside = True
                continue
            if inside:
                if line.startswith("- "):
                    out.append(line[2:].strip())
                else:
                    break
        return out

    def _role(self, role, system, user):
        if role == "normalizer":
            kind = re.search(r"- (code\.impact_analysis@\d+):", system)
            m = re.search(r"rename (\w+) to (\w+)(?: in ([\w./-]+))?", user, re.I)
            if m:
                symbol, rename_to, repo = m.group(1), m.group(2), m.group(3)
            else:
                m = re.search(r"uses (\w+) in ([\w./-]+).*?call it (\w+)", user, re.I)
                if not m or not kind:
                    # Anything else goes to the general kinds when the registry has them: doing
                    # words to task.act, everything else to task.general.
                    gen = re.search(r"- (task\.general@\d+):", system)
                    act = re.search(r"- (task\.act@\d+):", system)
                    if act and re.search(r"\b(book|buy|order|purchase|pay|send|post|reserve|deploy|delete)\b", user, re.I):
                        return {"kind": act.group(1), "parameters": {"task": user}, "given": {}, "reason": "acts in the world"}
                    if gen and user.strip():
                        return {"kind": gen.group(1), "parameters": {"task": user}, "given": {}, "reason": "finds things out"}
                    return {"kind": "none"}
                symbol, repo, rename_to = m.group(1), m.group(2), m.group(3)
            out = {"kind": kind.group(1) if kind else "none",
                   "parameters": {"symbol": symbol, "rename_to": rename_to},
                   "given": {"repo": repo} if repo else {},
                   "reason": "what breaks" if "break" in user.lower() else "what uses"}
            return out

        if role == "planner":
            files = self._section(system, "Files")
            usd = float(re.search(r"usd ([\d.]+)", system).group(1))
            secs = int(re.search(r"time (\d+)s", system).group(1))
            groups = []
            for f in files:
                g = f.split("/")[0] + "/**" if "/" in f else f
                if g not in groups:
                    groups.append(g)
            share = usd if "TEST-OVERSPEND" in system else int(usd * 0.9 / max(len(groups), 1) * 100) / 100.0
            if "TEST-PARTIAL-PLAN" in system:  # leave one group out, as a real model did (2026-11-18)
                groups = groups[:-1]
            if "TEST-USE-ALL" in system:  # spend the whole offer, as a real model did (2026-11-17)
                share = usd / max(len(groups), 1)
            # Real models often repeat the parent's other parameters unchanged (TEST-COPY-PARAMS);
            # TEST-CHANGE-SYMBOL makes a worker change one, which must be refused.
            parent = json.loads(re.search(r"Parameters: (\{.*\})", system).group(1))
            def params(g):
                p = {"paths": [g]}
                if "TEST-COPY-PARAMS" in system or "TEST-CHANGE-SYMBOL" in system:
                    p = {**parent, "paths": [g]}
                if "TEST-CHANGE-SYMBOL" in system:
                    p["symbol"] = "SomethingElse"
                return p
            return {"workers": [{"parameters": params(g), "usd": share, "time": f"{secs}s"} for g in groups]}

        if role == "worker":
            general = re.search(r"Parameters: (\{.*\})", system)
            params = json.loads(general.group(1)) if general else {}
            if "task" in params:  # the general kinds: a short answer with a source
                answer = {"answer": f"Mock answer for: {params['task']}",
                          "sources": [{"url": "https://example.com/mock", "title": "Mock source"}]}
                if "Every action taken" in system:
                    answer["actions"] = []
                return {"answer": answer, "searched": []}
            params = json.loads(re.search(r"Parameters: (\{.*\})", system).group(1))
            sym, new = params.get("symbol", ""), params.get("rename_to", "")
            files = self._section(system, "Files to search")
            fx = (BedrockConverseHandler.fixture or {}).get("files", {})
            word = re.compile(rf"(?<![\w$]){re.escape(sym)}(?![\w$])")
            breaks, mentions, proposals = [], [], []
            for f in files:
                for n, text in sorted(((int(k), v) for k, v in fx.get(f, {}).get("text", {}).items())):
                    if not word.search(text):
                        continue
                    if f.endswith(".md"):
                        mentions.append({"file": f, "line": n, "where": "doc"})
                        proposals.append({"manifest": "0.1", "want": f"Update {f} to say {new}", "why": f"Line {n} names the class"})
                    elif re.search(rf"['\"]{re.escape(sym)}['\"]", text):
                        mentions.append({"file": f, "line": n, "where": "string"})
                    elif text.strip().startswith("import"):
                        breaks.append({"file": f, "line": n, "reason": "import"})
                    elif re.search(rf"\b(class|interface)\s+{re.escape(sym)}\b", text):
                        breaks.append({"file": f, "line": n, "reason": "declaration"})
                    elif re.search(rf":\s*{re.escape(sym)}\b", text):
                        breaks.append({"file": f, "line": n, "reason": "type"})
                    else:
                        breaks.append({"file": f, "line": n, "reason": "call"})
            if "TEST-HALLUCINATE" in system and files:
                breaks.append({"file": files[0], "line": 9999, "reason": "call"})
            if "TEST-SKIP-SEARCHED" in system:  # leave a file out of `searched`, as a real model did
                files = files[:-1]
            return {"answer": {"breaks": breaks, "mentions": mentions}, "searched": files, "proposals": proposals}

        return {}

    # ---- Minimal MCP (Streamable HTTP) server with one tool, for orchestrator tests ----
    MCP_TOOLS = [{
        "name": "search_code",
        "description": "Search the indexed code base",
        "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]},
    }]

    def _handle_mcp(self):
        length = int(self.headers.get("Content-Length", 0))
        try:
            msg = json.loads(self.rfile.read(length).decode("utf-8"))
        except Exception:
            self.send_response(400)
            self.end_headers()
            return
        with self.lock:
            BedrockConverseHandler.mcp_history.append({
                "method": msg.get("method"),
                "authorization": self.headers.get("Authorization"),
                "session": self.headers.get("Mcp-Session-Id"),
            })
        if BedrockConverseHandler.mcp_token and self.headers.get("Authorization") != "Bearer " + BedrockConverseHandler.mcp_token:
            self.send_response(401)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(b'{"error":"unauthorized"}')
            return
        if "id" not in msg:  # notification
            self.send_response(202)
            self.end_headers()
            return
        method = msg.get("method")
        if method == "initialize":
            result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "mock-mcp", "version": "0.1"}}
        elif method == "tools/list":
            result = {"tools": self.MCP_TOOLS}
        elif method == "tools/call":
            q = (msg.get("params", {}).get("arguments") or {}).get("query", "")
            result = {"content": [{"type": "text", "text": f"3 matches for '{q}' in permissions.py"}], "isError": False}
        else:
            result = None
        payload = {"jsonrpc": "2.0", "id": msg["id"]}
        if result is None:
            payload["error"] = {"code": -32601, "message": f"method not found: {method}"}
        else:
            payload["result"] = result
        data = json.dumps(payload)
        # Answer as SSE, the way FastMCP's streamable HTTP transport does.
        body = f"event: message\ndata: {data}\n\n".encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Mcp-Session-Id", "mock-session-1")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def run(host="127.0.0.1", port=8000, latency_ms=0, mcp_token=None):
    BedrockConverseHandler.latency_ms = latency_ms
    BedrockConverseHandler.mcp_token = mcp_token
    server_address = (host, port)
    ThreadingHTTPServer.request_queue_size = 512
    httpd = ThreadingHTTPServer(server_address, BedrockConverseHandler)
    httpd.daemon_threads = True
    print(f"Mock Bedrock Converse server running on http://{host}:{port} (latency={latency_ms}ms, threading=enabled)")
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        print("\nShutting down mock server.")
        httpd.server_close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="AWS Bedrock Converse Mock Server")
    parser.add_argument("--host", default="127.0.0.1", help="Bind host (default: 127.0.0.1)")
    parser.add_argument("--port", type=int, default=8000, help="Bind port (default: 8000)")
    parser.add_argument("--latency-ms", type=int, default=0, help="Synthetic latency in ms (default: 0)")
    parser.add_argument("--mcp-token", default=None, help="Require this Bearer token on /mcp")
    args = parser.parse_args()

    run(host=args.host, port=args.port, latency_ms=args.latency_ms, mcp_token=args.mcp_token)
