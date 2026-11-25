#!/usr/bin/env python3
"""
mock-bedrock/test_server.py
Unit tests for mock Bedrock Converse server using standard library urllib.
"""

import json
import threading
import time
import unittest
import urllib.request
from server import run, ThreadingHTTPServer, BedrockConverseHandler


class TestMockBedrockServer(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.host = "127.0.0.1"
        cls.port = 8899
        BedrockConverseHandler.latency_ms = 0
        cls.httpd = ThreadingHTTPServer((cls.host, cls.port), BedrockConverseHandler)
        cls.httpd.daemon_threads = True
        cls.thread = threading.Thread(target=cls.httpd.serve_forever, daemon=True)
        cls.thread.start()
        time.sleep(0.1)

    @classmethod
    def tearDownClass(cls):
        cls.httpd.shutdown()
        cls.httpd.server_close()

    def test_health_ping(self):
        url = f"http://{self.host}:{self.port}/ping"
        req = urllib.request.Request(url, method="GET")
        with urllib.request.urlopen(req) as resp:
            self.assertEqual(resp.status, 200)
            data = json.loads(resp.read().decode())
            self.assertEqual(data.get("status"), "ok")

    def test_single_turn_converse(self):
        url = f"http://{self.host}:{self.port}/model/anthropic.claude-3-sonnet/converse"
        payload = {
            "messages": [
                {"role": "user", "content": [{"text": "Hello world"}]}
            ]
        }
        req = urllib.request.Request(
            url,
            data=json.dumps(payload).encode("utf-8"),
            headers={"Content-Type": "application/json"},
            method="POST"
        )
        with urllib.request.urlopen(req) as resp:
            self.assertEqual(resp.status, 200)
            data = json.loads(resp.read().decode())
            self.assertEqual(data.get("stopReason"), "end_turn")
            content = data["output"]["message"]["content"][0]
            self.assertIn("Hello world", content["text"])

    def test_tool_use_and_multi_turn(self):
        url = f"http://{self.host}:{self.port}/model/anthropic.claude-3-sonnet/converse"
        
        # Turn 1: User asks for calculation with calculator tool available
        payload1 = {
            "messages": [
                {"role": "user", "content": [{"text": "Please calculate 21 * 2 using calculator"}]}
            ],
            "toolConfig": {
                "tools": [
                    {
                        "toolSpec": {
                            "name": "calculator",
                            "description": "Calculates math expression",
                            "inputSchema": {"json": {"type": "object"}}
                        }
                    }
                ]
            }
        }
        req1 = urllib.request.Request(
            url,
            data=json.dumps(payload1).encode("utf-8"),
            headers={"Content-Type": "application/json"},
            method="POST"
        )
        with urllib.request.urlopen(req1) as resp1:
            self.assertEqual(resp1.status, 200)
            data1 = json.loads(resp1.read().decode())
            self.assertEqual(data1.get("stopReason"), "tool_use")
            tool_use = data1["output"]["message"]["content"][0]["toolUse"]
            self.assertEqual(tool_use["name"], "calculator")
            self.assertEqual(tool_use["input"]["expression"], "21 * 2")
            tool_use_id = tool_use["toolUseId"]

        # Turn 2: Agent executes tool and returns toolResult
        payload2 = {
            "messages": [
                {"role": "user", "content": [{"text": "Please calculate 21 * 2 using calculator"}]},
                {"role": "assistant", "content": [{"toolUse": tool_use}]},
                {
                    "role": "user",
                    "content": [
                        {
                            "toolResult": {
                                "toolUseId": tool_use_id,
                                "content": [{"text": "42"}],
                                "status": "success"
                            }
                        }
                    ]
                }
            ],
            "toolConfig": payload1["toolConfig"]
        }
        req2 = urllib.request.Request(
            url,
            data=json.dumps(payload2).encode("utf-8"),
            headers={"Content-Type": "application/json"},
            method="POST"
        )
        with urllib.request.urlopen(req2) as resp2:
            self.assertEqual(resp2.status, 200)
            data2 = json.loads(resp2.read().decode())
            self.assertEqual(data2.get("stopReason"), "end_turn")
            content = data2["output"]["message"]["content"][0]["text"]
            self.assertIn("42", content)

    def test_concurrent_requests(self):
        # Reset stats
        reset_req = urllib.request.Request(f"http://{self.host}:{self.port}/reset_stats", data=b"", method="POST")
        with urllib.request.urlopen(reset_req) as resp:
            self.assertEqual(resp.status, 200)

        # Temporarily enable synthetic latency on the handler
        BedrockConverseHandler.latency_ms = 150
        num_requests = 5
        results = [None] * num_requests
        errors = []

        def worker(idx):
            try:
                url = f"http://{self.host}:{self.port}/model/anthropic.claude-3-sonnet/converse"
                payload = {"messages": [{"role": "user", "content": [{"text": f"Worker {idx}"}]}]}
                req = urllib.request.Request(
                    url,
                    data=json.dumps(payload).encode("utf-8"),
                    headers={"Content-Type": "application/json"},
                    method="POST"
                )
                with urllib.request.urlopen(req) as resp:
                    if resp.status == 200:
                        results[idx] = json.loads(resp.read().decode())
                    else:
                        errors.append(f"Worker {idx} got status {resp.status}")
            except Exception as e:
                errors.append(f"Worker {idx} error: {e}")

        threads = [threading.Thread(target=worker, args=(i,)) for i in range(num_requests)]
        start_time = time.time()
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        elapsed = time.time() - start_time

        # Reset latency back to 0
        BedrockConverseHandler.latency_ms = 0

        self.assertEqual(len(errors), 0, f"Encountered errors: {errors}")
        self.assertEqual(len([r for r in results if r is not None]), num_requests)

        # Verify simultaneous overlapping handlers from server stats
        stats_req = urllib.request.Request(f"http://{self.host}:{self.port}/stats", method="GET")
        with urllib.request.urlopen(stats_req) as resp:
            self.assertEqual(resp.status, 200)
            stats = json.loads(resp.read().decode())
            max_active = stats.get("max_active_requests", 0)
            self.assertGreaterEqual(
                max_active,
                2,
                f"Expected at least 2 peak simultaneous active handlers, observed {max_active}"
            )

        # Scheduling margin assertion: concurrent execution finishes faster than serial (5 * 150ms = 750ms)
        self.assertLess(elapsed, 1.20, f"Expected concurrent execution within margin, took {elapsed:.3f}s")

    def _post(self, path, payload, headers=None):
        req = urllib.request.Request(
            f"http://{self.host}:{self.port}{path}",
            data=json.dumps(payload).encode(),
            headers={"Content-Type": "application/json", **(headers or {})},
            method="POST",
        )
        return urllib.request.urlopen(req)

    def test_chat_completions_tool_round_trip(self):
        # OpenAI-style chat: the same calculator script as Converse, in chat shapes.
        tools = [{"type": "function", "function": {"name": "calculator", "description": "math",
                                                   "parameters": {"type": "object"}}}]
        first = [{"role": "system", "content": "be brief"},
                 {"role": "user", "content": "Please calculate 21 * 2 using calculator"}]
        with self._post("/v1/chat/completions", {"model": "m", "messages": first, "tools": tools}) as r:
            d = json.loads(r.read())
        choice = d["choices"][0]
        self.assertEqual(choice["finish_reason"], "tool_calls")
        call = choice["message"]["tool_calls"][0]
        self.assertEqual(call["function"]["name"], "calculator")
        self.assertIsInstance(json.loads(call["function"]["arguments"]), dict)
        second = first + [choice["message"],
                          {"role": "tool", "tool_call_id": call["id"], "content": "42"}]
        with self._post("/v1/chat/completions", {"model": "m", "messages": second, "tools": tools}) as r:
            d = json.loads(r.read())
        self.assertEqual(d["choices"][0]["finish_reason"], "stop")
        self.assertIn("42", d["choices"][0]["message"]["content"])
        self.assertGreater(d["usage"]["total_tokens"], 0)

    def test_mcp_initialize_list_call_over_sse(self):
        with self._post("/mcp", {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}) as resp:
            self.assertEqual(resp.headers.get("Mcp-Session-Id"), "mock-session-1")
            self.assertIn("text/event-stream", resp.headers.get("Content-Type"))
            frame = resp.read().decode()
            data = json.loads(frame.split("data:", 1)[1].strip())
            self.assertEqual(data["id"], 1)
        with self._post("/mcp", {"jsonrpc": "2.0", "id": 2, "method": "tools/list"}) as resp:
            data = json.loads(resp.read().decode().split("data:", 1)[1].strip())
            self.assertEqual(data["result"]["tools"][0]["name"], "search_code")
        with self._post("/mcp", {"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                                 "params": {"name": "search_code", "arguments": {"query": "auth"}}}) as resp:
            data = json.loads(resp.read().decode().split("data:", 1)[1].strip())
            self.assertIn("auth", data["result"]["content"][0]["text"])

    def test_converse_records_authorization_header(self):
        self._post("/reset_stats", {}).close()
        body = {"messages": [{"role": "user", "content": [{"text": "hi"}]}]}
        self._post("/model/m/converse", body, {"Authorization": "AWS4-HMAC-SHA256 Credential=AKIDHOST/x"}).close()
        with urllib.request.urlopen(f"http://{self.host}:{self.port}/requests") as resp:
            trace = json.loads(resp.read().decode())
        self.assertEqual(trace["requests"][-1]["authorization"], "AWS4-HMAC-SHA256 Credential=AKIDHOST/x")

    def test_loop_forever_keeps_requesting_tools(self):
        tools = {"tools": [{"toolSpec": {"name": "calculator"}}]}
        msgs = [
            {"role": "user", "content": [{"text": "loop forever please"}]},
            {"role": "assistant", "content": [{"toolUse": {"toolUseId": "t1", "name": "calculator", "input": {}}}]},
            {"role": "user", "content": [{"toolResult": {"toolUseId": "t1", "content": [{"text": "2"}]}}]},
        ]
        with self._post("/model/m/converse", {"messages": msgs, "toolConfig": tools}) as resp:
            self.assertEqual(json.loads(resp.read().decode())["stopReason"], "tool_use")


if __name__ == "__main__":
    unittest.main()
