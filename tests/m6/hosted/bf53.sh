#!/bin/bash
# BF-53 on graft-m3f (`build/BUILD-PLAN.md`, M6-WP3; `tests/m6/results/bf53.md`), one step per call after the site
# and edge day (`bf53-day.sh`). Each step logs to tests/m6/raw/bf53/<step>.log (a step run again logs to <step>-2.log),
# every line through `scrub`. Tokens and seeds stay in 0600 files under $K and are sent only in headers or bodies.
#
#   bash tests/m6/hosted/bf53.sh <step> [args]
#
# Needs `source tests/am1/lf13/env-m3f.sh`, CLOUDFLARE_*, ~/.graft-m3f (with keys2/ from ruling B), TOOLS_TOKEN_FILE,
# and $B, the bundle's builds: site/, edge/, lifeboat/ (product builds), graftyard and graft-blob, from the run's head.
#
# Steps, in order:
#   release      the `Keys` (customer-signed: ruling B's executor and drain) and an executor-signed `ReleaseUploaded`
#                of the lifeboat's release record (formats.txt = the edge's enabled formats)
#   hold 1|0     the feed hold (`/_bed/feed?hold=`)
#   seed <tag>   `bf53-seed.mjs`: the repository, pushes, the pull request, the lowered archive cap until it archives
#   token        the bucket-scoped R2 read token on graft-m3f-export (ruling C), written as the bundle's feed.json
#   init         `lifeboat init` (LifeboatAdd), a local CA and the lifeboat's certificate, the bundle's config
#   up|down      graft-blob and workerd on the bundle, as orb services (workerd with a high open-file limit: every
#                chain's copy is a Durable Object with its own SQLite file, and ~3,200 chains passed a 1,024 limit)
#   status [c]   the puller's /status (with chain c's held index), halt and trap counts, the last pass lines
#   enroll       `lifeboat enroll` with m3-member's PAT, then the token checks: main edge 401, lifeboat 200, PAT 401,
#                writes 503 (a `Stored` row's hash is lifeboatbed's and the native tests', not re-run here)
#   release-view the `Release` group's lifeboats: the decoder's release and `seen`
#   stop|start   the feed stopped for this orb (the R2 S3 host to 127.0.0.1 in /etc/hosts)
#   leaders snap|read [chains]  members' `/stats` `/log_rows` counts on the repository's chain, release, ids, and the
#                chains named: saved, then the rise on members that led and on the others
#   block|unblock the edge blocked for this orb (the edge host to 127.0.0.1 in /etc/hosts)
#   gh           enrolled `gh pr list` and `gh api` against the lifeboat, and the lag header
#   teardown     LifeboatRemove, the R2 token deleted, every /etc/hosts line of this row removed
set -uo pipefail
cd "$(dirname "$0")/../../.."
STEP=${1:?step}
O=tests/m6/raw/bf53; mkdir -p "$O"
W=$HOME/.graft-m3f; K=$W/bf53; mkdir -p "$K" "$K/boat" "$K/blob"; chmod 700 "$K" "$K/boat"
B=${B:-/tmp/bf53/builds}
CLI=$B/graftyard
C=(--config "$W/install.toml")
A=${CLOUDFLARE_ACCOUNT_ID:?}
BOAT=lifeboat.bf53.test; LP=8445; BP=8792; AP=8793
EDGE=$GRAFT_EDGE; EDGE_HOST=${EDGE#https://}
R2_HOST=$A.r2.cloudflarestorage.com
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "https://api.cloudflare.com/client/v4$1" "${@:2}"; }
scrub() { sed -e "s/$A/<cf-account-id>/g" -e "s/${CF_WORKERS_SUBDOMAIN:?}/<sub>/g" -e "s#$HOME/#~/#g" -e "s#$PWD/#<checkout>/#g"; }
N=$(ls "$O" | grep -c "^$STEP\(-[0-9]*\)\?\.log$" || true)
L="$O/$STEP$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; git rev-parse --short HEAD; cat; date -u +%FT%TZ; } | scrub >"$L"; cat "$L"; }
bed() { curl -s --noproxy '*' -m 120 -H @<(printf 'x-bed-token: %s\n' "$(cat "$BED_TOKEN_FILE")") "$EDGE$1" "${@:2}"; }
admin() { curl -s --noproxy '*' -m 300 "http://127.0.0.1:$AP$1" "${@:2}"; }
lb() { curl -s --noproxy '*' -m 60 --cacert "$K/ca.pem" --resolve "$BOAT:$LP:127.0.0.1" "$@"; }
au() { printf 'authorization: token %s\n' "$(cat "$1")"; }
field() { python3 -c 'import json,sys; print(json.loads(sys.argv[1]).get(sys.argv[2]) or "")' "$1" "$2"; }
release_read() { bed /_bed/identity -X POST -d '{"chain":"release","read":{}}'; }
pub() { python3 - "$1" <<'EOF'
import sys
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives import serialization
k = Ed25519PrivateKey.from_private_bytes(bytes.fromhex(open(sys.argv[1]).read().strip()))
print(k.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw).hex())
EOF
}
# hosts <name> on|off: points <name> at 127.0.0.1 for this orb, so the bundle loses that host and nothing else does.
hosts() {
  # Both families: a name with AAAA records resolves to its IPv6 address past an IPv4-only hosts line.
  if [ "$2" = on ]; then grep -q " $1 # bf53\$" /etc/hosts || printf '127.0.0.1 %s # bf53\n::1 %s # bf53\n' "$1" "$1" | sudo -n tee -a /etc/hosts >/dev/null
  else sudo -n sed -i "/ $1 # bf53\$/d" /etc/hosts; fi
  echo "$1 now resolves to $(getent ahosts "$1" | awk '{print $1}' | sort -u | tr '\n' ' ')"
}

