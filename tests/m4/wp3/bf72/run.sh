#!/bin/bash
# BF-72 locally: an unmodified actions/runner 2.338.2 against `runsvc_host`, with `octo/shared` a mirror repository whose
# `deploy.yml` is at tag `v1` and a branch also named `v1` holds a different file (GHES: the tag wins).
#
#   tests/m4/wp3/bf72/run.sh <runner dir> <out dir> [port]
#
# Passes when every job succeeds, the called job prints the caller's typed inputs and the inherited secret's hash, the
# later job reads the called workflow's output, and the called job's ID token names `octo/shared`'s file at the tag and
# carries the caller run's actor and run number (BF-73's rows on a planned run).
set -euo pipefail
runner=$1 out=$2 port=${3:-8797}
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)
shared=$(mktemp -d)/shared
mkdir -p "$shared"
git -C "$shared" init -q -b main
cp -r "$here/shared/.github" "$shared/"
g() { git -C "$shared" -c user.name=b -c user.email=b@b "$@"; }
g add -A && g commit -qm v1 && g tag v1
sed 's|/v1"|/branch"|' "$here/shared/.github/workflows/deploy.yml" > "$shared/.github/workflows/deploy.yml"
g checkout -qb v1 && g commit -qam branch && g checkout -q main
secret=bf72-$(date +%s%N)
want_key=$(printf %s "$secret" | sha256sum | cut -c1-16)
MIRROR_EXTRA="octo/shared=$shared" HOST_SECRETS="DEPLOY_KEY=$secret" HOST_ACTOR=monalisa \
  bash "$repo/tests/m4/wp3/run-host.sh" "$runner" "$here/caller.yml" "$out" "$port" > "$out.host.log" 2>&1 || true
logs=$(cat "$out"/runner-*.out "$out"/blobs/* 2>/dev/null || true)
fail=0
check() { if eval "$2"; then echo "PASS $1"; else echo "FAIL $1"; fail=1; fi; }
check "every job succeeded" "grep -q 'all jobs completed' '$out/host.out' && ! grep -q 'conclusion: \\(Failure\\|Skipped\\|Cancelled\\)' '$out/fold.json' && [ \$(grep -c Success '$out/fold.json') -eq 3 ]"
check "the called job ran as 'call / deploy'" "grep -q '\"name\": \"call / deploy\"' '$out/fold.json'"
check "typed inputs" "grep -aq 'BF72 called target=prod-7 dry=true dry_is_bool=true' <<<\"\$logs\""
check "the inherited secret" "grep -aq 'BF72 called key_sha=$want_key' <<<\"\$logs\""
check "the tag, not the branch" "grep -aq 'BF72 called version=v1' <<<\"\$logs\" && grep -aq 'url=https://prod-7.example/v1' <<<\"\$logs\""
check "the later job read the output" "grep -aq 'BF72 after url=https://prod-7.example/v1 result=success' <<<\"\$logs\""
check "job_workflow_ref names the called file" "grep -aq 'BF72 job_workflow_ref=octo/shared/.github/workflows/deploy.yml@refs/tags/v1' <<<\"\$logs\""
check "workflow_ref stays the caller's" "grep -aq 'BF72 workflow_ref=owner/repo/' <<<\"\$logs\""
check "the called job's token carries the caller run's actor and number" "grep -aq 'BF72 actor=monalisa run_number=1' <<<\"\$logs\""
exit $fail
