#!/usr/bin/env bash
# read-memo's checks in local workerd, on the member's own `workers/site/src/tier.rs` and `txn.rs` with real SQLite
# (`bed/src/lib.rs`, `/memo_scratch` and `/memo`; `decisions/DECISIONS.md`, "`read-memo`"). Exits 1 unless:
# - every edge case holds: a `decide` scratch's write never reaches a later read, a buffered write is read over the
#   memo, a transaction from another handle or a snapshot writer's own statements drop it, `clear` drops it, two
#   prefixes of one table keep their own answers, and a refused read is not kept;
# - a leader and a follower with the memo on keep the fold digest of a member with it off after every entry of a long
#   run, and every reply the leader renders is that member's byte for byte, on B0-33's mix at 60 and 11 commands an
#   entry and on the push-only mix.
# Needs worker-build and tests/m0's wrangler. About five minutes. `tests/m0carried/plant97.py` plants bugs against it.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
P=${P:-8816}; W=$root/tests/m0/node_modules/.bin/wrangler
B=$(mktemp -d); cp -r "$here/bed/Cargo.toml" "$here/bed/wrangler.toml" "$here/bed/src" "$B/"
sed -i "s#\.\./\.\./\.\./\.\./\(workers\|crates\)#$root/\1#g" "$B/src/lib.rs"
sed -i "s#\.\./\.\./\.\./crates#$root/crates#g" "$B/Cargo.toml"
cd "$B"
CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$root/.worktrees/target-leaderprof} worker-build --release >"$B/build.log" 2>&1 \
	|| { tail -20 "$B/build.log"; exit 2; }
setsid "$W" dev --port "$P" --persist-to "$B/state" >"$B/dev.log" 2>&1 & DEV=$!
trap 's=$?; kill -- -$DEV 2>/dev/null; wait $DEV 2>/dev/null || true; rm -rf "$B"; exit $s' EXIT
until curl -s "http://127.0.0.1:$P/noop" 2>/dev/null | grep -q "{}"; do sleep 1; done
fail=0
got=$(curl -s "http://127.0.0.1:$P/memo_scratch?o=edge$RANDOM")
echo "edge cases: $got"
echo "$got" | python3 -c "import json,sys; d=json.load(sys.stdin); sys.exit(0 if d and all(d.values()) else 1)" || fail=1
for q in "cmds=60&entries=300" "cmds=11&entries=300" "cmds=60&entries=100&only=push"; do
	got=$(curl -s "http://127.0.0.1:$P/memo?o=run$RANDOM&$q")
	echo "$q: $(echo "$got" | python3 -c "import json,sys; d=json.load(sys.stdin); print(json.dumps({k: d.get(k) for k in ('repliesCompared', 'mismatch', 'reads')}))" 2>/dev/null || echo "$got" | head -c 400)"
	echo "$got" | python3 -c "import json,sys; d=json.load(sys.stdin); sys.exit(0 if d.get('mismatch') is None and d.get('repliesCompared') else 1)" || fail=1
done
n=$(grep -c -e 'RuntimeError' -e 'out of bounds' "$B/dev.log" || true)
echo "wasm traps: $n"; [ "$n" = 0 ] || fail=1
[ "$fail" = 0 ] && echo "memo: ok" || echo "memo: FAILED"
exit "$fail"
