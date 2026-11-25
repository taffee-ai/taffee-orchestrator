#!/usr/bin/env bash
# Deploy orchestratord as a long-running service on a test machine, then check it answers
# over the network. Run from your workstation.
#
#   scripts/deploy-node.sh              run CI on the machine, build the image, deploy
#   SKIP_CI=1 scripts/deploy-node.sh    reuse the last CI build (sync only)
#   TAFFEE_BOX=node1 scripts/deploy-node.sh
#
# Required: MODEL_ENDPOINT, the base URL of your OpenAI-style gateway (replaces the placeholder in
# deploy/orchestrator.node.json), e.g. MODEL_ENDPOINT=https://model-gateway.example/v1
#
# On the machine: the service directory (~/taffee-orchestrator-service) holds the config, the
# caller API bearer tokens (created once, mode 600) and the compose env file. The model key stays where
# it is (MODEL_KEY_FILE, default ~/.secrets/model.key) and is mounted read-only.
# A copy of the caller API bearer token is kept on this workstation in ~/.taffee/<label>.token (mode 600).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
LOCAL="$(cd "$HERE/.." && pwd)"
[ -f "$HERE/boxes.local.sh" ] || { echo "missing scripts/boxes.local.sh (copy boxes.example.sh)" >&2; exit 2; }
PRE=":"; LABEL=""; KEY=""; DEFAULT_BOX=""
. "$HERE/boxes.local.sh"
NAME=${TAFFEE_BOX:-node2}
declare -F "box_$NAME" >/dev/null || { echo "no box_$NAME in boxes.local.sh" >&2; exit 2; }
"box_$NAME"
SSH=(ssh ${KEY:+-i "$KEY"} -o ServerAliveInterval=30 "$BOX")
REMOTE=${TAFFEE_REMOTE_DIR:-projects/taffee-orchestrator}
SERVICE_DIR=${TAFFEE_SERVICE_DIR:-taffee-orchestrator-service}
MODEL_KEY_FILE=${MODEL_KEY_FILE:-.secrets/model.key}
PORT=${ORCH_PORT:-8780}
MODEL_ENDPOINT=${MODEL_ENDPOINT:?set MODEL_ENDPOINT to your gateway base URL}
MODEL_HOST=${MODEL_ENDPOINT%/v1}

VERSION=$(git -C "$LOCAL" rev-parse --short HEAD 2>/dev/null || echo unknown)
# Uncommitted changes get a fingerprint of their content, so two dirty builds can be told apart.
if [ -n "$(git -C "$LOCAL" status --porcelain 2>/dev/null)" ]; then
  FP=$( { git -C "$LOCAL" diff HEAD; git -C "$LOCAL" ls-files -o --exclude-standard -z | (cd "$LOCAL" && xargs -0 cat 2>/dev/null); } | shasum | cut -c1-7)
  VERSION="$VERSION-dirty.$FP"
fi
echo "== deploying $VERSION to $NAME"

# 1. Sync, test and build the binaries in the builder container.
if [ "${SKIP_CI:-0}" = 1 ]; then
  TAFFEE_BOX=$NAME "$HERE/remote.sh" -- true
else
  TAFFEE_BOX=$NAME "$HERE/remote.sh"
fi

# 2. Build the runtime image, prepare the service directory, start or replace the container.
"${SSH[@]}" bash -s <<REMOTE_SCRIPT
set -euo pipefail
$PRE
cd ~/$REMOTE
[ -s ~/$MODEL_KEY_FILE ] || { echo "no model key at ~/$MODEL_KEY_FILE" >&2; exit 2; }
docker build -q --build-arg TAFFEE_VERSION=$VERSION -t taffee-orchestrator:$VERSION -f docker/Dockerfile . >/dev/null
mkdir -p ~/$SERVICE_DIR && chmod 700 ~/$SERVICE_DIR
sed -e "s#https://model-gateway.example/v1#$MODEL_ENDPOINT#" -e "s#https://model-gateway.example#$MODEL_HOST#" deploy/orchestrator.node.json > ~/$SERVICE_DIR/orchestrator.json
if [ ! -s ~/$SERVICE_DIR/api-tokens ]; then
  ( umask 077; printf 'user:owner %s\n' "\$(openssl rand -hex 24)" > ~/$SERVICE_DIR/api-tokens )
fi
cat > ~/$SERVICE_DIR/service.env <<ENV
ORCH_BIND=0.0.0.0
ORCH_PORT=$PORT
ORCH_IMAGE_TAG=$VERSION
ORCH_MEM_LIMIT=2g
ORCH_MAX_AGENTS=20
ORCH_API_TOKENS_FILE=\$HOME/$SERVICE_DIR/api-tokens
ORCH_CONFIG_FILE=\$HOME/$SERVICE_DIR/orchestrator.json
ORCH_MODEL_KEY_FILE=\$HOME/$MODEL_KEY_FILE
ENV
docker compose -p taffee-orchestrator --env-file ~/$SERVICE_DIR/service.env \
  -f deploy/compose.orchestrator.yaml -f deploy/compose.node.yaml up -d --remove-orphans
C=\$(docker compose -p taffee-orchestrator --env-file ~/$SERVICE_DIR/service.env \
  -f deploy/compose.orchestrator.yaml -f deploy/compose.node.yaml ps -q orchestrator)
for i in \$(seq 1 60); do
  s=\$(docker inspect -f '{{.State.Health.Status}}' "\$C" 2>/dev/null || echo missing)
  [ "\$s" = healthy ] && break
  [ "\$s" = unhealthy ] && break
  sleep 3
done
echo "container health: \$s"
docker logs --tail 6 "\$C" 2>&1 | sed 's/\x1b\[[0-9;]*m//g'
[ "\$s" = healthy ]
REMOTE_SCRIPT

# 3. Keep a copy of the caller API bearer token here, then check the service from this workstation.
mkdir -p ~/.taffee && chmod 700 ~/.taffee
( umask 077; "${SSH[@]}" "cut -d' ' -f2 ~/$SERVICE_DIR/api-tokens | head -1" > ~/.taffee/"${LABEL:-$NAME}".token )
HOST=$(ssh -G "$BOX" | awk '$1 == "hostname" { print $2 }')
URL="http://$HOST:$PORT"
echo "== service: $URL"
curl -sf --max-time 10 "$URL/healthz" && echo
[ "$(curl -sf --max-time 10 "$URL/healthz" | sed 's/.*"version":"\([^"]*\)".*/\1/')" = "$VERSION" ] \
  && echo "== running version $VERSION, reachable from this workstation" \
  || { echo "== the service does not report $VERSION" >&2; exit 1; }
