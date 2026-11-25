#!/bin/bash
# BF-70's local half at $0: an unmodified actions/runner runs tests/m4/wp3/oidc/minio.yml under the in-memory run
# service (crates/actions/examples/runsvc_host.rs), whose job declares `id-token: write`. configure-aws-credentials
# v6.2.0 takes the job's ID token to a local MinIO's STS, which verifies it against the host's published JWKS and
# issues credentials, and an S3 read uses them. The host switches ActionsOidcKey to slot B after the first mint, and
# the job's second token, signed under B, is taken by MinIO too. A second job with no grant sees no token URL.
#
#   tests/m4/wp3/oidc/run.sh <runner dir> <configure-aws-credentials checkout> <out dir> [port]
#
# HOSTED=1 runs BF-70's hosted workflow (`hosted.yml`) instead, with the action mirrored as `m3org/bf70-cac`, and also
# checks that the job verified its token against the published JWKS and that the ungranted job's own token got 403.
#
# Needs docker (sudo -n) with the images cgr.dev/chainguard/minio and minio-client, and the aws CLI. Exits 0 only when every check holds.
set -uo pipefail
runner=$1 cac=$2 out=$3 port=${4:-8792}
repo=$(cd "$(dirname "$0")/../../../.." && pwd)
unset HTTP_PROXY HTTPS_PROXY http_proxy https_proxy
rm -rf "$out" && mkdir -p "$out"
git clone -q --bare --branch v6.2.0 "$cac" "$out/cac.git"
# MinIO reads the issuer's discovery document once at start, so the host comes up first, then MinIO, then the runner.
cargo build -q --release -p graftyard-actions --example runsvc_host --manifest-path "$repo/Cargo.toml" || exit 1
bin=${CARGO_TARGET_DIR:-$repo/target}/release/examples/runsvc_host
host_log="$out/host"
mkdir -p "$host_log"
# The workflow by its repository path, which the token's `workflow` and `workflow_ref` carry.
if [ "${HOSTED:-}" = 1 ]; then wf=tests/m4/wp3/oidc/hosted.yml as=m3org/bf70-cac
else wf=tests/m4/wp3/oidc/minio.yml as=aws-actions/configure-aws-credentials; fi
(cd "$repo" && OIDC_ROTATE_AFTER=1 MIRROR_EXTRA="$as=$out/cac.git" exec "$bin" "$port" "$wf" "$host_log") > "$out/host.out" 2>&1 &
host=$!
for _ in $(seq 60); do curl -sf "http://127.0.0.1:$port/_services/token/.well-known/jwks" >/dev/null && break; sleep 1; done
sudo -n docker rm -f bf70minio >/dev/null 2>&1
sudo -n docker run -d --name bf70minio --network host -e MINIO_ROOT_USER=root -e MINIO_ROOT_PASSWORD=rootpass123 \
  -e "MINIO_IDENTITY_OPENID_CONFIG_URL=http://127.0.0.1:$port/_services/token/.well-known/openid-configuration" \
  -e MINIO_IDENTITY_OPENID_CLIENT_ID=sts.amazonaws.com -e MINIO_IDENTITY_OPENID_CLAIM_NAME=repository_owner \
  cgr.dev/chainguard/minio:latest server /data --address 127.0.0.1:9000 >/dev/null || exit 1
trap 'sudo -n docker rm -f bf70minio >/dev/null 2>&1; kill $host 2>/dev/null; [ -n "${mcdir:-}" ] && sudo -n rm -rf "$mcdir"' EXIT
for _ in $(seq 30); do curl -sf http://127.0.0.1:9000/minio/health/live >/dev/null && break; sleep 1; done
AWS_ACCESS_KEY_ID=root AWS_SECRET_ACCESS_KEY=rootpass123 aws --endpoint-url http://127.0.0.1:9000 --region us-east-1 \
  s3 mb s3://probe >/dev/null && echo probe | AWS_ACCESS_KEY_ID=root AWS_SECRET_ACCESS_KEY=rootpass123 \
  aws --endpoint-url http://127.0.0.1:9000 --region us-east-1 s3 cp - s3://probe/hello.txt >/dev/null
# The policy the token's `repository_owner` names (the host's repository is owner/repo): the probe bucket only.
echo '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:*"],"Resource":["arn:aws:s3:::probe","arn:aws:s3:::probe/*"]}]}' > "$out/owner-policy.json"
mcdir=$(mktemp -d) && chmod 777 "$mcdir"
mc() { sudo -n docker run --rm --network host -v "$out/owner-policy.json:/p.json" -v "$mcdir:/mc" -e MC_CONFIG_DIR=/mc \
  cgr.dev/chainguard/minio-client:latest "$@"; }
mc alias set l http://127.0.0.1:9000 root rootpass123 >/dev/null && mc admin policy create l owner /p.json || exit 1
n=0
while kill -0 $host 2>/dev/null; do
  for _ in $(seq 60); do [ -s "$host_log/jitconfig.txt" ] || ! kill -0 $host 2>/dev/null && break; sleep 1; done
  [ -s "$host_log/jitconfig.txt" ] || break
  n=$((n + 1))
  (cd "$runner" && rm -f .runner .credentials .credentials_rsaparams)
  jit=$(cat "$host_log/jitconfig.txt"); rm -f "$host_log/jitconfig.txt"
  (cd "$runner" && exec setsid ./run.sh --jitconfig "$jit" > "$out/runner-$n.out" 2>&1) &
  wait $! || true
done
wait $host
ok=1
check() { if grep -rq "$2" $1; then echo "PASS $3"; else echo "FAIL $3"; ok=0; fi; }
diag="$host_log/blobs"
check "$out/runner-*.out" "Job sts completed with result: Succeeded" "the id-token job succeeded"
check "$out/runner-*.out" "Job denied completed with result: Succeeded" "the job with no grant succeeded"
check "$diag" "the runner exported both" "the runner exported ACTIONS_ID_TOKEN_REQUEST_URL and _TOKEN"
check "$diag" "s3 read ok" "configure-aws-credentials' credentials read the bucket"
check "$diag" "second token ok" "MinIO took the token signed after the rotation"
check "$diag" "no id-token grant, no URL" "a job without the grant got no token URL"
if [ "${HOSTED:-}" = 1 ]; then
  check "$diag" "verified kid=[A-Za-z0-9_-]" "the job verified its token against the published JWKS"
  check "$diag" "idtoken without the grant: 403" "the ungranted job's own token was refused 403"
fi
python3 - "$host_log/idtokens.jsonl" <<'PY' || ok=0
import base64, json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
slots = [r.get("slot") for r in rows if "token" in r]
claims = [json.loads(base64.urlsafe_b64decode(r["token"].split(".")[1] + "==")) for r in rows if "token" in r]
print("mints:", len(rows), "slots:", slots, "kids:", sorted({r["kid"] for r in rows if "kid" in r}))
for c in claims[:1]:
    print("claims:", json.dumps({k: c[k] for k in ("iss", "aud", "sub", "repository", "workflow_ref", "runner_environment", "event_name", "ref")}))
ok = len(slots) >= 2 and slots[0] == 0 and slots[-1] == 1 and all(c["aud"] == "sts.amazonaws.com" for c in claims)
print("PASS tokens under slot A then slot B" if ok else "FAIL tokens under slot A then slot B")
sys.exit(0 if ok else 1)
PY
[ $ok = 1 ] && echo PASS || { echo FAIL; exit 1; }
