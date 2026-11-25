#!/usr/bin/env bash
# Build dist/strand-agent.wasm. Runs inside the builder image (scripts/ci.sh calls the same
# steps); do not run it on a bare host.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
[ -f /.dockerenv ] || { echo "run inside the builder container: scripts/remote.sh" >&2; exit 2; }
npm install --no-audit --no-fund
node build.mjs
ls -lh dist/
