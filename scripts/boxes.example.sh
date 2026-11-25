# Test machines for scripts/remote.sh. Copy to scripts/boxes.local.sh (git-ignored) and fill in.
# Each box_<name> sets:
#   BOX    ssh destination: user@host, or an alias from ~/.ssh/config
#   KEY    ssh key (optional if your ssh config already picks one)
#   PRE    shell command run first on the machine (e.g. put docker on PATH); ":" for none
#   LABEL  evidence folder suffix: docs/evidence/<date>-<LABEL>/. Keep it neutral; it is committed.
DEFAULT_BOX=node1

box_node1() {   # a Linux machine with Docker
  BOX=user@test-host-1
  KEY=$HOME/.ssh/id_test
  PRE=":"
  LABEL=taffee-localtest-node1
}

box_node2() {   # a Mac with Docker Desktop; docker is not on PATH over ssh
  BOX=test-host-2
  KEY=$HOME/.ssh/id_test
  PRE='export PATH=/usr/local/bin:/Applications/Docker.app/Contents/Resources/bin:$PATH'
  LABEL=taffee-localtest-node2
}
