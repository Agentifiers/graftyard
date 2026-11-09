#!/bin/bash
# BF-34's Terraform row as written on graft-m2b: terraform-provider-github's `TestAccGithubRepositoryCollaborator`
# (built from /tmp/tpg into /tmp/tfrows, terraform 1.17.1 there) against the edge as m3-admin, <runs> times. Its two
# runnable subtests each create a `github_repository` (auto_init) in m3org, a `github_repository_collaborator` with
# `triage` for GH_TEST_EXTERNAL_USER1, and destroy both (`DELETE /repos/m3org/tf-acc-test-repo-collab-*`); the third
# skips without GH_TEST_COLLABORATOR, as it does on github.com CI. After each run, every `tf-acc-test-` repository
# that run created must answer 404. The token reaches the test only through GITHUB_TOKEN in this script's
# environment; never argv or a log (go test -v prints no token).
#   bash tests/m3/repos/tf.sh <ext login> <runs> <out dir>
set -uo pipefail
cd "$(dirname "$0")/../../.."
source tests/m3/rows/env.sh
EXT=$1; RUNS=${2:-5}; OUT=$3
mkdir -p "$OUT"
TF=/tmp/tfrows
GITHUB_TOKEN=$(awk '$1 == "admin" { print $2 }' "$M3_STATE/m3-tokens.txt")
export GITHUB_TOKEN GITHUB_OWNER=m3org GITHUB_BASE_URL="$GRAFT_EDGE/" GH_TEST_AUTH_MODE=organization
export GH_TEST_EXTERNAL_USER1=$EXT TF_CLI_CONFIG_FILE=$TF/tf.rc TF_IN_AUTOMATION=1 CHECKPOINT_DISABLE=1
(cd /tmp/tpg && git rev-parse --short HEAD) > "$OUT/provider-rev.txt"
for i in $(seq 1 "$RUNS"); do
  start=$(date -u +%FT%TZ)
  # The test's working directories are kept (TF_ACC_PERSIST_WORKING_DIR) under a fresh TMPDIR, so the
  # configurations name the repositories this run created; no state file holds the token.
  tmp=$(mktemp -d)
  (cd /tmp/tpg && TMPDIR=$tmp TF_ACC_PERSIST_WORKING_DIR=1 TF_ACC=1 TF_ACC_TERRAFORM_PATH=$TF/terraform \
    go test ./github -run '^TestAccGithubRepositoryCollaborator$' -count=1 -v -timeout 15m) > "$OUT/run-$i.log" 2>&1
  rc=$?
  # The repositories this run named, and what GET answers for each now.
  gone=""
  for n in $(grep -rhoE 'tf-acc-test-repo-collab-[A-Za-z0-9]{5}' --include='*.tf' "$tmp" | sort -u); do
    s=$(curl -s -m 60 -o /dev/null -w '%{http_code}' -H @- "$GRAFT_EDGE/api/v3/repos/m3org/$n" <<<"authorization: token $GITHUB_TOKEN")
    gone="$gone $n=$s"
  done
  pass=$(grep -cE '^\s*--- PASS' "$OUT/run-$i.log"); fail=$(grep -cE '^\s*--- FAIL' "$OUT/run-$i.log"); skip=$(grep -cE '^\s*--- SKIP' "$OUT/run-$i.log")
  rm -rf "$tmp"
  printf '%s\trun %s\trc=%s\tpass=%s\tfail=%s\tskip=%s\tafter:%s\n' "$start" "$i" "$rc" "$pass" "$fail" "$skip" "$gone" | tee -a "$OUT/summary.tsv"
done
echo TF-DONE >> "$OUT/summary.tsv"
