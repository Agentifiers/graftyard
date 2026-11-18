#!/bin/bash
# Runs an unmodified actions/runner against the in-memory run service (crates/actions/examples/runsvc_host.rs).
#
#   tests/m4/wp3/run-host.sh <runner dir> <workflow.yml> <out dir> [port] [kill-after-s]
#
# <runner dir> is an extracted actions-runner-linux-x64-2.338.2. The host registers one runner, creates a run from the
# workflow's jobs, and exits when every job completed; the runner is ephemeral, so the script restarts it with a new JIT
# config for each job. With kill-after-s, the runner's whole process tree is killed that many seconds into the first
# job and nothing more is started: the job must end at its lease (BF-4). Results are in <out dir>.
set -euo pipefail
runner=$1 wf=$2 out=$3 port=${4:-8790} kill_after=${5:-}
repo=$(cd "$(dirname "$0")/../../.." && pwd)
cargo build -q --release -p graftyard-actions --example runsvc_host --manifest-path "$repo/Cargo.toml"
rm -rf "$out" && mkdir -p "$out"
"${CARGO_TARGET_DIR:-$repo/target}/release/examples/runsvc_host" "$port" "$wf" "$out" > "$out/host.out" 2>&1 &
host=$!
n=0
while kill -0 $host 2>/dev/null; do
  for _ in $(seq 1 60); do [ -s "$out/jitconfig.txt" ] && break; sleep 1; done
  n=$((n + 1))
  (cd "$runner" && rm -f .runner .credentials .credentials_rsaparams)
  jit=$(cat "$out/jitconfig.txt")
  rm -f "$out/jitconfig.txt"
  (cd "$runner" && exec setsid ./run.sh --jitconfig "$jit" > "$out/runner-$n.out" 2>&1) &
  r=$!
  if [ -n "$kill_after" ]; then
    sleep "$kill_after"
    kill -9 -- -"$r" 2>/dev/null || true
    echo "runner killed at $(date -u +%H:%M:%SZ)" >> "$out/host.out"
    wait $host
    break
  fi
  wait $r || true
  # The host writes a fresh JIT config for the next runner when jobs remain.
  for _ in $(seq 1 10); do [ -s "$out/jitconfig.txt" ] || ! kill -0 $host 2>/dev/null && break; sleep 1; done
done
wait $host || true
cat "$out/host.out"
