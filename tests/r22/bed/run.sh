#!/usr/bin/env bash
# R22's local check (tests/am1/results/r22-sweep-fill.md): the bed built against GIT_ROOT's crates (default this
# worktree; pass a checkout of origin/main for the before arm), N packs of SIZE bytes seeded into bucket A, workerd
# restarted so seeding's heap is gone, then peak workerd RSS while one sweep fills B and C, and every copy checked.
# EDGE=1 (this worktree only) also runs /edge. Needs worker-build and tests/m0's wrangler.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../../.." && pwd)
GIT_ROOT=${GIT_ROOT:-$root}; N=${N:-16}; SIZE=${SIZE:-20000000}; P=${P:-8795}
W=$root/tests/m0/node_modules/.bin/wrangler
B=$(mktemp -d); S=$B/state; cp -r "$here/Cargo.toml" "$here/wrangler.toml" "$here/src" "$B/"
sed -i "s#\.\./\.\./\.\./crates#$GIT_ROOT/crates#" "$B/Cargo.toml"
cd "$B" && CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$B/target} worker-build --release ${EDGE:+--features edge} >"$B/build.log" 2>&1 \
  || { tail -20 "$B/build.log"; exit 1; }
dev() { "$W" dev --port $P --persist-to $S >"$B/dev.log" 2>&1 & DEV=$!
  until curl -s http://127.0.0.1:$P/nope 2>/dev/null | grep -q "not found"; do sleep 1; done; }
wd() { for p in $(pgrep -x workerd); do echo "$(awk '/VmRSS/{print $2}' /proc/$p/status) $p"; done | sort -n | tail -1 | cut -d' ' -f2; }
dev
for i in $(seq 1 $N); do curl -s "http://127.0.0.1:$P/seed?i=$i&size=$SIZE" >/dev/null; done
kill $DEV; wait $DEV 2>/dev/null; sleep 2; dev
pid=$(wd); idle=$(( $(awk '/VmRSS/{print $2}' /proc/$pid/status) / 1024 )); max=0
curl -s "http://127.0.0.1:$P/sweep" >"$B/sweep.json" & c=$!
while kill -0 $c 2>/dev/null; do r=$(awk '/VmRSS/{print $2}' /proc/$pid/status 2>/dev/null || echo 0); ((r > max)) && max=$r; sleep 0.1; done
echo "idle_rss_mb=$idle peak_rss_mb=$((max / 1024)) sweep=$(cat "$B/sweep.json")"
kill $DEV; wait $DEV 2>/dev/null; sleep 1; dev
echo "count=$(curl -s http://127.0.0.1:$P/count)"
[ -n "${EDGE:-}" ] && echo "edge=$(curl -s http://127.0.0.1:$P/edge)"
kill $DEV; wait $DEV 2>/dev/null; rm -rf "$B"
