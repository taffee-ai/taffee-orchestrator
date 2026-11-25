#!/usr/bin/env bash
# Send a sentence to the orchestrator service on a test machine, as a caller would, and print
# the reply. Uses the caller API bearer token scripts/deploy-node.sh saved in ~/.taffee/<label>.token.
#
#   scripts/ask-node.sh "If I rename PaymentService to PaymentGateway in payments-service, what breaks?"
#   DRY_RUN=1 scripts/ask-node.sh "..."      the readback only: the frozen manifest, nothing runs
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
PRE=":"; LABEL=""; KEY=""; DEFAULT_BOX=""
. "$HERE/boxes.local.sh"
NAME=${TAFFEE_BOX:-node2}
"box_$NAME"
TOKEN_FILE=~/.taffee/"${LABEL:-$NAME}".token
[ -s "$TOKEN_FILE" ] || { echo "no API bearer token at $TOKEN_FILE; run scripts/deploy-node.sh first" >&2; exit 2; }
HOST=$(ssh -G "$BOX" | awk '$1 == "hostname" { print $2 }')
WANT=${1:?usage: scripts/ask-node.sh "<what you want>"}
BODY=$(WANT="$WANT" DRY="${DRY_RUN:-0}" python3 -c 'import json, os; b = {"manifest": "0.1", "want": os.environ["WANT"]}
if os.environ["DRY"] == "1": b["dry_run"] = True
print(json.dumps(b))')
OUT=$(mktemp)
trap 'rm -f "$OUT"' EXIT
CODE=$(curl -s -o "$OUT" -w '%{http_code}' --connect-timeout 10 --max-time "${MAX_TIME:-900}" \
  -H "Authorization: Bearer $(cat "$TOKEN_FILE")" -H "content-type: application/json" \
  -d "$BODY" "http://$HOST:${ORCH_PORT:-8780}/v1/work") || true
if [ ! -s "$OUT" ]; then
  echo "no reply from the service (HTTP ${CODE:-none}); see its log: docker logs on $NAME" >&2
  exit 1
fi
python3 -m json.tool "$OUT"
echo "HTTP $CODE" >&2
