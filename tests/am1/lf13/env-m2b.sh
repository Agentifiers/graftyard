# The graft-m2b install for the B0-29 rerun (tests/fanout/b029.sh): tests/m3/rows/env.sh plus the tools token and the
# install name, which rows.sh's teardown, purge and cost stop read (source this file).
source tests/m3/rows/env.sh
export TOOLS_TOKEN_FILE=$HOME/.graft-m2b/tools.token
export INSTALL=graft-m2b
export HELD=${HELD:-999}
