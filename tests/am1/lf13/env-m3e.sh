# The graft-m3e install, for the hosted drivers (tests/am1/hosted/lib.mjs and the scripts built on it; source this file).
# tests/am1/results/install-m3e.md lists the install.
export GRAFT_EDGE=https://graft-m3e-edge.<workers-subdomain>.workers.dev
export GRAFT_TOOLS_EDGE=https://graft-m3e-tools-edge.<workers-subdomain>.workers.dev
export BED_TOKEN_FILE=$HOME/.graft-m3e/bed.token
export M3_STATE=$HOME/.graft-m3e/m3
export INSTALL=graft-m3e
export HELD=${HELD:-999}
export TOOLS_TOKEN_FILE=$HOME/.graft-m3e/tools.token
