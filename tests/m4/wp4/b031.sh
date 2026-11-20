#!/bin/bash
# B0-31 on graft-m4a, as one orb service: rows.sh's B0-31 steps on an install whose edge is a product build (no
# `/_bed` routes, so no bed shard warm, shard gate, or `/_bed/kill`). A fresh repository, rehomed on apac
# (`transfer.mjs repo:ID 2`), its apac shard woken by a client clone instead of shardwarm.mjs, LM-1's 8 learners
# (enam 4, weur 2, apac 2), k 4 through the sites' test-faults `/bench_seed`, then `a1.mjs b0-31 --seconds 600` with
# the confined m3-ci fleet (Agent class, 1 MiB pushes). The edge ignores `x-graft-place-fail` on a product build, so
# both halves run with buckets up. The per-second leader and admission traces, the voters' memory, and tsave run
# beside it; the teardown sweep goes through the scratch shardkill Worker. The stop watcher (b035watch.sh) runs as
# its own service with the same TAG.
#   TAG=<tag> bash tests/m4/wp4/b031.sh   -> tests/m4/raw/wp4/b031-<tag>.status
set -uo pipefail
cd "$(dirname "$0")/../../.."
export ROWS_ENV=tests/am1/lf13/env-m4a.sh
source $ROWS_ENV
[ "$INSTALL" = graft-m4a ] || { echo "INSTALL is $INSTALL, not graft-m4a" >&2; exit 1; }
ST=tests/m4/raw/wp4/b031-$TAG.status
mkdir -p tests/m4/raw/wp4 tests/am1/raw/runs tests/fanout/raw
[ -e tests/m4/raw/wp4/.b031-$TAG.started ] && exec sleep infinity
[ -z "$(git status --porcelain --untracked-files=no)" ] || { echo "b031.sh: commit the driver changes first" >&2; exit 1; }
touch tests/m4/raw/wp4/.b031-$TAG.started
stamp() { date -u +%FT%TZ; }
end() { echo "$(stamp) $1" >> "$ST"; [ ${#tracers[@]} -gt 0 ] && kill "${tracers[@]}" 2>/dev/null
  if python3 tests/fanout/teardown.py --sweep --shardkill >> "$ST" 2>&1; then echo "$(stamp) sweep ok" >> "$ST"
  else echo "$(stamp) SWEEP FAILED" >> "$ST"; fi
  echo ALLDONE >> "$ST"; exec sleep infinity; }
tracers=()
echo "$(stamp) start $(git rev-parse --short HEAD)" >> "$ST"
repo="b031-$TAG"
node tests/fanout/terms.mjs "$repo" 10800 > tests/fanout/raw/terms-$TAG-b0-31.jsonl 2>&1 & tracers+=($!)
node tests/fanout/admit.mjs "$repo" 10800 > tests/fanout/raw/admit-$TAG-b0-31.jsonl 2>&1 & tracers+=($!)
node tests/fanout/vmem.mjs "$repo" 10800 > tests/fanout/raw/vmem-$TAG-b0-31.jsonl 2>&1 & tracers+=($!)
node tests/am1/runs/mkrepos.mjs "$repo" >> "$ST" 2>&1
rid=$(awk -v n="$repo" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt")
[ -n "$rid" ] || end "no repo"
node tests/am1/runs/transfer.mjs "repo:$rid" 2 "$repo" >> "$ST" 2>&1
tail -1 "$ST" | grep -q "^rehomed on 2" || end "b0-31: not rehomed on apac, row skipped"
# A client clone wakes the shard that serves the repository's git reads, as a bed `/catchup` would.
node --input-type=module -e "
const { Git, tokens } = await import('./tests/am1/hosted/lib.mjs');
for (let i = 0; i < 6; i++) { const t = Date.now(); const r = new Git('m3org/$repo').as(tokens.admin).clone();
  console.log(new Date().toISOString(), 'warm clone', i, r.ok, Date.now() - t, 'ms', r.ok ? '' : r.out.slice(0, 120));
  if (r.ok) break; await new Promise((d) => setTimeout(d, 10000)); }" >> "$ST" 2>&1
node tests/fanout/learners.mjs "repo:$rid" enam:4000,weur:2000,apac:2000 >> "$ST" 2>&1 \
  || end "b0-31: LEARNERS NOT PLACED, row skipped"
node tests/fanout/k.mjs "repo:$rid" 4 >> "$ST" 2>&1
tail -1 "$ST" | grep -q ": k 4 at " || echo "$(stamp) b0-31: k 4 not granted; the row runs at the k it has" >> "$ST"
echo "$(stamp) b0-31 $repo $rid start" >> "$ST"
rm -f tests/fanout/raw/.tsave-$TAG-b0-31.done
DONE_FILE=tests/fanout/raw/.tsave-$TAG-b0-31.done EVERY=30000 LEARNERS=1 node tests/nr3/tsave.mjs "repo:$rid" \
  > tests/fanout/raw/tsave-$TAG-b0-31.jsonl 2> /tmp/b031tsave-$TAG.err & tracers+=($!)
bash tests/am1/runs/a1.sh b0-31 "$repo" "$rid" --seconds 600
touch tests/fanout/raw/.tsave-$TAG-b0-31.done
node tests/fanout/k.mjs "repo:$rid" 1 >> "$ST" 2>&1
echo "$(stamp) b0-31 $repo $rid done $(grep -E '^exit' tests/am1/raw/runs/a1-b0-31-$repo.log)" >> "$ST"
sleep 70
end "row ended"
