#!/bin/bash
# The HostedRunnersAppKey rotation row on App 1835008 (graft-m4a, the runner account; tests/m5/results/m5-wp6.md,
# "HostedRunnersAppKey, end to end"): the runbook's in-place rotation (decisions/DECISIONS.md, "`rotate-holders`")
# under live hosted jobs. The new key goes on the App's record first, the runner store's one slot is PATCHed with it,
# graft-runners' next pass mints with it, then the old key comes off. No site day, no new graft-runners version.
#
# graft-runners holds no key between passes: `Live::load` reads HOSTED_RUNNERS_APP_KEY once per pass (the */5 cron or
# a `workflow_job` delivery), and its installation tokens live in that pass only (workers/runners/src/worker.rs). So
# the first pass that starts 4 s after the write (A17 p1) uses the new key; `passes` reads every pass after it.
#
#   bash tests/m5/a17/hrak.sh <tag> <step>
#
# Steps, in order: state; key (a new RSA 2048 key, PKCS#8, in ~/.graft-m4a/hosted/app.pem, 0600); add (key 2 on the
# record beside key 1, then `mint` with it); repo; jobs start; put (the store write, timed by its Date header); passes
# (after at least one full pass); only (key 1 off, refused until the put is 600 s old, an old App JWT's lifetime, and a
# clean pass followed it); passes again; jobs stop; state; teardown (the row repository deleted). One log per step
# under tests/m5/results/raw/hrak/<tag>/, through `scrub`. No step prints a key or token.
#
# Needs `source tests/am1/lf13/env-m4a.sh`, TOOLS_TOKEN_FILE, the runner account's CLOUDFLARE_RUNNER_ACCOUNT_ID,
# CLOUDFLARE_RUNNER_API_TOKEN and CF_RUNNER_WORKERS_SUBDOMAIN, and APPKEYS (tests/m3/hosted/tools' `appkeys`).
set -uo pipefail
cd "$(dirname "$0")/../../.."
TAG=${1:?tag}; STEP=${2:?step}
O=tests/m5/results/raw/hrak/$TAG; mkdir -p "$O"
H=$HOME/.graft-m4a/hosted; mkdir -p "$H"; chmod 700 "$H"
S=$O/.state; mkdir -p "$S"
RB="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_RUNNER_ACCOUNT_ID"
ra() { curl -s -H "Authorization: Bearer $CLOUDFLARE_RUNNER_API_TOKEN" "$@"; }
scrub() { sed -e "s/$CLOUDFLARE_RUNNER_ACCOUNT_ID/<runner-account>/g" -e "s/${CF_RUNNER_WORKERS_SUBDOMAIN:-none}/<runner-sub>/g" -e "s/${CF_WORKERS_SUBDOMAIN:-none}/<sub>/g" -e "s#$HOME/#~/#g"; }
N=$(ls "$O" | grep -c "^$STEP" || true)
L="$O/$STEP$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; git rev-parse --short HEAD; cat; } | scrub >"$L"; cat "$L"; }
STORE=$(ra "$RB/secrets_store/stores" | jq -r '.result[] | select(.name == "graft-runners") | .id')
secret() { ra "$RB/secrets_store/stores/$STORE/secrets?per_page=100" | jq -c '.result[] | select(.name == "HostedRunnersAppKey")'; }
# Cloudflare's clock, in ms: the orb's ran 28 s behind it (tests/m5/results/raw/bf46r45/r1/clock.txt).
cfms() { echo $(( $(date -d "$(curl -sI https://api.cloudflare.com/client/v4/ | tr -d '\r' | sed -n 's/^[Dd]ate: //p')" +%s) * 1000 )); }
spki() { openssl pkey -in "$1" -pubout -outform DER 2>/dev/null | sha256sum | cut -c1-16; }
node_() { node tests/m5/a17/hrak.mjs "$@"; }
# Error lines: every `graft-runners: ` line but the RunnerBox's "launched N s after arm" (workers/runners/box.mjs), its
# one info line under that prefix. r1's first `only` was refused on that line, which a pass with the new key wrote.
errs() { grep 'graft-runners: ' "$1" | grep -vc 'graft-runners: launched '; }
passes_since() {  # every graft-runners pass line and error line from $1 (Cloudflare ms) to now
  CLOUDFLARE_ACCOUNT_ID=$CLOUDFLARE_RUNNER_ACCOUNT_ID CLOUDFLARE_API_TOKEN=$CLOUDFLARE_RUNNER_API_TOKEN \
    MATCH=graft-runners KEEP=graft-runners python3 tests/m5/bf46/logs.py graft-runners "$1" "$(cfms)"
}

case $STEP in
state)
  { echo "orb minus Cloudflare: $(( $(date +%s) - $(cfms) / 1000 )) s"
    ra "$RB/workers/scripts/graft-runners/versions?per_page=100" | jq -c '{versions: (.result.items | length)}'
    ra "$RB/workers/scripts/graft-runners/deployments" | jq -c '.result.deployments[0] | {created_on, versions: [.versions[] | "\(.version_id[0:8]) \(.percentage)"]}'
    ra "$RB/workers/scripts/graft-runners/schedules" | jq -c '{cron: [.result.schedules[].cron]}'
    secret | jq -c '{name, created, modified}'
    node_ show
    echo "passes, last 15 min:"; passes_since $(( $(cfms) - 900000 )) | tail -4
  } 2>&1 | log ;;
