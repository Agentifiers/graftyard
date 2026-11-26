#!/bin/bash
# BM-6's lifeboat line on graft-m6a (`build/BUILD-PLAN.md`, M6-WP4; the combined drill plan's section 6), one step per
# call, as `bf53.sh` ran BF-53's. Each step logs to tests/m6/raw/bm6-lifeboat/<step>.log (a step run again logs to
# <step>-2.log), every line through `scrub`. Keys, tokens and seeds stay in 0600 files under $K and are sent only in
# headers, bodies or a credential helper.
#
#   bash tests/m6/hosted/bm6-lifeboat.sh <step> [args]
#
# Needs CLOUDFLARE_* (the harness's account token, which reads and makes the line's R2 token only), and in $K, from
# M6-WP1 (`.amp/xfer/`, moved here at 0600): install.toml, genesis.signed, pat, owner (the seeded login), and
# M6-WP1's `lifeboat init` output, boat/lifeboat.json and boat/lifeboat.key. graft-m6a's customer key never leaves
# M6-WP1's orb: it runs `lifeboat init` (LifeboatAdd) and `lifeboat remove` (LifeboatRemove), and line 3's two member
# reads with the bench token (`tests/m6/hosted/bm6-leaders.sh`), and sends the output here.
# $B holds the run's builds from R2's head: site/, edge/, lifeboat/ (product), graftyard, graft-blob, and lifeboat-git/
# (containers/git's `lifeboat-git` stage: usr/local and shard-server).
#
# Steps, in the plan's order:
#   files         checks $K's files (0600, the genesis against install.toml) and $B's builds, prints no secret
#   token         the R2 read token, Object Read on graft-m6a-export only, as the bundle's feed.json; lists the bucket
#                 and is refused on a packs bucket
#   bundle <rel>  with M6-WP1's lifeboat.json and lifeboat.key in boat/ (its `lifeboat init` committed LifeboatAdd):
#                 a local CA and certificate, `lifeboat bundle --git` with R2's release.json SHA-256 <rel>
#   up|down       graft-blob and the bundle's start.sh (workerd and graft-git) as orb services
#   status        the puller's /status, the git copy's readiness for bm6, the last pass lines, halt and trap counts
#   enroll        `lifeboat enroll` with the seeded PAT, then the token checks (line 2: the main edge answers 401)
#   block|unblock the edge blocked for this orb (its host to loopback in /etc/hosts)
#   stop|start    the feed stopped for this orb (the R2 S3 host to loopback)
#   clone [tag]   line 4: an enrolled clone of bm6 from the lifeboat, refs against $K/refs-<tag> (`refs`), fsck --strict
#   refs <tag>    bm6's refs through the main edge, saved as $K/refs-<tag> (before a block)
#   push          one real client push to bm6 through the main edge (a stored-size file), its tip saved
#   fetch         line 5: `git fetch` in the clone, then the refs against the last `refs`
#   rebuild       line 6 and line 8: kill -9 graft-git and delete bm6's repository; clones answer 503 with Retry-After
#                 and never a short pack until a clone matches; git-dir bytes sampled every 2 s, peak ÷ repository
#                 bytes against the x3 floor (flagged over 2.5)
#   leaders <snap.jsonl> <read.jsonl>  line 3: judges M6-WP1's two `bm6-leaders.sh` outputs
#   sample        line 7's sampler: the puller's current_at every 5 s into $K/status.jsonl (an orb service)
#   lag           line 7: the pass lines since <ISO> (`LAG_FROM`): cycle times and the worst lag once current
#   gh            the gate: enrolled `gh pr list` and `gh api` against the lifeboat, and the lag header
#   spend         graft-m6a's spend since the line's start
#   teardown      the R2 token deleted, this line's /etc/hosts lines removed, the services stopped; then M6-WP1 runs
#                 `lifeboat remove` (LifeboatRemove) and reads the group's lifeboats from its view
set -uo pipefail
cd "$(dirname "$0")/../../.."
STEP=${1:?step}
O=tests/m6/raw/bm6-lifeboat; mkdir -p "$O"
K=${K:-$HOME/.graft-m6a-lb}; mkdir -p "$K" "$K/boat" "$K/blob" "$K/git"; chmod 700 "$K" "$K/boat"
B=${B:-/tmp/bm6lb/builds}
CLI=$B/graftyard
C=(--config "$K/install.toml")
A=${CLOUDFLARE_ACCOUNT_ID:?}
BOAT=lifeboat.bm6.test; LP=8446; BP=8794; AP=8795; GP=8796
EDGE=${EDGE:-https://graft-m6a-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev}; EDGE_HOST=${EDGE#https://}
R2_HOST=$A.r2.cloudflarestorage.com
WD=$PWD/tests/m0/node_modules/.bin/workerd
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "https://api.cloudflare.com/client/v4$1" "${@:2}"; }
scrub() { sed -e "s/$A/<cf-account-id>/g" -e "s/${CF_WORKERS_SUBDOMAIN:?}/<sub>/g" -e "s#$HOME/#~/#g" -e "s#$PWD/#<checkout>/#g"; }
N=$(ls "$O" | grep -c "^$STEP\(-[0-9]*\)\?\.log$" || true)
L="$O/$STEP$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; git rev-parse --short HEAD; cat; date -u +%FT%TZ; } | scrub >"$L"; cat "$L"; }
admin() { curl -s --noproxy '*' -m 300 "http://127.0.0.1:$AP$1" "${@:2}"; }
lb() { curl -s --noproxy '*' -m 60 --cacert "$K/ca.pem" --resolve "$BOAT:$LP:127.0.0.1" "$@"; }
au() { printf 'authorization: token %s\n' "$(cat "$1")"; }
owner() { cat "$K/owner"; }
REPO_URL() { echo "$1/$(owner)/bm6.git"; }
# git with the PAT (main edge) or the lifeboat token (lifeboat) from a credential helper, never on the command line.
# The local CA and no proxy only for the lifeboat's URL: the main edge keeps the public CAs and the orb's proxy.
gitc() { local tok=$1; shift
  git -c credential.helper= -c "credential.helper=!f() { echo username=$(owner); echo password=\$(cat $tok); }; f" \
    -c "http.https://$BOAT:$LP/.sslCAInfo=$K/ca.pem" -c "http.https://$BOAT:$LP/.proxy=" -c protocol.version=2 "$@"; }
