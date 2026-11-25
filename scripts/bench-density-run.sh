#!/usr/bin/env bash
# The density benchmark itself. Runs INSIDE the builder image; scripts/bench-density.sh starts it.
# Builds the guest component and the density binary from this checkout, starts the mock model,
# runs the bench and checks the result against docs/benchmarks/density-reference.json.
set -euo pipefail
if [ ! -f /.dockerenv ]; then
  echo "refusing to run outside a container: use scripts/bench-density.sh" >&2
  exit 2
fi
cd "$(dirname "$0")/.."
ROOT=$(pwd)
OUT=${BENCH_OUT:?BENCH_OUT is set by scripts/bench-density.sh}
mkdir -p "$OUT"

fix_owner() {
  [ -n "${HOST_UID:-}" ] || return 0
  chown -R "$HOST_UID:${HOST_GID:-$HOST_UID}" "$OUT" "$ROOT/strand-agent/dist" 2>/dev/null || true
}
trap 'kill $(jobs -p) 2>/dev/null || true; fix_owner' EXIT

log() { echo; echo "=== $* ($(date -u +%H:%M:%S)) ==="; }

log "environment"
{
  echo "date_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "source: ${BENCH_SOURCE:-unknown}"
  echo "kernel: $(uname -srm)"
  echo "cpu: $(awk -F': ' '/^model name/{print $2; exit}' /proc/cpuinfo)"
  echo "cpus_visible: $(nproc)"
  echo "mem_total_mb: $(awk '/MemTotal/{print int($2/1024)}' /proc/meminfo)"
  echo "rustc: $(rustc --version)"
  echo "node: $(node --version)"
  echo "python: $(python3 --version)"
  echo "density_levels: ${DENSITY_LEVELS:-1,10,50,100}"
} | tee "$OUT/environment.txt"

log "guest: build strand-agent.wasm"
cd "$ROOT/strand-agent"
# Always install from the committed lock file, then prove node_modules matches it, so the bench
# builds with exactly the dependencies the published runs used.
npm install --no-audit --no-fund
touch node_modules/.taffee-installed
python3 "$ROOT/scripts/check-node-modules.py" . | tee "$OUT/node-modules.log"
node build.mjs > "$OUT/guest-build.log" 2>&1 || { cat "$OUT/guest-build.log"; exit 1; }
sha256sum dist/strand-agent.wasm | tee "$OUT/component.sha256"

log "host: build the density binary (Cargo.lock as committed)"
cd "$ROOT/runner"
cargo build --release --locked --bin density 2>&1 | tail -2

log "mock model on :8000"
cd "$ROOT/mock-bedrock"
python3 server.py --port 8000 > /tmp/bench-mock-model.log 2>&1 &
for _ in $(seq 1 50); do curl -sf http://127.0.0.1:8000/ping >/dev/null && break; sleep 0.1; done
curl -sf http://127.0.0.1:8000/ping >/dev/null || { echo "mock model did not start"; cat /tmp/bench-mock-model.log; exit 1; }

# Compiling the component takes seconds and leaves the compiler's memory in the process. The
# service compiles once and keeps the result in a cache (a volume in deploy/), so the bench does
# the same: a short warm-up run compiles and caches it, and the measured run loads the cache.
log "warm-up: compile the component once and cache it"
DENSITY_LEVELS=1 DENSITY_COMPONENT="$ROOT/strand-agent/dist/strand-agent.wasm" "$CARGO_TARGET_DIR/release/density" \
  > "$OUT/warmup.log" 2>&1 || { cat "$OUT/warmup.log"; exit 1; }
head -1 "$OUT/warmup.log"

log "density bench"
DENSITY_COMPONENT="$ROOT/strand-agent/dist/strand-agent.wasm" "$CARGO_TARGET_DIR/release/density" 2>&1 | tee "$OUT/density.log"
sed -n 's/^DENSITY_RESULT //p' "$OUT/density.log" > "$OUT/density.json"
[ -s "$OUT/density.json" ] || { echo "the bench printed no DENSITY_RESULT line"; exit 1; }

log "check against the published reference"
python3 "$ROOT/scripts/bench-density-check.py" \
  "$ROOT/docs/benchmarks/density-reference.json" "$OUT/density.json" "$OUT/component.sha256" \
  2>&1 | tee "$OUT/density-check.log"
