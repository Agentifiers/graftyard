#!/usr/bin/env bash
# Proves a site-script build carries none of the test-only routes and fields (`test-faults` off), or, with
# `--expect-present`, that a test build carries them all. Usage, from the repo root, after `worker-build`:
#   tests/m1/lin/grep-faults.sh workers/site/build [--expect-present]
set -u
dir=${1:?build dir}
mode=${2:-}
# Route strings of 8 bytes or less (`/isolate`, `/faults`) can compile to integer compares with no string in the
# data, so the gated functions' symbol names are checked too (the release wasm keeps its name section).
needles=(/retire /probe_vote /reset /audit /deps_log ::faults:: ::test_route ::fault_for ::appended_kill
  ::audit_json ::isolate_json hold_deps_ms hold_ms no_claim fail_first
  down_ms after_quorum after_quorum_unsynced before_commit before_commit_sent before_commit_early
  GRAFTYARD_FAULTS GRAFTYARD_REQUEST_RETENTION_S drop_append_reply abandoned_timeout_now "killed by an injected fault"
  "graftyard member fault" "member down (injected)" GRAFTYARD_GENESIS ::register::
  /site_fault site_fault: "injected site fault" ::site_down
  /arrivals ::receipt_fault receipt_delay receipt_drop "reply dropped (injected)" no_marker /sweep_run ::RecordReq
  openPr ::open_pr "bf24 #")
found=0
for n in "${needles[@]}"; do
  c=$(cat "$dir/index_bg.wasm" "$dir/index.js" | grep -a -c -F -- "$n")
  printf '%-32s %s\n' "$n" "$c"
  [ "$c" -gt 0 ] && found=$((found + 1))
done
echo "needles present: $found of ${#needles[@]}"
if [ "$mode" = "--expect-present" ]; then [ "$found" -eq "${#needles[@]}" ]; else [ "$found" -eq 0 ]; fi
