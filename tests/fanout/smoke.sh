#!/bin/bash
# Memory smokes on graft-m2b, for one orb service: each "<name>:<agents>:<api reads/s or row>" in SMOKES gets a fresh
# repository f<name>, k = 4, and one B0-29-shaped a1.mjs run of SECONDS (default 300). <api reads> "row" keeps the
# row's own rate; a number overrides it (0 = API reads off). Ref reads and pollers stay at the row's rates, as in
# lm1-rerun's diagnostic B0-29. The row's own shard containers are torn down after it.
#   SMOKES="s250:250:0 s500:500:0" [ENTRY=max] TAG=x bash tests/fanout/smoke.sh   -> tests/fanout/raw/smoke-$TAG.status
cd "$(dirname "$0")/../.."
source tests/m3/rows/env.sh
export BED_TOKEN_FILE=${BED_TOKEN_FILE:-$HOME/.graft-m2b/bed.token}
ST=tests/fanout/raw/smoke-$TAG.status
[ -e tests/fanout/raw/.smoke-$TAG.started ] && exec sleep infinity
touch tests/fanout/raw/.smoke-$TAG.started
id() { awk -v n="$1" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt"; }
for s in $SMOKES; do
  IFS=: read -r name agents reads <<< "$s"
  repo="f$name"
  node tests/am1/runs/mkrepos.mjs "$repo" >> "$ST" 2>&1
  rid=$(id "$repo")
  [ -n "$rid" ] || { echo "$name: no repo" >> "$ST"; continue; }
  [ -n "${LEARNERS:-}" ] && node tests/fanout/learners.mjs "repo:$rid" "$LEARNERS" >> "$ST" 2>&1
  node tests/fanout/k.mjs "repo:$rid" 4 >> "$ST" 2>&1
  # ENTRY=max|<bytes>: every member's entry fill limit through test-faults `/stats?entry=` (m0-carried-7's A/B
  # switch), so a smoke compares ENTRY_SOFT with ENTRY_MAX on one deploy. A member keeps it until its isolate resets.
  [ -n "${ENTRY:-}" ] && node -e '
    import("./tests/am1/hosted/lib.mjs").then(async (l) => { for (const s of [0, 1, 2]) {
      const r = await l.member(process.argv[1], s, `/stats?entry=${process.argv[2]}&summary=1`, undefined, 15000);
      console.log(`entry ${process.argv[2]} on site ${s}: ${r.status} ${r.v?.entrySoft ?? ""}`); } })' "repo:$rid" "$ENTRY" >> "$ST" 2>&1
  flags="--agents $agents --seconds ${SECONDS_:-300}"
  [ "$reads" = row ] || flags="$flags --api-reads $reads"
  echo "$(date -u +%FT%TZ) $name $repo $rid start $flags" >> "$ST"
  bash tests/am1/runs/a1.sh b0-29 "$repo" "$rid" $flags
  echo "$(date -u +%FT%TZ) $name $repo $rid done $(grep -E '^exit' tests/am1/raw/runs/a1-b0-29-$repo.log)" >> "$ST"
  # The identity groups' own failed-call records, before their members idle out (LM-1 round 8).
  node tests/fanout/idstats.mjs ${IDCHAINS:-tokens:2490 names:152 names:708 release} > "tests/fanout/raw/idstats-$TAG-$name.json" 2>&1
  python3 tests/fanout/teardown.py "$rid" "$rid" > /dev/null 2>&1 && echo "$(date -u +%FT%TZ) torn down $rid" >> "$ST"
  echo "$(date -u +%FT%TZ) cost $(T0_FILE=${T0_FILE:-tests/fanout/raw/siteday-r2.t0} bash tests/fanout/cost.sh 2>/dev/null)" >> "$ST"
done
echo DONE >> "$ST"
exec sleep infinity