# hosts <name> on|off: points <name> at loopback for this orb only, both families.
hosts() {
  if [ "$2" = on ]; then grep -q " $1 # bm6lb\$" /etc/hosts || printf '127.0.0.1 %s # bm6lb\n::1 %s # bm6lb\n' "$1" "$1" | sudo -n tee -a /etc/hosts >/dev/null
  else sudo -n sed -i "/ $1 # bm6lb\$/d" /etc/hosts; fi
  echo "$1 now resolves to $(getent ahosts "$1" | awk '{print $1}' | sort -u | tr '\n' ' ')"
}
boat_host() { grep -q " $BOAT # bm6lb" /etc/hosts || echo "127.0.0.1 $BOAT # bm6lb" | sudo -n tee -a /etc/hosts >/dev/null; }
net_id() { jq -r .id "$K/repo.json"; }
# refs_of <dir>: a clone's branch refs as "<oid> refs/heads/<b>", sorted by name.
refs_of() { git -C "$1" for-each-ref --format='%(objectname) %(refname)' refs/remotes/origin |
  sed 's#refs/remotes/origin/#refs/heads/#' | grep -v ' refs/heads/HEAD$' | sort -k2; }
latest_refs() { ls -t "$K"/refs-* 2>/dev/null | head -1; }

case $STEP in
files)
  { for f in install.toml genesis.signed pat owner boat/lifeboat.json boat/lifeboat.key; do
      [ -s "$K/$f" ] && echo "$f: $(stat -c %a "$K/$f")" || echo "$f: MISSING"
    done
    d=$(sha256sum "$K/genesis.signed" | cut -c1-64)
    grep -q "^genesis = \"$d\"" "$K/install.toml" && echo "genesis matches install.toml" || echo "FAIL: genesis is not install.toml's"
    for b in site edge lifeboat lifeboat-git/shard-server lifeboat-git/usr/local/bin/git graftyard graft-blob; do
      [ -e "$B/$b" ] && echo "build $b: ok" || echo "build $b: MISSING"
    done
    "$B/lifeboat-git/usr/local/bin/git" --version
    echo "owner $(owner); repo bm6"
    # bm6's id and its names shard, from the main edge.
    curl -s -H @<(au "$K/pat") -H 'user-agent: bm6-lifeboat' "$EDGE/api/v3/repos/$(owner)/bm6" | jq -c '{id, full_name}' | tee "$K/repo.json"
  } 2>&1 | log ;;
