# The graft-m3f install, for the hosted drivers (tests/am1/hosted/lib.mjs and the scripts built on it; source this file).
# tests/am1/results/install-m3f.md lists the install.
export GRAFT_EDGE=https://graft-m3f-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev
export GRAFT_TOOLS_EDGE=https://graft-m3f-tools-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev
export BED_TOKEN_FILE=$HOME/.graft-m3f/bed.token
export M3_STATE=$HOME/.graft-m3f/m3
export INSTALL=graft-m3f
export HELD=${HELD:-999}
