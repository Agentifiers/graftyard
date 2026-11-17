#!/usr/bin/env bash
# lookup-poll's cross-request check (`tests/lookupbed/src/lib.rs`) in local workerd: N concurrent `/read`s in one isolate
# meet `NotLeader` naming no leader for LEADS_IN ms, then a leader; every third client gives up after CANCEL_MS, so its
# request is cancelled mid-lookup, as a hung or disconnected edge request is. Prints each answer's outcome and wait, the
# posts the members took before the leader change, and the count of workerd's cross-request errors. Exits 1 also if a
# round's posts before the change pass MAX_POSTS (one lookup's passes plus each request's first post). Exits 1 if any cross-request error appears
# or any request that was not cancelled failed or waited more than LEADS_IN + MAX_EXTRA ms.
# probe-known's check runs after the three rounds (see below).
# Needs worker-build and tests/m0's wrangler (`npm ci` in tests/m0).
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
P=${P:-8798}; N=${N:-60}; LEADS_IN=${LEADS_IN:-2000}; CANCEL_MS=${CANCEL_MS:-300}; MAX_EXTRA=${MAX_EXTRA:-1500}
MAX_POSTS=${MAX_POSTS:-$((N + 3 * (LEADS_IN / 280 + 1)))}
W=${WRANGLER:-$root/tests/m0/node_modules/.bin/wrangler}
B=$(mktemp -d); cp -r "$here/Cargo.toml" "$here/wrangler.toml" "$here/src" "$B/"
sed -i "s#\.\./\.\./crates#$root/crates#" "$B/Cargo.toml"
cd "$B" && CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$B/target} worker-build --release >"$B/build.log" 2>&1 \
  || { tail -20 "$B/build.log"; exit 1; }
"$W" dev --port $P >"$B/dev.log" 2>&1 & DEV=$!
until curl -s "http://127.0.0.1:$P/nope" 2>/dev/null | grep -q "not found"; do sleep 1; done
u="http://127.0.0.1:$P"
fails=0
for round in 1 2 3; do
  curl -s "$u/arm?leads_in=$LEADS_IN&chain=$round" >/dev/null
  for i in $(seq 1 "$N"); do
    if (( i % 3 == 0 )); then
      (curl -s -m "$(awk "BEGIN{print $CANCEL_MS/1000}")" "$u/read?chain=$round" >"$B/r$round.$i" 2>/dev/null; [ -s "$B/r$round.$i" ] || echo cancelled >"$B/r$round.$i"; echo >>"$B/r$round.$i") &
    else
      (curl -s -m 30 "$u/read?chain=$round" >"$B/r$round.$i"; echo >>"$B/r$round.$i") &
    fi
  done
  wait $(jobs -p | grep -v "^$DEV$") 2>/dev/null
  ok=$(cat "$B"/r$round.* | grep -c '^ok site=1'); can=$(cat "$B"/r$round.* | grep -c '^cancelled')
  bad=$(cat "$B"/r$round.* | grep -v '^$' | grep -vc -e '^ok site=1' -e '^cancelled')
  max=$(cat "$B"/r$round.* | grep -o 'ms=[0-9]*' | cut -d= -f2 | sort -n | tail -1)
  p50=$(cat "$B"/r$round.* | grep -o 'ms=[0-9]*' | cut -d= -f2 | sort -n | awk '{a[NR]=$1} END{print a[int((NR+1)/2)]}')
  posts=$(curl -s "$u/posts?chain=$round")
  echo "round $round: ok $ok cancelled $can other $bad | wait p50 ${p50:-?} max ${max:-?} ms | posts while none led $posts"
  cat "$B"/r$round.* | grep -v '^$' | grep -v -e '^ok site=1' -e '^cancelled' | cut -c1-160 | sort | uniq -c | head -3
  { [ "$bad" = 0 ] && [ "${max:-99999}" -le $((LEADS_IN + MAX_EXTRA)) ] && [ "${posts:-99999}" -le "$MAX_POSTS" ]; } || fails=1
  sleep 3
done
# probe-known: once this isolate holds the group's config at the record's generation, `create` (`ensure_group` before
# a commit) posts nothing. A record of the next generation (a move) is not known: `create` probes, as on main, and the
# old group's leader answers it (a move's new members are joined by the move's own steps, not by `create`).
curl -s "$u/arm?leads_in=0&chain=4" >/dev/null
curl -s "$u/read?chain=4" >/dev/null
c0=$(curl -s "$u/counts?chain=4&gen=1")
known=$(curl -s "$u/create?chain=4&gen=1")
c1=$(curl -s "$u/counts?chain=4&gen=1")
moved=$(curl -s "$u/create?chain=4&gen=2")
c2=$(curl -s "$u/counts?chain=4&gen=1")
echo "known config (gen 1): create $known, member posts $((c1 - c0)) | moved record (gen 2): create $moved, posts $((c2 - c1))"
{ [ "$((c1 - c0))" = 0 ] && [ "$((c2 - c1))" -gt 0 ]; } || fails=1
x=$(grep -c -e 'Cannot perform I/O on behalf of a different request' -e 'after being dropped' -e 'previous WASM instance' \
  -e 'Promise will never complete' "$B/dev.log")
echo "cross-request errors in workerd's log: $x"
grep -o -e 'Cannot perform I/O on behalf of a different request[^.]*' -e 'closure invoked[^.]*' -e 'Promise will never complete' "$B/dev.log" | sort | uniq -c | head
kill $DEV; wait $DEV 2>/dev/null
[ -n "${KEEP:-}" ] && echo "kept $B" || rm -rf "$B"
[ "$x" = 0 ] && [ $fails = 0 ] && echo PASS || { echo FAIL; exit 1; }
