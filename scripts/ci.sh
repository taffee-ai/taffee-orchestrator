#!/usr/bin/env bash
# Build and test everything. Runs INSIDE the builder image (scripts/remote.sh starts it);
# never run it on a bare host.
#
#   scripts/ci.sh            build guest + host, mock tests, unit + integration tests
#   RUN_DENSITY=1 ...        also run the density bench (DENSITY_LEVELS, default 1,10,50,100)
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$(pwd)
EVID=${EVIDENCE_DIR:-$ROOT/docs/evidence/$(date -u +%Y-%m-%d)${EVIDENCE_SUFFIX:-}}
mkdir -p "$EVID" "$ROOT/dist"
if [ ! -f /.dockerenv ]; then
  echo "refusing to run outside a container (see AGENTS.md)" >&2
  exit 2
fi

log() { echo; echo "=== $* ($(date -u +%H:%M:%S)) ==="; }
# Files written into the bind-mounted checkout belong to the box user, not root.
fix_owner() {
  [ -n "${HOST_UID:-}" ] || return 0
  chown -R "$HOST_UID:${HOST_GID:-$HOST_UID}" "$ROOT/dist" "$ROOT/docs/evidence" "$ROOT/strand-agent/dist" \
    "$ROOT/strand-agent/package-lock.json" "$ROOT/mock-bedrock" 2>/dev/null || true
}
trap fix_owner EXIT

log "environment"
rustc --version; cargo --version; node --version; python3 --version
echo "host: $(uname -srm)  cpus: $(nproc)  mem: $(awk '/MemTotal/{print int($2/1024)" MB"}' /proc/meminfo)"

log "guest: build strand-agent.wasm"
cd "$ROOT/strand-agent"
if [ ! -d node_modules/@bytecodealliance/componentize-js ] || [ package.json -nt node_modules/.taffee-installed ] \
   || [ package-lock.json -nt node_modules/.taffee-installed ]; then
  npm install --no-audit --no-fund
  touch node_modules/.taffee-installed
fi
python3 "$ROOT/scripts/check-node-modules.py" . | tee "$EVID/node-modules.log"
if [ "${SKIP_GUEST_BUILD:-0}" != 1 ] || [ ! -f dist/strand-agent.wasm ]; then
  node build.mjs 2>&1 | tee "$EVID/guest-build.log"
fi
ls -l dist/strand-agent.wasm
sha256sum dist/strand-agent.wasm | tee "$EVID/component.sha256"

log "mock-bedrock: unit tests"
cd "$ROOT/mock-bedrock"
python3 -m unittest -v test_server 2>&1 | tee "$EVID/mock-test.log"

log "mocks: start model (:8000) and MCP (:8001)"
python3 server.py --port 8000 > /tmp/mock-model.log 2>&1 &
python3 server.py --port 8001 --mcp-token test-mcp-token-0123456789 > /tmp/mock-mcp.log 2>&1 &
trap 'kill $(jobs -p) 2>/dev/null || true; fix_owner' EXIT
for p in 8000 8001; do
  for _ in $(seq 1 50); do curl -sf "http://127.0.0.1:$p/ping" >/dev/null && break; sleep 0.1; done
done

log "host: unit tests"
cd "$ROOT/runner"
cargo test --release --lib 2>&1 | tee "$EVID/runner-unit.log"

log "host: integration tests"
cargo test --release --test integration_test -- --test-threads=1 2>&1 | tee "$EVID/runner-integration.log"

log "work manifest: conformance vectors (docs/spec/work-manifest/vectors)"
cargo test --release --test conformance -- --nocapture --test-threads=1 2>&1 | tee "$EVID/conformance.log"

log "work manifest: engine end to end"
cargo test --release --test work_test -- --test-threads=1 2>&1 | tee "$EVID/work-engine.log"

log "host: build binaries"
cargo build --release --bins 2>&1 | tail -3
cp "$CARGO_TARGET_DIR/release/orchestratord" "$CARGO_TARGET_DIR/release/taffee-run" "$CARGO_TARGET_DIR/release/density" "$ROOT/dist/"
ls -l "$ROOT/dist"

if [ "${RUN_DENSITY:-0}" = 1 ]; then
  log "density bench"
  DENSITY_COMPONENT="$ROOT/strand-agent/dist/strand-agent.wasm" "$ROOT/dist/density" 2>&1 | tee "$EVID/density.log"
fi

log "done: evidence in $EVID"