case $STEP in
release)
  { PREV=$(release_read | jq -r .value.last); echo "release last $PREV"
    entry() { echo "$1" >"$K/spec.json"; "$CLI" "${C[@]}" release entry "$K/spec.json" --out "$K/$2.unsigned" --prev "$PREV" "${@:4}" >/dev/null \
      && "$CLI" sign "$K/$2.unsigned" --out "$K/$2.signed" --key "$3" >/dev/null \
      && a=$("$CLI" "${C[@]}" release post "$K/$2.signed" --edge "$EDGE") && echo "$2: $a" && PREV=$(field "$a" digest) && [ -n "$PREV" ]; }
    # The `Keys` once: a rerun after a refused upload finds the group's keys already ruling B's.
    if [ "$(release_read | jq -r .value.keys.executor)" = "$(pub "$W/keys2/executor.seed")" ]; then echo "keys1: already committed"
    else entry "{\"Keys\": {\"executor\": \"$(pub "$W/keys2/executor.seed")\", \"drain\": \"$(pub "$W/keys2/drain.seed")\"}}" keys1 \
      "$W/customer.key" --keys "$W/genesis.signed" || exit 1; fi
    # `FormatSet` order (family, then version), which `FormatSet::file` writes and the group hashes.
    READS=$(api "/accounts/$A/workers/scripts/graft-m3f-edge/settings" | jq -c '.result.bindings[] | select(.name == "FORMATS_ENABLED") | .text | split(",")
      | sort_by(split(".") | map(tonumber)) | unique_by(split(".") | map(tonumber))')
    echo "reads $READS"
    python3 - "$K" "$READS" <<'PY'
import hashlib, json, sys
k, reads = sys.argv[1], json.loads(sys.argv[2])
text = "".join(f"{f}\n" for f in reads)
open(f"{k}/formats.txt", "w").write(text)
rec = {"label": "bf53-lifeboat", "files": [{"name": "formats.txt", "sha256": hashlib.sha256(text.encode()).hexdigest()}]}
open(f"{k}/release.json", "wb").write(json.dumps(rec, sort_keys=True).encode())
PY
    openssl pkeyutl -sign -rawin -inkey "$W/release.key" -in "$K/release.json" -out "$K/release.sig" || exit 1
    sha256sum "$K/release.json" | cut -c1-64 >"$K/release.id"; echo "release $(cat "$K/release.id")"
    entry "{\"ReleaseUploaded\": {\"record\": \"$K/release.json\", \"record_sig\": \"$(od -An -tx1 -v "$K/release.sig" | tr -d ' \n')\",
      \"versions\": [[\"Edge\", \"00000000-0000-4000-8000-0000000000b5\"]], \"reads\": $READS}}" upload "$W/keys2/executor.seed" || exit 1
    release_read | jq -c '{last: .value.last, keys: (.value.keys | {generation, executor: .executor[0:8], drain: .drain[0:8], customer: .customer[0:8]}), uploads: (.value.uploads | if type == "object" then keys else . end)}'
  } 2>&1 | log ;;
hold)
  { bed "/_bed/feed?hold=${2:?1|0}" -X POST; echo; } 2>&1 | log ;;
seed)
  { node tests/m6/hosted/bf53-seed.mjs "${2:?tag}" "$K"; echo "exit $?"; tail -3 "$K/seed.jsonl" | cut -c1-600; } 2>&1 | log ;;
