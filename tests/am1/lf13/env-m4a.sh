# The graft-m4a install, for the hosted drivers (tests/am1/hosted/lib.mjs and the scripts built on it; source this file).
# tests/am1/results/install-m4a.md lists the install.
export GRAFT_EDGE=https://graft-m4a-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev
export GRAFT_TOOLS_EDGE=https://graft-m4a-tools-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev
export BED_TOKEN_FILE=$HOME/.graft-m4a/bed.token
# The tools token lib.mjs reads; without it a driver looks for tests/m0/.m0-token, which this checkout lacks (m57's and
# m59's first starts).
export TOOLS_TOKEN_FILE=$HOME/.graft-m4a/tools.token
export M3_STATE=$HOME/.graft-m4a/m3
export INSTALL=graft-m4a
export HELD=${HELD:-999}