token)
  { ( umask 077
      jq -nc --arg r "com.cloudflare.edge.r2.bucket.${A}_default_graft-m6a-export" '{name: "graft-m6a-bm6-lifeboat-read",
        policies: [{effect: "allow", resources: {($r): "*"}, permission_groups: [{id: "6a018a9f2fc74eb6b293b0c548f38b39"}]}]}' |
        api "/accounts/$A/tokens" -X POST -H 'content-type: application/json' --data-binary @- >"$K/r2token.json" )
    jq -c '{ok: .success, id: (.result.id // "" | .[0:8]), errors: [.errors[]? | {code, message}]}' "$K/r2token.json"
    jq -r '.result.id // empty' "$K/r2token.json" >"$K/r2token.id"
    python3 - "$K" "$R2_HOST" <<'PY'
import hashlib, json, os, sys
k, host = sys.argv[1], sys.argv[2]
t = json.load(open(f"{k}/r2token.json"))["result"]
feed = {"FEED_URL": f"https://{host}", "FEED_BUCKET": "graft-m6a-export", "FEED_KEY_ID": t["id"],
        "FEED_SECRET": hashlib.sha256(t["value"].encode()).hexdigest(), "FEED_REGION": "auto"}
fd = os.open(f"{k}/boat/feed.json", os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
os.write(fd, json.dumps(feed).encode()); os.close(fd)
print("feed.json written (0600)")
PY
    # Scope: the export bucket lists, a packs bucket refuses (R2's S3 API, SigV4 by the same keys).
    sleep 10
    python3 - "$K/boat/feed.json" <<'PY'
import datetime, hashlib, hmac, json, sys, urllib.request, urllib.error
f = json.load(open(sys.argv[1]))
def get(bucket):
    host = f["FEED_URL"].split("//")[1]; now = datetime.datetime.utcnow()
    amz, day = now.strftime("%Y%m%dT%H%M%SZ"), now.strftime("%Y%m%d")
    q = "list-type=2&max-keys=1"; ph = hashlib.sha256(b"").hexdigest()
    canon = f"GET\n/{bucket}\n{q}\nhost:{host}\nx-amz-content-sha256:{ph}\nx-amz-date:{amz}\n\nhost;x-amz-content-sha256;x-amz-date\n{ph}"
    scope = f"{day}/auto/s3/aws4_request"
    sts = f"AWS4-HMAC-SHA256\n{amz}\n{scope}\n{hashlib.sha256(canon.encode()).hexdigest()}"
    k = ("AWS4" + f["FEED_SECRET"]).encode()
    for p in (day, "auto", "s3", "aws4_request"): k = hmac.new(k, p.encode(), hashlib.sha256).digest()
    sig = hmac.new(k, sts.encode(), hashlib.sha256).hexdigest()
    auth = f"AWS4-HMAC-SHA256 Credential={f['FEED_KEY_ID']}/{scope}, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature={sig}"
    r = urllib.request.Request(f"https://{host}/{bucket}?{q}", headers={"x-amz-date": amz, "x-amz-content-sha256": ph, "authorization": auth})
    try: return urllib.request.urlopen(r, timeout=30).status
    except urllib.error.HTTPError as e: return e.code
print("graft-m6a-export list:", get("graft-m6a-export"), "(want 200)")
print("graft-m6a-site-a-packs list:", get("graft-m6a-site-a-packs"), "(want 403)")
PY
  } 2>&1 | log ;;
