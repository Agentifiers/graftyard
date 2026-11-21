#!/usr/bin/env bash
# The Workers Cache API probe behind `colo-gate` (tests/am1/results/b034-colo.md): scratch Workers on the account's
# workers.dev, the hostname kind the test edge uses, never a site day. `probe.sh run` deploys graft-m1-coloprobe
# (probe.js), a second copy graft-m1-coloprobe2, a front graft-m1-coloprobe-front (front.js) that reaches the probe
# over a service binding as graft-front reaches the edge, and graft-m1-r2probe (r2probe.js, read-only on
# graft-m1-packs-a), runs every measurement, and prints them. `probe.sh delete` deletes all four. Needs
# CLOUDFLARE_ACCOUNT_ID, CLOUDFLARE_API_TOKEN and CF_WORKERS_SUBDOMAIN; the probe's token stays in $W/tok (0600) and
# is never printed.
set -uo pipefail
D=$(cd "$(dirname "$0")" && pwd)
W=${W:-/tmp/coloprobe-run}
CF=https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID:?}
H="Authorization: Bearer ${CLOUDFLARE_API_TOKEN:?}"
SUB=${CF_WORKERS_SUBDOMAIN:?}
P=graft-m1-coloprobe

put() { # script module.js bindings-json
  printf '{"main_module":"%s","compatibility_date":"2026-10-01","bindings":%s}' "$(basename "$2")" "$3" >"$W/meta.json"
  curl -s -X PUT -H "$H" "$CF/workers/scripts/$1" -F "metadata=@$W/meta.json;type=application/json" \
    -F "$(basename "$2")=@$2;type=application/javascript+module" | jq -c "{script:\"$1\",success,errors}"
  curl -s -X POST -H "$H" -H 'content-type: application/json' "$CF/workers/scripts/$1/subdomain" \
    -d '{"enabled":true}' >/dev/null
  rm -f "$W/meta.json"
}
ready() { for _ in $(seq 30); do [ "$(curl -s -o /dev/null -w '%{http_code}' "$1/?t=$T")" = 200 ] && return; sleep 2; done; }
pcts() { python3 -c '
import json,sys
v=sorted(json.load(sys.stdin)); p=lambda q: v[min(len(v)-1,int(q/100*len(v)))]
print(f"n {len(v)} p50 {p(50)} p90 {p(90)} p99 {p(99)} max {v[-1]}")'; }

run() {
  mkdir -p "$W" && umask 077
  [ -s "$W/tok" ] || openssl rand -hex 16 | tr -d '\n' >"$W/tok"
  T=$(cat "$W/tok")
  sec=$(printf '[{"type":"secret_text","name":"TOKEN","text":"%s"}]' "$T")
  put $P "$D/probe.js" "$sec"
  put $P"2" "$D/probe.js" "$sec"
  put $P-front "$D/front.js" "[{\"type\":\"service\",\"name\":\"EDGE\",\"service\":\"$P\"}]"
  put graft-m1-r2probe "$D/r2probe.js" \
    "$(printf '[{"type":"secret_text","name":"TOKEN","text":"%s"},{"type":"r2_bucket","name":"BUCKET","bucket_name":"graft-m1-packs-a"}]' "$T")"
  U=https://$P.$SUB.workers.dev U2=https://$P"2".$SUB.workers.dev F=https://$P-front.$SUB.workers.dev
  R=https://graft-m1-r2probe.$SUB.workers.dev
  for u in "$U" "$U2" "$F" "$R"; do ready "$u"; done
  echo "== 10 x 100 put-then-match rounds, 200-byte entries"
  : >"$W/bench.jsonl"
  for _ in $(seq 10); do curl -s "$U/bench?t=$T&n=100&size=200" >>"$W/bench.jsonl"; echo >>"$W/bench.jsonl"; done
  jq -s -c '{colos:(map(.colo)|unique), n:(map(.n)|add), hits:(map(.hits)|add)}' "$W/bench.jsonl"
  for f in put match miss; do printf '%-6s ' "$f"; jq -s -c "map(.$f)|add" "$W/bench.jsonl" | pcts; done
  echo "== one put, 300 matches from other isolates"
  K=x$RANDOM; curl -s "$U/put?t=$T&k=$K" >/dev/null
  for _ in $(seq 6); do seq 50 | xargs -P 25 -I{} curl -s "$U/match?t=$T&k=$K"; done |
    jq -s -c '{n:length, hits:(map(select(.hit))|length), isolates:(map(.iso)|unique|length), ms:(map(.ms)|sort|.[(length*99/100|floor)])}'
  echo "== max-age=3, matched every 1.5 s"
  K=t$RANDOM; curl -s "$U/put?t=$T&k=$K&ma=3" >/dev/null
  for _ in $(seq 6); do curl -s "$U/match?t=$T&k=$K" | jq -c '{hit,age}'; sleep 1.5; done
  echo "== delete, then 20 matches"
  K=d$RANDOM; curl -s "$U/put?t=$T&k=$K" >/dev/null; curl -s "$U/delete?t=$T&k=$K" | jq -c '{gone}'
  seq 20 | xargs -P 10 -I{} curl -s "$U/match?t=$T&k=$K" | jq -s -c '{n:length, hits:(map(select(.hit))|length)}'
  echo "== key on the request's host, put direct, matched through the binding"
  K=h$RANDOM; curl -s "$U/put?t=$T&k=$K" >/dev/null; curl -s "$F/match?t=$T&k=$K" | jq -c '{hit}'
  echo "== key on a fixed origin: through the binding, and from a second script; default and named cache"
  for N in "" 1; do
    K=s$RANDOM; O=https://gate.graftyard.invalid
    curl -s "$U/put?t=$T&k=$K&o=$O&named=$N" >/dev/null
    echo "named=${N:-0} front=$(curl -s "$F/match?t=$T&k=$K&o=$O&named=$N" | jq .hit)" \
      "other-script=$(curl -s "$U2/match?t=$T&k=$K&o=$O&named=$N" | jq .hit)"
  done
  echo "== chain prefixes in graft-m1-packs-a: one list against each .pack's 12-byte check, 4 at a time"
  for n in $(curl -s "$R/nets?t=$T" | jq -r '.prefixes[]' | cut -d/ -f2); do
    curl -s "$R/bench?t=$T&net=$n&n=20" | python3 -c '
import json,sys
d=json.load(sys.stdin); p=lambda v,q: sorted(v)[min(len(v)-1,int(q/100*len(v)))]
print(d["prefix"],"packs",d["packs"],"etags agree",d["etagsAgree"],"list p50/p99",p(d["list"],50),p(d["list"],99),
      "checks p50/p99",p(d["checks"],50),p(d["checks"],99))'
  done
}

delete() {
  for s in $P-front $P"2" $P graft-m1-r2probe; do
    curl -s -X DELETE -H "$H" "$CF/workers/scripts/$s?force=true" | jq -c "{script:\"$s\",deleted:.success}"
  done
}

"${1:?run|delete}"
