#!/usr/bin/env bash
# F43's soak on the test install: load with large inline packs and small pushes on several repositories, under a
# rotating chaos schedule, while observe.ts samples every member and the peer-call capture (`/stats` `peers`) is read
# every cycle. Each cycle is one line of `cycles.tsv`; a cycle that leaves a member behind its leader writes that
# member's and its leader's `/stats` to `stuck-<cycle>.json`. The run resumes at the next cycle after a reboot.
#
#   tests/m1/wp8/f43-soak.sh <out dir> <minutes> <repo names, comma-separated>
#
# Per cycle (about 4 minutes): leaders moved to a site (cycle mod 3), then one chaos step (cycle mod 4: none; site
# fault on a follower site; gray 30% fault on the leader's site; a follower's member reset through
# `/_bed/kill`), during which each repository takes 2 pushes of a 700 kB blob and 4 small pushes; then the fault
# clears and the run waits 60 s and checks every follower's stored log (`last`) against its leader's commit index.
# (A follower's `applied` may trail by one entry while the group is idle: it learns the commit index from the next
# append.) Each cycle reads every member's `/stats` with `reset=1`, so `stats-<cycle>.jsonl` holds that cycle's calls.
set -u
OUT=${1:?out dir}
MINUTES=${2:?minutes}
REPOS=${3:?repo names}
HERE=$(cd "$(dirname "$0")" && pwd)
HOST=graft-m1-edge.<workers-subdomain>.workers.dev
EDGE=https://graft-m1-wp7-edge.<workers-subdomain>.workers.dev
TOK=$(sed -n 2p ~/.graft-integ/tokens.txt | cut -d' ' -f1)
MT=$(cat "$HERE/../../m0/.m0-token")
B=$(cat ~/.graft-wp6/bed.token)
mkdir -p "$OUT"
[ -f "$OUT/done" ] && exec sleep infinity
[ -f "$OUT/start" ] || date +%s >"$OUT/start"
END=$(( $(cat "$OUT/start") + MINUTES * 60 ))
log() { echo "$(date -u +%FT%T.%3NZ) $*" | tee -a "$OUT/soak.log"; }
CHAINS=$(for r in ${REPOS//,/ }; do printf 'repo:%s,' "$(jq -r ".\"$r\"" ~/.graft-integ/repos.json)"; done)
CHAINS=${CHAINS%,}
W=$(mktemp -d)
trap 'rm -rf "$W"; kill "$obs" 2>/dev/null; bash "$HERE/fault.sh" off >>"$OUT/soak.log" 2>&1' EXIT
node "$HERE/observe.ts" --out "$OUT/observe.jsonl" --chains "$CHAINS" --every 5000 &
obs=$!
status() { # chain site -> role, last, commit (or -); a failed read is retried once
  local v
  v=$(status1 "$1" "$2")
  [ "${v#- }" = "$v" ] || { sleep 3; v=$(status1 "$1" "$2"); }
  echo "$v"
}
status1() {
  curl -s -m 15 -X POST -H "Authorization: Bearer $MT" "$EDGE/member" -d "{\"chain\":\"$1\",\"site\":$2,\"path\":\"/status\"}" |
    jq -r '.v | "\(.role // "-") \(.last // "-") \(.commit // "-")"' 2>/dev/null || echo "- - -"
}
member_stats() { # chain site [query]
  curl -s -m 20 -X POST -H "Authorization: Bearer $MT" "$EDGE/member" -d "{\"chain\":\"$1\",\"site\":$2,\"path\":\"/stats${3:-}\",\"deadline\":10000}" |
    jq -c --arg c "$1" --argjson s "$2" '{chain: $c, site: $s, at: now, roundMs: (.v.roundMs[-20:]), failedCalls: .v.failedCalls, peers: .v.peers}'
}
pushes() { # repo name, cycle: 2 big and 4 small pushes, each its own commit on the repo's soak branch
  local name=$1 c=$2 d=$W/$1 u="https://x:$TOK@$HOST/wp6/$1.git" t0 rc
  [ -d "$d" ] || { git init -q -b main "$d"; git -C "$d" config user.email f43@x; git -C "$d" config user.name f43; }
  for k in b1 s1 s2 b2 s3 s4; do
    case $k in b*) head -c 716800 /dev/urandom >"$d/$k" ;; *) echo "$c $k $RANDOM" >"$d/$k" ;; esac
    git -C "$d" add "$k" && git -C "$d" commit -qm "$c $k"
    t0=$(date +%s%3N)
    git -C "$d" push -q "$u" "+HEAD:refs/heads/f43/soak" >/dev/null 2>&1
    rc=$?
    printf '%s\t%s\t%s\t%s\t%s\n' "$(date -u +%FT%TZ)" "$c" "$name" "$k" "$rc $(( $(date +%s%3N) - t0 ))" >>"$OUT/pushes.tsv"
  done
}
c=$(( $(wc -l <"$OUT/cycles.tsv" 2>/dev/null || echo 0) ))
while [ "$(date +%s)" -lt "$END" ]; do
  c=$((c + 1))
  lead=$(( c % 3 ))
  follower=$(( (lead + 1 + (c / 3) % 2) % 3 ))
  bash "$HERE/home.sh" "$lead" "$CHAINS" >>"$OUT/soak.log" 2>&1
  case $(( c % 4 )) in
    0) chaos=none ;;
    1) chaos="fault $follower"; bash "$HERE/fault.sh" on "$follower" >>"$OUT/soak.log" 2>&1 ;;
    2) chaos="gray $lead"; FAULT_RATE=0.3 bash "$HERE/fault.sh" on "$lead" >>"$OUT/soak.log" 2>&1 ;;
    3) chaos="reset $follower"
       for r in ${REPOS//,/ }; do
         id=$(jq -r ".\"$r\"" ~/.graft-integ/repos.json)
         curl -s -m 20 -X POST -H "Authorization: Bearer $MT" "$EDGE/member" \
           -d "{\"chain\":\"repo:$id\",\"site\":$follower,\"path\":\"/reset\",\"body\":{}}" >/dev/null
       done ;;
  esac
  log "cycle $c: leaders to $lead, $chaos"
  jobs=()
  for r in ${REPOS//,/ }; do pushes "$r" "$c" & jobs+=($!); done
  wait "${jobs[@]}"
  bash "$HERE/fault.sh" off >>"$OUT/soak.log" 2>&1
  sleep 60
  behind=""
  for ch in ${CHAINS//,/ }; do
    read -r -a s0 <<<"$(status "$ch" 0)"; read -r -a s1 <<<"$(status "$ch" 1)"; read -r -a s2 <<<"$(status "$ch" 2)"
    last=("${s0[1]}" "${s1[1]}" "${s2[1]}")
    commit=("${s0[2]}" "${s1[2]}" "${s2[2]}")
    l=-1
    for s in 0 1 2; do v="s$s[0]"; [ "${!v}" = leader ] && l=$s; done
    [ "$l" = -1 ] && { behind="$behind $ch:noleader"; continue; }
    for s in 0 1 2; do
      [ "$s" = "$l" ] && continue
      if [ "${last[$s]}" = - ] || [ "${last[$s]}" -lt "${commit[$l]}" ]; then
        behind="$behind $ch:$s@${last[$s]}<${commit[$l]}"
        { member_stats "$ch" "$s"; member_stats "$ch" "$l"; } >>"$OUT/stuck-$c.json"
      fi
    done
  done
  for ch in ${CHAINS//,/ }; do for s in 0 1 2; do member_stats "$ch" "$s" "?reset=1"; done; done >>"$OUT/stats-$c.jsonl"
  printf '%s\t%s\t%s\t%s\n' "$(date -u +%FT%TZ)" "$c" "$chaos" "${behind:-ok}" >>"$OUT/cycles.tsv"
  log "cycle $c done: ${behind:-every member caught up}"
done
touch "$OUT/done"
log "soak done after $c cycles"
exec sleep infinity