bundle)
  { REL=${2:?R2 release.json SHA-256}; echo "$REL" >"$K/release.id"
    jq -c '{host, edge, key: .key[0:16]}' "$K/boat/lifeboat.json"
    [ "$(jq -r .host "$K/boat/lifeboat.json")" = $BOAT ] || { echo "FAIL: lifeboat.json is not for $BOAT"; exit 1; }
    if [ ! -s "$K/ca.pem" ]; then
      openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 4 -subj /CN=bm6lb-ca \
        -keyout "$K/ca.key" -out "$K/ca.pem" 2>/dev/null
      openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj /CN=$BOAT -keyout "$K/tls.key" -out "$K/tls.csr" 2>/dev/null
      printf 'subjectAltName=DNS:%s\n' $BOAT >"$K/san.ext"
      openssl x509 -req -in "$K/tls.csr" -CA "$K/ca.pem" -CAkey "$K/ca.key" -CAcreateserial -days 4 -extfile "$K/san.ext" \
        -out "$K/tls.pem" 2>/dev/null && echo "local CA and certificate for $BOAT"
    fi
    "$CLI" lifeboat bundle --dir "$K/boat" --builds "$B" --release "$REL" --listen 127.0.0.1:$LP \
      --tls-key "$K/tls.key" --tls-chain "$K/tls.pem" --blob http://127.0.0.1:$BP --admin 127.0.0.1:$AP \
      --state "$K/boat/state" --workerd "$WD" --out "$K/boat/config.capnp" \
      --git "$B/lifeboat-git" --git-data "$K/git" --git-listen 127.0.0.1:$GP --blob-root "$K/blob"
  } 2>&1 | log ;;
up)
  { amp orb service start bm6lb-blob --command "exec $B/graft-blob --root $K/blob --listen 127.0.0.1:$BP" 2>&1 | head -1
    amp orb service start bm6lb-boat --command "cd $K/boat && WORKERD=$WD exec bash $K/boat/start.sh >>$K/workerd.log 2>&1" 2>&1 | head -1
    for _ in $(seq 60); do admin /status -o /dev/null && break; sleep 1; done
    admin /status | jq -c '{decoder: .decoder[0:12], current_at, owed, packs, objects, halted, gone, chains: (.held | length)}'
    curl -s --noproxy '*' "http://127.0.0.1:$GP/health" | jq -c '{git, disk: .disk[0:8], boot: .boot[0:8]}'
  } 2>&1 | log ;;
down)
  { amp orb service stop bm6lb-boat 2>&1 | tail -1; amp orb service stop bm6lb-blob 2>&1 | tail -1; } 2>&1 | log ;;
status)
  { admin /status | jq -c --arg c "repo:$(net_id 2>/dev/null)" '{decoder: .decoder[0:12], current_at, owed, unserved,
      unserved_alarm, packs, objects, halted, gone, chains: (.held | length), bm6: .held[$c]}'
    echo "bm6 git: $(admin "/git?net=$(net_id)")"
    echo "now $(date +%s%3N)"
    echo "halt lines: $(grep -c "lifeboat halt" "$K/workerd.log")"
    echo "wasm traps: $(grep -cE "out of bounds|RuntimeError" "$K/workerd.log")"
    grep "lifeboat_pass" "$K/workerd.log" | tail -2 | cut -c1-900
  } 2>&1 | log ;;
