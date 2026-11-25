#!/usr/bin/env bash
# One real work request against a live OpenAI-style chat model.
# Runs on the test machine after scripts/ci.sh has built dist/orchestratord and the guest.
# Everything runs inside the builder container; the key is read by the host process only.
#
#   KEY_FILE=~/.secrets/model.key scripts/live-work.sh
#   LIVE_MODEL=claude-haiku-5.5 LIVE_ENDPOINT=https://model-gateway.example/v1 scripts/live-work.sh
#   LIVE_WANT="If I rename PaymentService to PaymentGateway in payments-service, what breaks?" ...
set -euo pipefail
cd "$(dirname "$0")/.."
KEY_FILE=${KEY_FILE:-$HOME/.secrets/model.key}
LIVE_ENDPOINT=${LIVE_ENDPOINT:?set LIVE_ENDPOINT to your OpenAI-style gateway base URL, e.g. https://model-gateway.example/v1}
LIVE_MODEL=${LIVE_MODEL:-claude-haiku-5.5}
LIVE_WANT=${LIVE_WANT:-If I rename PaymentService to PaymentGateway in payments-service, what breaks? Need file and line.}
EVID=${EVIDENCE_DIR:-docs/evidence/$(date -u +%Y-%m-%d)${EVIDENCE_SUFFIX:-}}
[ -s "$KEY_FILE" ] || { echo "no key at $KEY_FILE" >&2; exit 2; }
[ -x dist/orchestratord ] && [ -f strand-agent/dist/strand-agent.wasm ] || { echo "run scripts/ci.sh first" >&2; exit 2; }
mkdir -p "$EVID"

docker run --rm --name taffee-orch-live \
  -e LIVE_ENDPOINT="$LIVE_ENDPOINT" -e LIVE_MODEL="$LIVE_MODEL" -e LIVE_WANT="$LIVE_WANT" -e EVID="$EVID" \
  -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
  -v "$PWD":/src -v "$KEY_FILE":/run/model.key:ro \
  taffee-orch-builder:dev bash -c '
set -euo pipefail
cd /src
SPEC=docs/spec/work-manifest
COMMIT=4f2c9e1b7a3d5c8e9f0a1b2c3d4e5f6a7b8c9d0e
ROLE=$(python3 -c "import json,os; print(json.dumps({\"model\": {\"provider\": \"openai-chat\", \"modelId\": os.environ[\"LIVE_MODEL\"], \"endpoint\": os.environ[\"LIVE_ENDPOINT\"], \"maxTokens\": 4096}}))")
HOST=$(python3 -c "import os,urllib.parse; u=urllib.parse.urlparse(os.environ[\"LIVE_ENDPOINT\"]); print(f\"{u.scheme}://{u.netloc}\")")
cat > /tmp/live.json <<JSON
{
  "turn_timeout_secs": 300,
  "default_spec": { "model": { "provider": "openai-chat", "modelId": "$LIVE_MODEL", "endpoint": "$LIVE_ENDPOINT" } },
  "credentials": [ { "kind": "bearer", "name": "live-model", "destination": "$HOST", "token_file": "/run/model.key" } ],
  "work": {
    "kinds_dir": "$SPEC/kinds",
    "principal": "orchestrator:live",
    "resolve": {
      "names": { "payments-service": ["github.com/acme/payments-service"] },
      "versions": { "github.com/acme/payments-service@main": "$COMMIT" }
    },
    "policy": { "principals": { "user:anonymous": { "max_effects": "read-only", "max_usd": 2.0, "max_time": "15m" } } },
    "inputs": { "github.com/acme/payments-service@$COMMIT": "$SPEC/fixtures/payments-service@4f2c9e1.yaml" },
    "roles": { "normalizer": $ROLE, "planner": $ROLE, "worker": $ROLE }
  }
}
JSON
TAFFEE_ORCH_CONFIG=/tmp/live.json TAFFEE_ORCH_COMPONENT=strand-agent/dist/strand-agent.wasm \
  TAFFEE_ORCH_LISTEN=127.0.0.1:8780 dist/orchestratord > /tmp/orch.log 2>&1 &
for i in $(seq 1 120); do curl -sf http://127.0.0.1:8780/healthz >/dev/null && break; sleep 1; done
python3 - <<PY
import json, os, time, urllib.request
req = {"manifest": "0.1", "want": os.environ["LIVE_WANT"]}
t = time.time()
r = urllib.request.Request("http://127.0.0.1:8780/v1/work", data=json.dumps(req).encode(),
                           headers={"content-type": "application/json"}, method="POST")
try:
    body = urllib.request.urlopen(r, timeout=900).read()
except urllib.error.HTTPError as e:
    body = e.read()
reply = json.loads(body)
reply["x-live"] = {"model": os.environ["LIVE_MODEL"], "endpoint": os.environ["LIVE_ENDPOINT"], "seconds": round(time.time() - t, 1)}
out = os.path.join(os.environ["EVID"], "live-work.json")
open(out, "w").write(json.dumps(reply, indent=2) + "\n")
print(json.dumps({k: reply.get(k) for k in ["status", "checks", "coverage", "usage", "failure", "reason", "trace", "x-live"]}, indent=2))
if reply.get("answer"):
    print(json.dumps(reply["answer"], indent=2))
PY
kill %1 2>/dev/null || true
grep -vi "authorization\|bearer" /tmp/orch.log | tail -20 > "$EVID/live-orchestratord.log" || true
chown -R "$HOST_UID:$HOST_GID" "$EVID"
'