token)
  { ( umask 077
      jq -nc --arg r "com.cloudflare.edge.r2.bucket.${A}_default_graft-m3f-export" '{name: "graft-m3f-bf53-lifeboat-read",
        policies: [{effect: "allow", resources: {($r): "*"}, permission_groups: [{id: "6a018a9f2fc74eb6b293b0c548f38b39"}]}]}' |
        api "/accounts/$A/tokens" -X POST -H 'content-type: application/json' --data-binary @- >"$K/r2token.json" )
    jq -c '{ok: .success, id: (.result.id // "" | .[0:8]), errors: [.errors[]? | {code, message}]}' "$K/r2token.json"
    jq -r '.result.id // empty' "$K/r2token.json" >"$K/r2token.id"
    python3 - "$K" "$R2_HOST" <<'PY'
import hashlib, json, os, sys
k, host = sys.argv[1], sys.argv[2]
t = json.load(open(f"{k}/r2token.json"))["result"]
feed = {"FEED_URL": f"https://{host}", "FEED_BUCKET": "graft-m3f-export", "FEED_KEY_ID": t["id"],
        "FEED_SECRET": hashlib.sha256(t["value"].encode()).hexdigest(), "FEED_REGION": "auto"}
fd = os.open(f"{k}/boat/feed.json", os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
os.write(fd, json.dumps(feed).encode()); os.close(fd)
print("feed.json written (0600)")
PY
  } 2>&1 | log ;;
init)
  { "$CLI" "${C[@]}" lifeboat init --host $BOAT --key "$W/customer.key" --dir "$K/boat" --edge "$EDGE" --genesis "$W/genesis.signed" || exit 1
    if [ ! -s "$K/ca.pem" ]; then
      openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 3 -subj /CN=bf53-ca \
        -keyout "$K/ca.key" -out "$K/ca.pem" 2>/dev/null
      openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj /CN=$BOAT -keyout "$K/tls.key" -out "$K/tls.csr" 2>/dev/null
      printf 'subjectAltName=DNS:%s\n' $BOAT >"$K/san.ext"
      openssl x509 -req -in "$K/tls.csr" -CA "$K/ca.pem" -CAkey "$K/ca.key" -CAcreateserial -days 3 -extfile "$K/san.ext" \
        -out "$K/tls.pem" 2>/dev/null && echo "local CA and certificate for $BOAT"
    fi
    "$CLI" lifeboat bundle --dir "$K/boat" --builds "$B" --release "$(cat "$K/release.id")" --listen 127.0.0.1:$LP \
      --tls-key "$K/tls.key" --tls-chain "$K/tls.pem" --blob http://127.0.0.1:$BP --admin 127.0.0.1:$AP \
      --state "$K/boat/state" --out "$K/boat/config.capnp"
    echo "trustBrowserCas lines: $(grep -c trustBrowserCas "$K/boat/config.capnp")"
  } 2>&1 | log ;;
up)
  { amp orb service start bf53-blob --command "exec $B/graft-blob --root $K/blob --listen 127.0.0.1:$BP" 2>&1 | head -1
    amp orb service start bf53-boat --command "ulimit -n 524288; cd $K/boat && exec $PWD/tests/m0/node_modules/.bin/workerd serve config.capnp --experimental >>$K/workerd.log 2>&1" 2>&1 | head -1
    for _ in $(seq 60); do admin /status -o /dev/null && break; sleep 1; done
    admin /status | jq -c '{decoder: .decoder[0:12], current_at, owed, packs, pages, objects, decoder_at, halted, gone, chains: (.held | length)}'
  } 2>&1 | log ;;
down)
  { amp orb service stop bf53-boat 2>&1 | tail -1; amp orb service stop bf53-blob 2>&1 | tail -1; } 2>&1 | log ;;
status)
  { admin /status | jq -c --arg c "${2:-}" '{decoder: .decoder[0:12], current_at, owed, unserved, unserved_alarm, packs, pages,
      objects, decoder_at, halted, gone, chains: (.held | length), chain: (if $c == "" then null else .held[$c] end)}'
    echo "now $(date +%s%3N)"
    echo "halt lines: $(grep -c "lifeboat halt" "$K/workerd.log")"
    echo "wasm traps: $(grep -cE "out of bounds|RuntimeError" "$K/workerd.log")"
    grep "lifeboat_pass" "$K/workerd.log" | tail -2 | cut -c1-700
  } 2>&1 | log ;;
