#!/usr/bin/env bash
# BF-12's double-apply evidence through `workers/site/src/txn.rs`, in local workerd (`src/lib.rs`): per object, apply
# request r1 at index 5, again at index 9, then restart workerd on its persisted state. Exits 1 unless the first apply leaves no `dup` row,
# the second leaves exactly `r1@9`, and the row is still there after the restart. PLANT=1 builds without the feature
# (no `dup` write at all), which must fail it. Needs worker-build and tests/m0's wrangler.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../../.." && pwd)
P=${P:-8798}; W=$root/tests/m0/node_modules/.bin/wrangler
B=$(mktemp -d); cp -r "$here/Cargo.toml" "$here/wrangler.toml" "$here/src" "$B/"
sed -i "s#\.\./\.\./\.\./\.\./workers#$root/workers#" "$B/src/lib.rs"
cd "$B" && CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$B/target} worker-build --release ${PLANT:+--no-default-features} >"$B/build.log" 2>&1 \
  || { tail -20 "$B/build.log"; exit 1; }
dev() { "$W" dev --port $P --persist-to "$B/state" >"$B/dev.log" 2>&1 & DEV=$!
  until curl -s "http://127.0.0.1:$P/nope?o=x" 2>/dev/null | grep -q "not found"; do sleep 1; done; }
dev
u() { curl -s "http://127.0.0.1:$P$1"; }
o=o$RANDOM
u "/apply?o=$o&rid=r1&idx=5" >/dev/null; first=$(u "/dup?o=$o")
u "/apply?o=$o&rid=r1&idx=9" >/dev/null; second=$(u "/dup?o=$o")
u "/apply?o=$o&rid=r2&idx=10" >/dev/null; third=$(u "/dup?o=$o")
kill $DEV; wait $DEV 2>/dev/null; sleep 2; dev; after=$(u "/dup?o=$o")
echo "first apply: [$first] | same rid again at 9: [$second] | a new rid: [$third] | after restart: [$after]"
kill $DEV; wait $DEV 2>/dev/null; rm -rf "$B"
[ -z "$first" ] && [ "$second" = "r1@9" ] && [ "$third" = "r1@9" ] && [ "$after" = "r1@9" ] && { echo PASS; exit 0; }
echo FAIL; exit 1
