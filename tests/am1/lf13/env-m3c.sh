# The graft-m3c install, for LF-13's drivers (tests/am1/rest/lf13.mjs through tests/fanout/lf13.sh; source this file).
export GRAFT_EDGE=https://graft-m3c-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev
export GRAFT_TOOLS_EDGE=https://graft-m3c-tools-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev
export BED_TOKEN_FILE=$HOME/.graft-m3c/bed.token
export M3_STATE=$HOME/.graft-m3c/m3
export INSTALL=graft-m3c
export HELD=${HELD:-999}