enroll)
  { "$CLI" lifeboat enroll --host $BOAT --edge "$EDGE" --token-file "$K/pat" --gh-config "$K/gh" --out "$K/lifeboat.token" || exit 1
    code() { curl -s --noproxy '*' -m 60 -o /dev/null -w '%{http_code}' "$@"; }
    U=https://$BOAT:$LP; R="$(owner)/bm6"
    echo "line 2, main edge, lifeboat token: $(code -H @<(au "$K/lifeboat.token") -H 'user-agent: bm6' "$EDGE/api/v3/user") (want 401)"
    echo "main edge, PAT: $(code -H @<(au "$K/pat") -H 'user-agent: bm6' "$EDGE/api/v3/user") (want 200)"
    for _ in $(seq 20); do c=$(lb -o /dev/null -w '%{http_code}' -H @<(au "$K/lifeboat.token") -H 'user-agent: bm6' "$U/api/v3/user")
      [ "$c" = 200 ] && break; sleep 15; done
    echo "lifeboat, lifeboat token: $c (want 200; 401 until the feed object with its row arrives)"
    echo "lifeboat, PAT: $(lb -o /dev/null -w '%{http_code}' -H @<(au "$K/pat") -H 'user-agent: bm6' "$U/api/v3/user") (want 401)"
    echo "lifeboat, REST write: $(lb -o /dev/null -w '%{http_code}' -X POST -H @<(au "$K/lifeboat.token") -H 'user-agent: bm6' "$U/api/v3/repos/$R/issues" -d '{"title":"no"}') (want 503)"
  } 2>&1 | log ;;
block|unblock)
  { hosts "$EDGE_HOST" "$([ "$STEP" = block ] && echo on || echo off)"
    echo "this orb → edge: $(curl -s --noproxy '*' -m 10 -o /dev/null -w '%{http_code}' "$EDGE/api/v3/meta")"; } 2>&1 | log ;;
stop|start)
  { hosts "$R2_HOST" "$([ "$STEP" = stop ] && echo on || echo off)"; } 2>&1 | log ;;
refs)
  { T=${2:?tag}
    gitc "$K/pat" ls-remote --heads "$(REPO_URL "$EDGE")" | awk '{print $1, $2}' | sort -k2 >"$K/refs-$T"
    echo "refs-$T: $(wc -l <"$K/refs-$T") branches"; cat "$K/refs-$T"
  } 2>&1 | log ;;
push)
  { W=$K/src; [ -d "$W/.git" ] || gitc "$K/pat" clone -q "$(REPO_URL "$EDGE")" "$W" || exit 1
    git -C "$W" config user.email bm6@lifeboat.test; git -C "$W" config user.name bm6-lifeboat
    git -C "$W" checkout -q main && gitc "$K/pat" -C "$W" pull -q --ff-only origin main
    f="lb-$(date -u +%H%M%S).bin"; head -c $((3 << 20)) /dev/urandom >"$W/$f"
    git -C "$W" add "$f" && git -C "$W" commit -q -m "bm6 lifeboat $f"
    gitc "$K/pat" -C "$W" push -q origin main && echo "pushed $(git -C "$W" rev-parse HEAD) to main ($f, 3 MiB: a stored pack)"
  } 2>&1 | log ;;
clone)
  { boat_host; T=${2:-c1}; ref=$(latest_refs); rm -rf "$K/$T"
    gitc "$K/lifeboat.token" clone -q "$(REPO_URL "https://$BOAT:$LP")" "$K/$T" 2>&1 | tail -3
    echo "clone rc ${PIPESTATUS[0]}"
    if diff <(cat "$ref") <(refs_of "$K/$T"); then echo "refs match $(basename "$ref")"; else echo "FAIL: refs differ from $(basename "$ref")"; fi
    git -C "$K/$T" fsck --no-progress --strict 2>&1 | tail -3; echo "fsck rc ${PIPESTATUS[0]}"
  } 2>&1 | log ;;
fetch)
  { boat_host; T=${2:-c1}; ref=$(latest_refs)
    gitc "$K/lifeboat.token" -C "$K/$T" fetch -q origin 2>&1 | tail -3; echo "fetch rc ${PIPESTATUS[0]}"
    if diff <(cat "$ref") <(refs_of "$K/$T"); then echo "refs match $(basename "$ref")"; else echo "FAIL: refs differ from $(basename "$ref")"; fi
  } 2>&1 | log ;;
