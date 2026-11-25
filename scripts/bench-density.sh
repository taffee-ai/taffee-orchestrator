#!/usr/bin/env bash
# Rerun the published density benchmark on your own machine and check your numbers against ours.
# Needs Docker and a checkout with the Strands submodule:
#
#   git clone --recurse-submodules https://github.com/taffee-ai/taffee-orchestrator
#   cd taffee-orchestrator
#   scripts/bench-density.sh
#
# Everything builds and runs inside the builder image; nothing is installed on your machine.
# Results land in docs/evidence/<date>-<label>/. See docs/benchmarks/density.md.
#
#   BENCH_LABEL=my-laptop scripts/bench-density.sh        name the results folder (default: local)
#   DENSITY_LEVELS=1,10,50,100,200 scripts/bench-density.sh   more agents (the check uses the
#                                                             levels the reference has)
set -euo pipefail
cd "$(dirname "$0")/.."

command -v docker >/dev/null || { echo "Docker is required: https://docs.docker.com/get-docker/" >&2; exit 2; }
if [ ! -d upstream/strands-harness-sdk/strands-ts/src ]; then
  echo "the Strands submodule is missing: run 'git submodule update --init'" >&2; exit 2
fi

# The builder image downloads an x86_64 Node and the published figures are x86_64. On an arm64
# host Docker emulates it: memory figures stay comparable, CPU figures do not.
PLATFORM=linux/amd64
LABEL=${BENCH_LABEL:-local}
OUT=docs/evidence/$(date -u +%Y-%m-%d)-density-$LABEL
SOURCE=unknown
if command -v git >/dev/null && git rev-parse --git-dir >/dev/null 2>&1; then
  SOURCE="$(git rev-parse --short HEAD)$([ -n "$(git status --porcelain --untracked-files=no)" ] && echo '-modified')"
fi

docker build --platform "$PLATFORM" -t taffee-orch-builder:dev -f docker/builder.Dockerfile docker
docker run --rm --platform "$PLATFORM" \
  -e BENCH_OUT="/src/$OUT" -e BENCH_SOURCE="$SOURCE" \
  -e DENSITY_LEVELS="${DENSITY_LEVELS:-1,10,50,100}" \
  -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" -e PYTHONDONTWRITEBYTECODE=1 \
  -v "$PWD":/src \
  -v taffee-orch-target:/work/target \
  -v taffee-orch-cargo:/usr/local/cargo/registry \
  -v taffee-orch-node-modules:/src/strand-agent/node_modules \
  taffee-orch-builder:dev bash scripts/bench-density-run.sh

echo
echo "results: $OUT"
