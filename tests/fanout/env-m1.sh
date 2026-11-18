# The graft-m1 install for the B0-34 drivers written for graft-m2b (tests/fanout/b034.sh, costrow.sh): source this
# file, or pass ROWS_ENV=tests/fanout/env-m1.sh. Hostnames come from the environment; no secret is in this file.
export GRAFT_EDGE=https://graft-m1-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev
export GRAFT_TOOLS_EDGE=https://graft-m1-wp7-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev
export BED_TOKEN_FILE=$HOME/.graft-wp6/bed.token
export M3_STATE=$HOME/.graft-m3h
# The install's script prefix (teardown.py, netimages.py, misses.py, purge.py), its edge script (edgever.py), and its
# site buckets (r2list.mjs, purge.py): graft-m1's are graft-m1-packs-<site>, not <install>-site-<site>-packs.
export INSTALL=graft-m1 EDGE_SCRIPT=graft-m1-edge R2_BUCKET='graft-m1-packs-{site}' BUCKET='{install}-packs-{site}'
# graft-m1's edge does not enable 32.13, so REST create answers 503: b034setup.mjs makes the repositories as
# tests/am1/b030/mkrepo.mjs does (CreateRepo on the owner's Authz, then RepoOwner).
export B034_CREATE=owner
