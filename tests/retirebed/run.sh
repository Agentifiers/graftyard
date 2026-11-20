#!/usr/bin/env bash
# `retire-sticks` in local workerd (`src/lib.rs`): per arm, a fresh object joins, is retired and reset, is asked to
# join again, then workerd restarts on its persisted state and it is asked once more. Exits 1 unless arm=fix refuses
# both joins and keeps no log, arm=reuse joins (a placement try's clear), and arm=main (main's `/retire`, storage
# deleted only) joins, which is the revive. Needs worker-build and tests/m0's wrangler.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
P=${P:-8796}; W=${WRANGLER:-$root/tests/m0/node_modules/.bin/wrangler}
B=$(mktemp -d); cp -r "$here/Cargo.toml" "$here/wrangler.toml" "$here/src" "$B/"
sed -i "s#\.\./\.\./\.\./workers#$root/workers#" "$B/src/lib.rs"
cd "$B" && CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$B/target} worker-build --release >"$B/build.log" 2>&1 \
  || { tail -20 "$B/build.log"; exit 1; }
[ -x "$W" ] || { echo "no wrangler at $W (npm ci in tests/m0, or WRANGLER=)"; exit 1; }
dev() { setsid "$W" dev --port $P --persist-to "$B/state" >"$B/dev.log" 2>&1 & DEV=$!
  until curl -s "http://127.0.0.1:$P/nope?o=x" 2>/dev/null | grep -q "not found"; do sleep 1; done; }
stop() { kill -- -$DEV 2>/dev/null; wait $DEV 2>/dev/null; }
trap 's=$?; stop; rm -rf "$B"; exit $s' EXIT
u() { curl -s "http://127.0.0.1:$P$1"; }
dev
declare -A first status
for arm in main fix reuse; do
  o=$arm-$RANDOM; eval "o_$arm=$o"
  u "/seed?o=$o" >/dev/null; u "/retire?o=$o&arm=$arm" >/dev/null; sleep 1
  status[$arm]=$(u "/status?o=$o"); first[$arm]=$(u "/join?o=$o")
done
stop; sleep 1; dev
ok=1
for arm in main fix reuse; do
  o_var=o_$arm; second=$(u "/join?o=${!o_var}")
  echo "arm=$arm after retire: ${status[$arm]} | join: ${first[$arm]} | after a workerd restart, join: $second"
  case $arm in
    fix) [[ ${first[$arm]} == refused* && $second == refused* && ${status[$arm]} == *"log=false" ]] || ok=0 ;;
    main|reuse) [[ ${first[$arm]} == joined && $second == joined ]] || ok=0 ;;
  esac
done
[ $ok = 1 ] && echo PASS || { echo FAIL; exit 1; }