key)
  { [ -e "$H/app.pem" ] && { echo "refused: $H/app.pem exists"; exit 1; }
    ( umask 077; openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$H/app.pem" 2>/dev/null )
    openssl pkey -in "$H/app.pem" -pubout -outform DER -out "$S/spki.der" 2>/dev/null
    echo "new key: RSA 2048 PKCS#8, SPKI sha256 $(spki "$H/app.pem")"
  } 2>&1 | log ;;
add)
  { node_ add 2 "$S/spki.der" && node_ mint "$H/app.pem"; } 2>&1 | log ;;
repo)
  node_ repo 2>&1 | log ;;
jobs)
  case ${3:?start|stop} in
    start) rm -f "$S/stop"
      amp orb service start hrak-jobs --command "cd $PWD && source tests/am1/lf13/env-m4a.sh && export TOOLS_TOKEN_FILE=$TOOLS_TOKEN_FILE && exec node tests/m5/a17/hrak.mjs jobs $PWD/$O $PWD/$S/stop >>$PWD/$O/jobs.out 2>&1" 2>&1 | head -1 | log ;;
    stop) touch "$S/stop"; echo "stop set; the loop ends once every run it dispatched is done" | log ;;
  esac ;;
put)
  # The runbook's in-place write over the one slot, as `graftyard runners init` writes it (cli/src/cf.rs, put_secret).
  # The lead's condition: the private half stays in ~/.graft-m4a/hosted/app.pem (0600), byte-equal to the value
  # PATCHed, and the record lists key 2 beside key 1, so a failed write leaves graft-runners on key 1.
  { [ "$(stat -c %a "$H/app.pem" 2>/dev/null)" = 600 ] || { echo "refused: $H/app.pem missing or not mode 0600"; exit 1; }
    keys=$(node_ show | jq -c '[.keys[] | "\(.id) \(.spki_sha256)"]')
    [ "$keys" = "[\"1 31e2cc5bbbbd0dd9\",\"2 $(spki "$H/app.pem")\"]" ] || { echo "refused: the record lists $keys"; exit 1; }
    body=$(mktemp -p "$H"); chmod 600 "$body"
    jq -Rsc '{value: ., comment: "HostedRunnersAppKey rotation row"}' "$H/app.pem" >"$body"
    jq -j .value "$body" | cmp -s - "$H/app.pem" || { shred -u "$body"; echo "refused: the body is not app.pem's bytes"; exit 1; }
    echo "app.pem 0600, $(wc -c <"$H/app.pem") bytes, byte-equal to the PATCH value; record keys $keys"
    id=$(secret | jq -r .id); hdr=$(mktemp)
    ok=$(ra -D "$hdr" "$RB/secrets_store/stores/$STORE/secrets/$id" -X PATCH -H 'content-type: application/json' --data-binary @"$body" |
      jq -c '{patch: "HostedRunnersAppKey", ok: .success, errors: [.errors[]?.code]}'); shred -u "$body"
    echo "$ok"
    d=$(tr -d '\r' <"$hdr" | sed -n 's/^[Dd]ate: //p'); rm -f "$hdr"
    echo "$ok" | jq -e .ok >/dev/null || { echo "stop: the PATCH failed; key 1 stays listed, do not run only"; exit 1; }
    echo $(( $(date -d "$d" +%s) * 1000 )) >"$S/put.ms"
    echo "written: answer Date $d; key SPKI sha256 $(spki "$H/app.pem")"
    secret | jq -c '{name, modified}'
  } 2>&1 | log ;;
passes)
  { p=$(cat "$S/put.ms"); passes_since "$p" | tee "$S/passes.jsonl"
    n=$(grep -c '"graft-runners pass:' "$S/passes.jsonl"); e=$(errs "$S/passes.jsonl")
    echo "since the put ($(date -u -d @$((p / 1000)) +%TZ)): $n passes, $e error lines"
  } 2>&1 | log ;;
only)
  { p=$(cat "$S/put.ms"); age=$(( ($(cfms) - p) / 1000 ))
    [ "$age" -lt 600 ] && { echo "refused: the put is $age s old, under an old App JWT's 600 s"; exit 1; }
    passes_since "$p" >"$S/passes.jsonl"
    grep -q '"graft-runners pass:' "$S/passes.jsonl" && [ "$(errs "$S/passes.jsonl")" = 0 ] ||
      { echo "refused: no clean pass since the put"; cat "$S/passes.jsonl"; exit 1; }
    node_ only 2 && node_ mint "$H/app.pem"
  } 2>&1 | log ;;
teardown)
  { touch "$S/stop"; amp orb service stop hrak-jobs 2>&1 | tail -1
    node_ unrepo
    rm -rf "$S"; echo "the new key stays: App 1835008's only key and HostedRunnersAppKey's value, private half in ~/.graft-m4a/hosted/app.pem"
  } 2>&1 | log ;;
*) echo "step: state key add repo jobs put passes only teardown"; exit 2 ;;
esac