enroll)
  { ( umask 077; awk '$1 == "member" {printf "%s", $2}' "$W/m3/m3-tokens.txt" >"$K/pat" )
    "$CLI" lifeboat enroll --host $BOAT --edge "$EDGE" --token-file "$K/pat" --gh-config "$K/gh" --out "$K/lifeboat.token" || exit 1
    code() { curl -s --noproxy '*' -m 60 -o /dev/null -w '%{http_code}' "$@"; }
    U=https://$BOAT:$LP; R=$(jq -r .repo "$K/seed.json")
    echo "main edge, lifeboat token: $(code -H @<(au "$K/lifeboat.token") -H 'user-agent: bf53' "$EDGE/api/v3/user") (want 401)"
    echo "main edge, PAT: $(code -H @<(au "$K/pat") -H 'user-agent: bf53' "$EDGE/api/v3/user") (want 200)"
    echo "lifeboat, lifeboat token: $(lb -o /dev/null -w '%{http_code}' -H @<(au "$K/lifeboat.token") -H 'user-agent: bf53' "$U/api/v3/user") (want 200)"
    echo "lifeboat, PAT: $(lb -o /dev/null -w '%{http_code}' -H @<(au "$K/pat") -H 'user-agent: bf53' "$U/api/v3/user") (want 401)"
    echo "lifeboat, REST write: $(lb -o /dev/null -w '%{http_code}' -X POST -H @<(au "$K/lifeboat.token") -H 'user-agent: bf53' "$U/api/v3/repos/$R/issues" -d '{"title":"no"}') (want 503)"
    echo "lifeboat, receive-pack: $(lb -o /dev/null -w '%{http_code}' -X POST -H 'user-agent: git/2.50' "$U/$R.git/git-receive-pack") (want 503)"
  } 2>&1 | log ;;
release-view)
  { release_read | jq -c '{last: .value.last[0:12], lifeboats: (.value.lifeboats // null), now_ms: (now * 1000 | floor)}'
  } 2>&1 | log ;;
stop|start)
  { hosts "$R2_HOST" "$([ "$STEP" = stop ] && echo on || echo off)"; } 2>&1 | log ;;
leaders)
  # The repository's chain, `release`, `ids`, and any chain named (the stale window's edge pulls); every held chain
  # would be ~20,000 tools calls.
  { M=${2:?snap|read}; shift 2
    { jq -r .chain "$K/seed.json"; echo release; echo ids; for c in "$@"; do echo "$c"; done; } | sort -u >"$K/chains.txt"
    node tests/m6/hosted/bf53-leaders.mjs "$K/chains.txt" "$M" "$K/leaders-snap.json"; } 2>&1 | log ;;
block|unblock)
  { hosts "$EDGE_HOST" "$([ "$STEP" = block ] && echo on || echo off)"
    echo "this orb → edge: $(curl -s --noproxy '*' -m 10 -o /dev/null -w '%{http_code}' "$EDGE/api/v3/meta")"; } 2>&1 | log ;;
gh)
  { R=$(jq -r .repo "$K/seed.json")
    grep -q " $BOAT # bf53" /etc/hosts || echo "127.0.0.1 $BOAT # bf53" | sudo -n tee -a /etc/hosts >/dev/null
    export GH_CONFIG_DIR=$K/gh GH_HOST=$BOAT:$LP SSL_CERT_FILE=$K/ca.pem GH_NO_UPDATE_NOTIFIER=1 GH_PROMPT_DISABLED=1
    export NO_PROXY="$NO_PROXY,$BOAT" no_proxy="$NO_PROXY,$BOAT"
    # gh honours HTTPS_PROXY over NO_PROXY for a name with a port: the orb's proxy then fails the loopback name.
    unset GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN HTTPS_PROXY HTTP_PROXY https_proxy http_proxy
    /usr/bin/gh pr list -R "$R" 2>&1 | sed 's/^/gh: /'; echo "gh pr list rc ${PIPESTATUS[0]}"
    /usr/bin/gh api "repos/$R/pulls/1" --jq '{number, title, state}' 2>&1 | sed 's/^/gh api: /'
    lb -D - -o /dev/null -H @<(au "$K/lifeboat.token") -H 'user-agent: bf53' "https://$BOAT:$LP/api/v3/meta" | tr -d '\r' | grep -i '^x-graftyard-lifeboat-lag'
  } 2>&1 | log ;;
teardown)
  # The hosts lines first: a blocked edge refuses the remove.
  { sudo -n sed -i '/# bf53$/d' /etc/hosts; echo "bf53 hosts lines left: $(grep -c "# bf53" /etc/hosts)"
    "$CLI" "${C[@]}" lifeboat remove --host $BOAT --key "$W/customer.key" --edge "$EDGE"
    t=$(cat "$K/r2token.id" 2>/dev/null); [ -n "$t" ] && api "/accounts/$A/tokens/$t" -X DELETE | jq -c '{token_deleted: .success}'
    release_read | jq -c '{lifeboats: (.value.lifeboats // null)}' 
  } 2>&1 | log ;;
*) echo "unknown step $STEP"; exit 1 ;;
esac
