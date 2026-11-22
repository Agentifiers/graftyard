#!/usr/bin/env bash
# BF-39's test hooks in `graft-ops` (`workers/ops/src/exec.rs`): `OPS_KILL` and `OPS_STEP_MIN_ENTRIES` exist only in a
# `test-faults` build. Builds the Worker both ways, as `workers/ops/deploy.mjs` does, and fails unless the product
# wasm names neither variable and the test-faults wasm names both. A product `graft-ops` that could read either one
# could be killed or have its gate lowered by a variable. About two minutes. Run from the repository root.
set -uo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
B=$(mktemp -d); trap 'rm -rf "$B"' EXIT
ok=1
for kind in product faults; do
	f=(); [ $kind = faults ] && f=(--features test-faults)
	(cd "$root/workers/ops" && worker-build --release --out-dir "$B/$kind" "${f[@]}") >"$B/$kind.log" 2>&1 \
		|| { tail -20 "$B/$kind.log"; exit 1; }
	wasm=$(ls "$B/$kind"/*.wasm "$B/$kind"/*/*.wasm 2>/dev/null | head -1)
	for v in OPS_KILL OPS_STEP_MIN_ENTRIES; do
		n=$(grep -c -a "$v" "$wasm")
		echo "$kind $v: $n"
		if [ $kind = product ] && [ "$n" != 0 ]; then echo "FAIL: the product build reads $v"; ok=0; fi
		if [ $kind = faults ] && [ "$n" = 0 ]; then echo "FAIL: the test-faults build lacks $v"; ok=0; fi
	done
done
[ $ok = 1 ] && echo PASS || { echo FAIL; exit 1; }
