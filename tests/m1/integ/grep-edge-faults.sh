#!/usr/bin/env bash
# Proves a git edge build (workers/edge) carries none of its test-only routes, headers, and variables (`test-faults`
# off), or, with `--expect-present`, that a test build carries them all. Usage, from the repo root, after
# `worker-build --release [--features test-faults]` in workers/edge:
#   tests/m1/integ/grep-edge-faults.sh workers/edge/build [--expect-present]
set -u
dir=${1:?build dir}
mode=${2:-}
needles=(/_bed/ BED_TOKEN x-bed-token x-graft-kill x-graft-down-ms x-graft-outcome-index x-graft-rid FORCE_FORMAT
  GRAFTYARD_GENESIS ::bed:: ::each_member /_bed/fault /site_fault /_bed/identity x-graft-shard ::bed_archive
  ::bed_place fail_place no_marker LOGINS)
found=0
for n in "${needles[@]}"; do
  c=$(cat "$dir/index_bg.wasm" "$dir/index.js" | grep -a -c -F -- "$n")
  printf '%-24s %s\n' "$n" "$c"
  [ "$c" -gt 0 ] && found=$((found + 1))
done
echo "needles present: $found of ${#needles[@]}"
if [ "$mode" = "--expect-present" ]; then [ "$found" -eq "${#needles[@]}" ]; else [ "$found" -eq 0 ]; fi
