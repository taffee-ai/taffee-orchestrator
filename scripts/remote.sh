#!/usr/bin/env bash
# Run scripts/ci.sh on a test machine inside the builder image. Run this from your workstation.
# Nothing builds locally.
#
#   scripts/remote.sh                 sync, build image if needed, run ci.sh
#   scripts/remote.sh -- <command>    run another command in the builder container instead
#   RUN_DENSITY=1 scripts/remote.sh   also run the density bench
#   TAFFEE_BOX=node2 scripts/remote.sh   run on another machine from scripts/boxes.local.sh
#
# Machines are defined in scripts/boxes.local.sh (git-ignored); copy scripts/boxes.example.sh.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
if [ ! -f "$HERE/boxes.local.sh" ]; then
  echo "missing scripts/boxes.local.sh: copy scripts/boxes.example.sh and fill in your machines" >&2; exit 2
fi
PRE=":"; LABEL=""; KEY=""; DEFAULT_BOX=""
# shellcheck source=boxes.example.sh
. "$HERE/boxes.local.sh"
NAME=${TAFFEE_BOX:-$DEFAULT_BOX}
if declare -F "box_$NAME" >/dev/null; then "box_$NAME"; else BOX=$NAME; fi   # else: user@host
KEY=${TAFFEE_BOX_KEY:-$KEY}
SUFFIX=${LABEL:+-$LABEL}
REMOTE=${TAFFEE_REMOTE_DIR:-projects/taffee-orchestrator}
LOCAL="$(cd "$(dirname "$0")/.." && pwd)"
SSH=(ssh ${KEY:+-i "$KEY"} -o ServerAliveInterval=30 "$BOX")

# No -t: changed files must get a fresh mtime on the box, or cargo can miss an edit made
# while an earlier build was running. --checksum decides what changed.
rsync -rlpz --checksum --delete -e "ssh ${KEY:+-i $KEY}" \
  --exclude='.git' --exclude='target/' --exclude='node_modules/' --exclude='dist/' \
  --exclude='strand-agent/dist/' --exclude='*.cwasm' --exclude='.builder-built' --exclude='__pycache__/' \
  --exclude='docs/evidence/' --exclude='scripts/boxes.local.sh' \
  "$LOCAL/" "$BOX:$REMOTE/"

CMD="scripts/ci.sh"
if [ "${1:-}" = "--" ]; then shift; CMD="$*"; fi

"${SSH[@]}" bash -s <<REMOTE_SCRIPT
set -euo pipefail
$PRE
cd ~/$REMOTE
if ! docker image inspect taffee-orch-builder:dev >/dev/null 2>&1 || [ docker/builder.Dockerfile -nt .builder-built ]; then
  docker build -t taffee-orch-builder:dev -f docker/builder.Dockerfile docker
  touch .builder-built
fi
# Detached: a dropped SSH session must not kill the run (both boxes are on Wi-Fi).
docker rm -f taffee-orch-ci >/dev/null 2>&1 || true
docker run -d --name taffee-orch-ci \
  -e RUN_DENSITY=${RUN_DENSITY:-0} -e DENSITY_LEVELS=${DENSITY_LEVELS:-1,10,50,100} \
  -e SKIP_GUEST_BUILD=${SKIP_GUEST_BUILD:-0} -e PYTHONDONTWRITEBYTECODE=1 \
  -e EVIDENCE_SUFFIX=$SUFFIX \
  -e HOST_UID=\$(id -u) -e HOST_GID=\$(id -g) \
  -v "\$PWD":/src \
  -v taffee-orch-target:/work/target \
  -v taffee-orch-cargo:/usr/local/cargo/registry \
  -v taffee-orch-node-modules:/src/strand-agent/node_modules \
  taffee-orch-builder:dev bash -c "$CMD" >/dev/null
REMOTE_SCRIPT

# Follow the logs until the container exits, reconnecting after a drop. Lines printed while
# disconnected are not replayed here; the full logs are in the evidence directory.
rc=""; since=""
for _ in $(seq 1 120); do
  "${SSH[@]}" "$PRE; docker logs -f $since taffee-orch-ci" 2>&1 || true
  if ! st=$("${SSH[@]}" "$PRE; echo \$(docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' taffee-orch-ci) \$(date +%s)" 2>/dev/null); then
    echo "(lost the connection to $BOX; retrying in 15s)"; sleep 15; continue
  fi
  set -- $st
  if [ "${1:-}" = exited ]; then rc=$2; break; fi
  echo "(log stream dropped; reconnecting)"; since="--since ${3:-0}"
done
"${SSH[@]}" "$PRE; docker rm taffee-orch-ci >/dev/null 2>&1" || true

# Bring the evidence (and the resolved lock files) back to the Mac, so a lock file the build
# had to change shows up in git status instead of being re-resolved on every run.
rsync -az -e "ssh ${KEY:+-i $KEY}" "$BOX:$REMOTE/docs/evidence/" "$LOCAL/docs/evidence/" 2>/dev/null || true
# --checksum: a lock file can change without changing size or mtime.
rsync -az --checksum -e "ssh ${KEY:+-i $KEY}" "$BOX:$REMOTE/strand-agent/package-lock.json" "$LOCAL/strand-agent/package-lock.json" 2>/dev/null || true
rsync -az --checksum -e "ssh ${KEY:+-i $KEY}" "$BOX:$REMOTE/runner/Cargo.lock" "$LOCAL/runner/Cargo.lock" 2>/dev/null || true

if [ -z "$rc" ]; then echo "gave up waiting for taffee-orch-ci on $BOX"; exit 1; fi
exit "$rc"
