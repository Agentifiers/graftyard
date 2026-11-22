#!/usr/bin/env bash
# A repository leader's CPU per entry and per command, by stage, in local workerd (`bed/src/lib.rs`): wasm32 under V8
# with real SQLite and the member's own `tier.rs` and `txn.rs`, on B0-33's committed command mix at N commands an
# entry (60, as at full load, and 11, as at a quarter load; `tests/am1/results/b033-quarter.md`). For each N it prints
# three runs' stage times on fresh objects and three after 300 entries of state, one run with no peers, workerd's own
# process CPU around each run, and the runtime's CPU per `/noop` request (the per-request floor); `table.py` then prints
# the medians by stage, per entry and per command. A profile, not a gate. Needs worker-build and tests/m0's wrangler. Exits 1
# on a failed route or a wasm trap.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
P=${P:-8813}; W=$root/tests/m0/node_modules/.bin/wrangler
# The bed's member DDL must be the product's, verbatim.
for s in "CREATE TABLE IF NOT EXISTS raft_state" "CREATE TABLE IF NOT EXISTS log " "CREATE TABLE IF NOT EXISTS fold " \
	"CREATE TABLE IF NOT EXISTS request " "CREATE INDEX IF NOT EXISTS request_at"; do
	line=$(grep -F "\"$s" "$here/bed/src/lib.rs" | sed 's/^ *//')
	grep -qF "$line" "$root/workers/site/src/host.rs" || { echo "bed DDL differs from host.rs SCHEMA: $line"; exit 1; }
done
B=$(mktemp -d); cp -r "$here/bed/Cargo.toml" "$here/bed/wrangler.toml" "$here/bed/src" "$B/"
sed -i "s#\.\./\.\./\.\./\.\./\(workers\|crates\)#$root/\1#g" "$B/src/lib.rs"
sed -i "s#\.\./\.\./\.\./crates#$root/crates#g" "$B/Cargo.toml"
cd "$B"
CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$root/.worktrees/target-leaderprof} worker-build --release >"$B/build.log" 2>&1 \
	|| { tail -20 "$B/build.log"; exit 1; }
# Its own session: wrangler's shutdown signals its whole process group.
setsid "$W" dev --port "$P" --persist-to "$B/state" >"$B/dev.log" 2>&1 & DEV=$!
trap 's=$?; kill -- -$DEV 2>/dev/null; wait $DEV 2>/dev/null || true; rm -rf "$B"; exit $s' EXIT
until curl -s "http://127.0.0.1:$P/noop" 2>/dev/null | grep -q "{}"; do sleep 1; done
tck=$(getconf CLK_TCK)
wpid() { for p in $(pgrep -x workerd); do echo "$(awk '/VmRSS/{print $2}' /proc/$p/status) $p"; done | sort -n | tail -1 | cut -d' ' -f2; }
cpu() { awk '{print $14 + $15}' "/proc/$1/stat"; }
pid=$(wpid)
out=$B/out.jsonl; k=0
# Each shape three times on a fresh object (grow=0) and three times after 300 untimed entries of state (grow=300), and
# once with no peers, so the sends' share of workerd's process CPU is the difference.
for n in 60 11; do
	curl -sf "http://127.0.0.1:$P/prof?o=warm$n&cmds=$n&entries=5&peers=6" >/dev/null || { echo "warm-up failed"; exit 1; }
	for spec in "0 6" "0 6" "0 6" "300 6" "300 6" "300 6" "0 0"; do
		set -- $spec; k=$((k + 1)); c0=$(cpu "$pid")
		got=$(curl -sf "http://127.0.0.1:$P/prof?o=run$k&cmds=$n&entries=30&peers=$2&grow=$1") || { echo "prof N=$n failed"; exit 1; }
		c1=$(cpu "$pid")
		echo "$got" | python3 -c "import json,sys; j=json.load(sys.stdin); j['processCpuMs']=$(( (c1 - c0) * 1000 / tck )); print(json.dumps(j))" | tee -a "$out"
	done
done
c0=$(cpu "$pid")
for _ in $(seq 500); do curl -sf "http://127.0.0.1:$P/noop" >/dev/null; done
c1=$(cpu "$pid")
echo "{\"noop\": 500, \"processCpuMsPerRequest\": $(python3 -c "print(round(($c1 - $c0) * 1000 / $tck / 500, 3))")}" | tee -a "$out"
n=$(grep -c -e 'RuntimeError' -e 'out of bounds' "$B/dev.log" || true)
echo "wasm traps: $n"; [ "$n" = 0 ]
python3 "$here/table.py" "$out"