rebuild)
  { boat_host; ref=$(latest_refs); NET=$(net_id)
    repo_bytes=$(du -sbc "$K/blob/pack" "$K/boat/state" 2>/dev/null | tail -1 | cut -f1)
    echo "repository bytes (graft-blob packs + the copy's state, as start.sh counts): $repo_bytes"
    ( peak=0; while :; do b=$(du -sb "$K/git" 2>/dev/null | cut -f1); [ "${b:-0}" -gt "$peak" ] && peak=$b; echo "$peak" >"$K/git.peak"; sleep 2; done ) &
    SAMPLER=$!
    gp=$(pgrep -x shard-server | head -1); echo "kill -9 graft-git (pid $gp), rm bm6's repository"
    kill -9 "$gp"; rm -rf "$K/git/$NET.git"
    # start.sh exits when graft-git does; the service restarts it, as a host's supervisor does.
    seen=0; short=0; ok=0
    for i in $(seq 180); do
      code=$(lb -o /dev/null -D "$K/adv.h" -w '%{http_code}' -u "$(owner):$(cat "$K/lifeboat.token")" -H 'user-agent: git/2.56' \
        "https://$BOAT:$LP/$(owner)/bm6.git/info/refs?service=git-upload-pack")
      if [ "$code" = 503 ]; then grep -qi '^retry-after:' "$K/adv.h" && seen=$((seen + 1)) || short=1
      elif [ "$code" = 200 ]; then
        rm -rf "$K/c2"
        if gitc "$K/lifeboat.token" clone -q "$(REPO_URL "https://$BOAT:$LP")" "$K/c2" 2>"$K/c2.err"; then ok=1; break; fi
        grep -q 503 "$K/c2.err" || short=1
      fi
      sleep 5
    done
    kill $SAMPLER 2>/dev/null; peak=$(cat "$K/git.peak")
    echo "503 with Retry-After: $seen; short or untagged answers: $short; clone after rebuild: $ok"
    [ $ok = 1 ] && { diff <(cat "$ref") <(refs_of "$K/c2") && echo "refs match $(basename "$ref")" || echo "FAIL: refs differ"
      git -C "$K/c2" fsck --no-progress --strict >/dev/null 2>&1 && echo "fsck ok" || echo "FAIL: fsck"; }
    python3 -c "p, r = $peak, $repo_bytes; x = p / r if r else 0; print(f'line 8 (R68): peak git dir {p} bytes / repository {r} = {x:.2f} against x3' + ('  FLAG: over 2.5' if x > 2.5 else ''))"
  } 2>&1 | log ;;
leaders)
  # Line 3: M6-WP1's two `bm6-leaders.sh` outputs (fault -5 min and the gate's start). A member that led at either read
  # counts as a leader; its `/log_rows` must not rise. Followers' must.
  { python3 - "${2:?snap.jsonl}" "${3:?read.jsonl}" <<'PY'
import json, sys
def load(p):
    out = {}
    for l in open(p):
        if l.strip():
            r = json.loads(l)
            out[(r["chain"], r["do_name"])] = r
    return out
a, b = load(sys.argv[1]), load(sys.argv[2])
lead = foll = 0
for k, m in sorted(b.items()):
    p = a.get(k, {})
    rise = (m.get("log_rows") or 0) - (p.get("log_rows") or 0)
    led = "leader" in (m.get("role"), p.get("role"))
    if led: lead += rise
    else: foll += rise
    print(k[0], m["site"], k[1], f"{p.get('role')}->{m.get('role')}", "rise", rise)
print(("ok    " if lead == 0 and foll > 0 else "FAIL  ") + f"line 3: /log_rows on leaders {lead}, on followers {foll}")
PY
  } 2>&1 | log ;;
