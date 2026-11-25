#!/usr/bin/env bash
# Smoke-test the runtime image the way the stack would run it. Runs ON THE TEST BOX, after
# scripts/ci.sh produced dist/orchestratord. Everything runs in containers on a private
# network; nothing is published on host ports. Cleans up after itself.
set -euo pipefail
cd "$(dirname "$0")/.."
EVID=${EVIDENCE_DIR:-docs/evidence/$(date -u +%Y-%m-%d)${EVIDENCE_SUFFIX:-}}
mkdir -p "$EVID"
NET=taffee-orch-smoke
TOKEN=smoke-token-0123456789abcdef
cleanup() { docker rm -f taffee-orch-smoke-mock taffee-orch-smoke-orch >/dev/null 2>&1 || true; docker network rm $NET >/dev/null 2>&1 || true; rm -rf /tmp/taffee-orch-smoke; }
trap cleanup EXIT
cleanup

docker build -t taffee-orchestrator:dev -f docker/Dockerfile . | tail -2
docker image ls taffee-orchestrator:dev --format 'image size: {{.Size}}'
mkdir -p /tmp/taffee-orch-smoke && echo "smoke $TOKEN" > /tmp/taffee-orch-smoke/tokens && chmod 644 /tmp/taffee-orch-smoke/tokens

docker network create $NET >/dev/null
docker run -d --name taffee-orch-smoke-mock --network $NET -e PYTHONDONTWRITEBYTECODE=1 \
  -v "$PWD/mock-bedrock":/mock:ro -w /mock python:3.12-slim python3 server.py --host 0.0.0.0 --port 8000 >/dev/null
docker run -d --name taffee-orch-smoke-orch --network $NET \
  -e TAFFEE_ORCH_MODEL_ENDPOINT=http://taffee-orch-smoke-mock:8000 \
  -e TAFFEE_ORCH_API_TOKENS_FILE=/run/orch-tokens -e TAFFEE_ORCH_MAX_AGENTS=20 \
  -v /tmp/taffee-orch-smoke/tokens:/run/orch-tokens:ro \
  --memory 1g taffee-orchestrator:dev >/dev/null

client() { docker run --rm --network $NET taffee-orch-builder:dev "$@"; }
client bash -c 'for i in $(seq 1 120); do curl -sf http://taffee-orch-smoke-orch:8780/healthz && exit 0; sleep 1; done; exit 1'
echo
client bash -c "
set -e
B=http://taffee-orch-smoke-orch:8780; A='Authorization: Bearer $TOKEN'
echo '--- unauthenticated:'; curl -s -o /dev/null -w '%{http_code}\n' \$B/v1/agents
echo '--- create:'; ID=\$(curl -sf -H \"\$A\" -H 'content-type: application/json' -d '{\"intent\":{\"objective\":\"smoke\"}}' \$B/v1/agents | python3 -c 'import json,sys;print(json.load(sys.stdin)[\"agent_id\"])'); echo \$ID
echo '--- turn 1:'; curl -sf -H \"\$A\" -H 'content-type: application/json' -d '{\"prompt\":\"Calculate 21 * 2\"}' \$B/v1/agents/\$ID/turns | python3 -c 'import json,sys;r=json.load(sys.stdin);print(r[\"outcome\"],r[\"result\"],r[\"message_count\"],len(r[\"calls\"]),\"calls\");assert r[\"outcome\"]==\"ok\" and \"42\" in r[\"result\"]'
echo '--- turn 2:'; curl -sf -H \"\$A\" -H 'content-type: application/json' -d '{\"prompt\":\"and again\"}' \$B/v1/agents/\$ID/turns | python3 -c 'import json,sys;r=json.load(sys.stdin);print(r[\"outcome\"],r[\"message_count\"]);assert r[\"message_count\"]==6'
echo '--- stats:'; curl -sf -H \"\$A\" \$B/v1/stats
echo; echo '--- delete:'; curl -s -o /dev/null -w '%{http_code}\n' -X DELETE -H \"\$A\" \$B/v1/agents/\$ID
" 2>&1 | tee "$EVID/image-smoke.log"
docker inspect --format 'health: {{.State.Health.Status}}' taffee-orch-smoke-orch | tee -a "$EVID/image-smoke.log"
docker logs taffee-orch-smoke-orch 2>&1 | tail -5 | tee -a "$EVID/image-smoke.log"
