#!/bin/bash
# BF-34's Terraform half on graft-m2b: terraform-provider-github (built from /tmp/tpg at its HEAD into /tmp/tfrows,
# with terraform 1.17.1 there) against the edge, as m3-admin.
# 1. `TestAccGithubRepositoryCollaborator` itself (TF_ACC, organization mode, owner m3org, GH_TEST_EXTERNAL_USER1 a
#    fresh user). Its config creates a `github_repository`, which the edge does not serve.
# 2. The same `github_repository_collaborator` resource on the existing m3org/j4: apply, a second plan (no changes),
#    and destroy, which withdraws the pending invitation.
# The token is read from the state file into GITHUB_TOKEN for the two processes only; it is never on argv or logged.
#   bash tests/m3/rows/tf.sh <ext login> > tests/m3/raw/rows/tf.log 2>&1
set -uo pipefail
cd "$(dirname "$0")/../../.."
source tests/m3/rows/env.sh
EXT=$1
TF=/tmp/tfrows
GITHUB_TOKEN=$(awk '$1 == "admin" { print $2 }' "$M3_STATE/m3-tokens.txt")
export GITHUB_TOKEN GITHUB_OWNER=m3org GITHUB_BASE_URL="$GRAFT_EDGE/" GH_TEST_AUTH_MODE=organization
export GH_TEST_EXTERNAL_USER1=$EXT TF_CLI_CONFIG_FILE=$TF/tf.rc TF_IN_AUTOMATION=1 CHECKPOINT_DISABLE=1
date -u +%FT%TZ
echo "== 1. TestAccGithubRepositoryCollaborator"
(cd /tmp/tpg && git rev-parse --short HEAD && TF_ACC=1 TF_ACC_TERRAFORM_PATH=$TF/terraform \
	go test ./github -run '^TestAccGithubRepositoryCollaborator$' -count=1 -v -timeout 10m 2>&1 | tail -40)
echo "== 2. github_repository_collaborator on m3org/j4"
cd $TF/cfg
rm -rf .terraform* terraform.tfstate*
export TF_VAR_collaborator=$EXT
$TF/terraform apply -auto-approve -no-color 2>&1 | grep -vE '^\s*$' | tail -15
echo "-- plan after apply (want: No changes)"
$TF/terraform plan -no-color -detailed-exitcode 2>&1 | grep -E 'No changes|to add|to change|to destroy|Error' ; echo "plan exit=${PIPESTATUS[0]}"
echo "-- destroy"
$TF/terraform destroy -auto-approve -no-color 2>&1 | grep -vE '^\s*$' | tail -8
date -u +%FT%TZ
