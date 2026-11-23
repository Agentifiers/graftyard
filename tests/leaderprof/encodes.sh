#!/usr/bin/env bash
# Lever A's split in local workerd (`tests/am1/results/seal-bytes-design.md`, section 8): builds the leaderprof bed and
# asks `/encodes` five times at each entry size (4 to 256 KB of agent inline packs), 30 reps a stage, 10 peers. Writes
# one JSON line per ask to OUT (default tests/fanout/raw/leaderprof-encodes.jsonl). Needs worker-build and tests/m0's
# wrangler. A profile, not a gate. Exits 1 on a failed ask or a wasm trap.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
P=${P:-8814}; W=$root/tests/m0/node_modules/.bin/wrangler; OUT=${OUT:-$root/tests/fanout/raw/leaderprof-encodes.jsonl}
B=$(mktemp -d); cp -r "$here/bed/Cargo.toml" "$here/bed/wrangler.toml" "$here/bed/src" "$B/"
sed -i "s#\.\./\.\./\.\./\.\./\(workers\|crates\)#$root/\1#g" "$B/src/lib.rs"
sed -i "s#\.\./\.\./\.\./crates#$root/crates#g" "$B/Cargo.toml"
cd "$B"
CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$root/.worktrees/target-leaderprof} worker-build --release >"$B/build.log" 2>&1 \
	|| { tail -20 "$B/build.log"; exit 1; }
setsid "$W" dev --port "$P" --persist-to "$B/state" >"$B/dev.log" 2>&1 & DEV=$!
trap 's=$?; kill -- -$DEV 2>/dev/null; wait $DEV 2>/dev/null || true; rm -rf "$B"; exit $s' EXIT
until curl -s "http://127.0.0.1:$P/noop" 2>/dev/null | grep -q "{}"; do sleep 1; done
curl -sf "http://127.0.0.1:$P/encodes?o=warm&kb=128&reps=5" >/dev/null
: >"$OUT"
for rep in 1 2 3 4 5; do for kb in 4 32 72 95 128 256; do
	curl -sf "http://127.0.0.1:$P/encodes?o=e$rep-$kb&kb=$kb&reps=30&peers=10" >>"$OUT"; echo >>"$OUT"
done; done
n=$(grep -c -e 'RuntimeError' -e 'out of bounds' "$B/dev.log" || true)
echo "wasm traps: $n"; [ "$n" = 0 ]
