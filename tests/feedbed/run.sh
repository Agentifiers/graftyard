#!/usr/bin/env bash
# feed-per-member's bed (`tests/feedbed/src/lib.rs`) in local workerd: a learner holds N feed polls while its group's
# voter, in the same isolate, takes STEPS inputs. Run twice: KEY=repo (FEEDS keyed by repository, as on main) must log
# workerd's cross-object errors, which shows the bed reproduces m3fp3; KEY=member (as now) must log none, and every
# learner poll must end held with 0 wakes (the voter's inputs are not its own). Exits 1 otherwise.
# Needs worker-build and tests/m0's wrangler (`npm ci` in tests/m0).
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
P=${P:-8797}; N=${N:-30}; STEPS=${STEPS:-20}
W=${WRANGLER:-$root/tests/m0/node_modules/.bin/wrangler}
B=$(mktemp -d); cp -r "$here/Cargo.toml" "$here/wrangler.toml" "$here/src" "$B/"
cd "$B" && CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$B/target} worker-build --release >"$B/build.log" 2>&1 \
  || { tail -20 "$B/build.log"; exit 1; }
L='repo:7#1#l0'; V='repo:7#1#f0'
enc() { python3 -c 'import sys,urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"; }
run() {
  local mode=$1
  "$W" dev --port $P --var "KEY:$mode" >"$B/dev-$mode.log" 2>&1 & DEV=$!
  until curl -s "http://127.0.0.1:$P/nope" 2>/dev/null | grep -q "not found"; do sleep 1; done
  local u="http://127.0.0.1:$P"
  for i in $(seq 1 "$N"); do (curl -s -m 20 "$u/poll?m=$(enc $L)&wait=4000" >"$B/p-$mode.$i"; echo >>"$B/p-$mode.$i") & done
  sleep 1
  for s in $(seq 1 "$STEPS"); do curl -s "$u/step?m=$(enc $V)" >/dev/null; sleep 0.05; done
  wait $(jobs -p | grep -v "^$DEV$") 2>/dev/null
  sleep 1
  kill $DEV; wait $DEV 2>/dev/null
  local x held woken
  x=$(grep -c -e 'Cannot perform I/O on behalf of a different' -e 'after being dropped' "$B/dev-$mode.log")
  held=$(cat "$B"/p-$mode.* | grep -c '^held')
  woken=$(cat "$B"/p-$mode.* | grep -o 'woken=[0-9]*' | cut -d= -f2 | awk '{s+=$1} END {print s+0}')
  echo "KEY=$mode: $held of $N polls answered held, $woken wakes from the voter's inputs, cross-object errors $x"
  grep -o -e 'Cannot perform I/O on behalf of a different[^.]*' -e 'closure invoked[^.]*' "$B/dev-$mode.log" | sort | uniq -c | head -3
  eval "X_$mode=$x HELD_$mode=$held WOKEN_$mode=$woken"
}
run repo
run member
[ -n "${KEEP:-}" ] && echo "kept $B" || rm -rf "$B"
if [ "$X_repo" -gt 0 ] && [ "$X_member" = 0 ] && [ "$HELD_member" = "$N" ] && [ "$WOKEN_member" = 0 ]; then echo PASS; exit 0; fi
echo FAIL; exit 1
