#!/usr/bin/env bash
# The far-leg A/B (`tests/m0/results/farleg.md`): one site day of the tree in DEPLOY on graft-m3d, then two 120 s
# runs of journey 9's mix (`--mix j9`) at 30 commands/s, B0-29's 27.6 a group, each on a new group, with BENCH_RAW so
# each run keeps the leader's trace. ARM names the runs. Resumes from the raw rows.
#
#   DEPLOY=/tmp/farA ARM=main bash tests/m0carried/farleg.sh
#   DEPLOY=/tmp/farB ARM=adapt bash tests/m0carried/farleg.sh
set -uo pipefail
cd "${DEPLOY:?}"
export CARGO_TARGET_DIR=/home/user/scratch/far-target
export BED_TOKEN_FILE=$HOME/.graft-m3d/bed.token
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:15f6ed1b1d4b2b08c8adccbce713e89762677c3656dd7ee9a99bdd4bcab025c5
VARS=$(mktemp); echo '{"SHARD_INSTANCE": "standard-4"}' > $VARS
date -u +%FT%TZ; git rev-parse --short HEAD
( cd workers/site && worker-build --release --features test-faults > /tmp/far-build-$ARM.log 2>&1 ); echo "BUILD EXIT $?"
node workers/site/deploy.mjs $HOME/.graft-m3d/sites.json --skip-build --test-faults --anchor $HOME/.graft-m3d/install.toml --image $IMG --vars $VARS 2>&1 | grep -E -i "Current Version ID|error|deployed|warn" | grep -v -i token
echo "SITEDAY EXIT ${PIPESTATUS[0]}"; date -u +%FT%TZ
rm -f $VARS
( cd tests/m0carried && npx --prefix ../m0 wrangler deploy -c edge-m3d.jsonc 2>&1 | grep -E "Version ID|rror"
  tr -d '\n' < ../m0/.m0-token | npx --prefix ../m0 wrangler secret put TOKEN -c edge-m3d.jsonc 2>&1 | grep -E "Success|rror" | grep -v -i value )
echo "BENCH EDGE DONE"; date -u +%FT%TZ
sleep 60
export EDGE_URL=https://graft-m3d-wp2-edge.<workers-subdomain>.workers.dev BENCH_RAW=1
RAW=tests/m1/wp2/raw
mkdir -p $RAW
for rate in 30; do
  for i in 1 2; do
    run=far-$ARM-r$rate-$i
    [ -f $RAW/$run.bench.json ] && continue
    node tests/m1/wp2/run.ts bench --run $run --rate $rate --seconds 120 --arrival mem --mix j9 > /tmp/$run.log 2>&1
    echo "$run $(jq -c '[.throughputPerS,.commitMs.p99,.leaderConfirmMs.p99,.commandsPerEntry,.entries,.leader,.failed,.verdict]' $RAW/$run.bench.json 2>/dev/null)"
  done
done
date -u +%FT%TZ
echo RUNS_DONE
exec sleep infinity
