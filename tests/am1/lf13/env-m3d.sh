# The graft-m3d install, for the hosted drivers (tests/am1/hosted/lib.mjs and the scripts built on it; source this file).
# tests/am1/results/install-m3d.md lists the install.
export GRAFT_EDGE=https://graft-m3d-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev
export GRAFT_TOOLS_EDGE=https://graft-m3d-tools-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev
export BED_TOKEN_FILE=$HOME/.graft-m3d/bed.token
export M3_STATE=$HOME/.graft-m3d/m3
export INSTALL=graft-m3d
export HELD=${HELD:-999}