lag)
  # Line 7: the pass lines since LAG_FROM (ISO): every change of current_at, the cycle between changes, and the worst lag
  # once current (now - current_at at each pass after the first change).
  { FROM=$(date -u -d "${LAG_FROM:?ISO}" +%s%3N)
    python3 - "$K/workerd.log" "$K/status.jsonl" "$FROM" <<'PY'
import json, sys
logf, statf, frm = sys.argv[1], sys.argv[2], int(sys.argv[3])
rows = [json.loads(l) for l in open(statf) if l.strip()]
rows = [r for r in rows if r["now"] >= frm]
changes, prev = [], None
for r in rows:
    if r["current_at"] != prev:
        changes.append((r["now"], r["current_at"])); prev = r["current_at"]
cycles = [round((b[1] - a[1]) / 1000) for a, b in zip(changes, changes[1:])]
first = changes[1][0] if len(changes) > 1 else None
worst = max((r["now"] - r["current_at"]) for r in rows if first and r["now"] >= first) if first else None
print(f"status samples {len(rows)}, current_at changes {len(changes)}, cycles s {cycles}")
print(f"worst lag once current: {None if worst is None else round(worst / 1000)} s; judge: current within 300 s, flip over 270 s")
if worst is not None:
    print("line 7: " + ("FAIL (not current within the alarm)" if worst > 300_000 else "FLIP: build probe moved chains" if worst > 270_000 else "pass"))
PY
  } 2>&1 | log ;;
sample)
  # The lag sampler for line 7, as an orb service: the puller's current_at every 5 s into $K/status.jsonl.
  { amp orb service start bm6lb-lag --command "while :; do curl -s --noproxy '*' -m 20 http://127.0.0.1:$AP/status | jq -c --argjson now \"\$(date +%s%3N)\" '{now: \$now, current_at, owed}' >>$K/status.jsonl; sleep 5; done" 2>&1 | head -1
  } 2>&1 | log ;;
gh)
  { boat_host; R="$(owner)/bm6"
    export GH_CONFIG_DIR=$K/gh GH_HOST=$BOAT:$LP SSL_CERT_FILE=$K/ca.pem GH_NO_UPDATE_NOTIFIER=1 GH_PROMPT_DISABLED=1
    export NO_PROXY="${NO_PROXY:-},$BOAT" no_proxy="${NO_PROXY:-},$BOAT"
    # gh honours HTTPS_PROXY over NO_PROXY for a name with a port: the orb's proxy then fails the loopback name.
    unset GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN HTTPS_PROXY HTTP_PROXY https_proxy http_proxy
    /usr/bin/gh pr list -R "$R" 2>&1 | sed 's/^/gh: /'; echo "gh pr list rc ${PIPESTATUS[0]}"
    /usr/bin/gh api "repos/$R/pulls/1" --jq '{number, title, state}' 2>&1 | sed 's/^/gh api: /'
    lb -D - -o /dev/null -H @<(au "$K/lifeboat.token") -H 'user-agent: bm6' "https://$BOAT:$LP/api/v3/meta" | tr -d '\r' | grep -i '^x-graftyard-lifeboat-lag'
  } 2>&1 | log ;;
spend)
  { INSTALL=graft-m6a python3 tests/fanout/m2bcost.py "${2:?from ISO}" "$(date -u +%FT%TZ)" 2>/dev/null | tail -n 1; } 2>&1 | log ;;
teardown)
  # The hosts lines first: a blocked edge refuses the remove.
  { sudo -n sed -i '/# bm6lb$/d' /etc/hosts; echo "bm6lb hosts lines left: $(grep -c "# bm6lb" /etc/hosts)"
    t=$(cat "$K/r2token.id" 2>/dev/null); [ -n "$t" ] && api "/accounts/$A/tokens/$t" -X DELETE | jq -c '{token_deleted: .success}'
    for s in bm6lb-lag bm6lb-boat bm6lb-blob; do amp orb service stop $s 2>&1 | tail -1; done
  } 2>&1 | log ;;
*) echo "unknown step $STEP"; exit 1 ;;
esac
