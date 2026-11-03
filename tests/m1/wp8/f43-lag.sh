#!/usr/bin/env bash
# F43's reproduction on the test install: one follower falls behind by entries whose AppendEntries may take longer
# than CALL_DEADLINE to reach it, then the fault clears and the run watches whether it catches up.
#
#   tests/m1/wp8/f43-lag.sh <out dir> <repo name> <leader site> <faulted site> [pushes, default 6] [blob kB, default 700]
#
# 1. One small push makes sure the repository's group exists; `home.sh <leader>` puts its leader there.
# 2. `fault.sh on <faulted>`: that site's member class fails every call, so the quorum is the other two.
# 3. <pushes> pushes, each one commit adding <blob kB> of random bytes, so each is one entry with an inline pack.
# 4. `fault.sh off`, then 180 s more of `/status` samples: the faulted member's `applied` against the leader's.
# observe.ts samples the group every 2 s throughout, and `--stats` reads every member's `/stats` at the end.
# The repository must exist in ~/.graft-integ/repos.json (tests/m1/integ/seed.ts --repos <name>). Each run pushes
# to its own branch, named after <out dir>. An orb service restarts a command that exits, so a finished run sleeps.
set -u
OUT=${1:?out dir}
NAME=${2:?repo name}
LEADER=${3:?leader site}
FAULTED=${4:?faulted site}
PUSHES=${5:-6}
KB=${6:-700}
[ -f "$OUT/done" ] && exec sleep infinity
HERE=$(cd "$(dirname "$0")" && pwd)
HOST=graft-m1-edge.<workers-subdomain>.workers.dev
ID=$(jq -r ".\"$NAME\"" ~/.graft-integ/repos.json)
CHAIN=repo:$ID
TOK=$(sed -n 2p ~/.graft-integ/tokens.txt | cut -d' ' -f1)
U="https://x:$TOK@$HOST/wp6/$NAME.git"
BRANCH=f43/$(basename "$OUT")
mkdir -p "$OUT"
log() { echo "$(date -u +%FT%T.%3NZ) $*" | tee -a "$OUT/run.log"; }
W=$(mktemp -d)
trap 'rm -rf "$W"; kill "$obs" 2>/dev/null' EXIT
git -C "$W" init -q -b main && git -C "$W" config user.email f43@x && git -C "$W" config user.name f43
push() { # one push of HEAD, timed
  local t0 out
  t0=$(date +%s%3N)
  out=$(git -C "$W" push "$U" "HEAD:refs/heads/$BRANCH" 2>&1)
  log "push $1: exit $? in $(( $(date +%s%3N) - t0 )) ms: $(tail -1 <<<"$out")"
}
log "chain $CHAIN, leader $LEADER, fault on $FAULTED, $PUSHES pushes of $KB kB to $BRANCH"
node "$HERE/observe.ts" --out "$OUT/observe.jsonl" --chains "$CHAIN" &
obs=$!
echo seed >"$W/seed" && git -C "$W" add seed && git -C "$W" commit -qm seed
push seed
bash "$HERE/home.sh" "$LEADER" "$CHAIN" >>"$OUT/run.log" 2>&1
node "$HERE/observe.ts" --stats "$OUT/stats-before.jsonl" --chains "$CHAIN" --reset >>"$OUT/run.log" 2>&1
bash "$HERE/fault.sh" on "$FAULTED" >>"$OUT/run.log" 2>&1
log "fault on $FAULTED"
for i in $(seq 1 "$PUSHES"); do
  head -c $((KB * 1024)) /dev/urandom >"$W/b$i" && git -C "$W" add "b$i" && git -C "$W" commit -qm "b$i"
  push "$i"
done
bash "$HERE/fault.sh" off >>"$OUT/run.log" 2>&1
log "fault off"
sleep 180
log "end"
node "$HERE/observe.ts" --stats "$OUT/stats.jsonl" --chains "$CHAIN" >"$OUT/stats.txt" 2>&1
cat "$OUT/stats.txt"
touch "$OUT/done"
exec sleep infinity
