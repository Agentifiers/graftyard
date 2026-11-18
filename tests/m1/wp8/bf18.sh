#!/usr/bin/env bash
# BF-18 on the hosted test install with a real `ChainLog`: "Archive a range of inline packs, kill all shards,
# rehydrate from markers and the log; a second shard behind its IngestCursor serves newer pushes. Pass: git fsck
# clean, every ref clones, second shard serves the newer pushes."
#
# Everything goes through graft-m1-edge on the real groups: pushes are unmodified `git push` over smart HTTP (the
# edge stages, validates on the home shard, places, and commits a `Push` on the repository's Raft group); the archive
# is the edge's test route `/_bed/archive`, which has the leader record real `Promoted` and `Archived` entries
# (`graftyard_raft::Member::record`, `RepoEvent::Promoted`/`Archived`) after the home shard's promotion and the
# archive object's two puts; each member then truncates its stored log through the range. Shards are the site
# scripts' `GitShard` class: the home shard (site 0) and site 1's shard of the same network, reached with the bed's
# `x-graft-shard: 1` header, which the edge honors only with the bed token.
#
# Usage: bf18.sh <workdir> <owner/repo> <repo id>   (tokens from ~/.graft-integ/tokens.txt, bed token ~/.graft-wp6)
set -uo pipefail
W=${1:?workdir} REPO=${2:?owner/repo} ID=${3:?repo id}
HOST=graft-m1-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev
TOK=$(sed -n 2p ~/.graft-integ/tokens.txt | cut -d' ' -f1)
BED=$(cat ~/.graft-wp6/bed.token)
U="https://x:$TOK@$HOST/$REPO.git"
mkdir -p "$W" && cd "$W" || exit 2
rm -rf src clones steps.jsonl && : > steps.jsonl
fails=0
g() { git -c user.name=bf18 -c user.email=bf18@graftyard.invalid -c credential.helper= -c init.defaultBranch=main "$@"; }
bed() { curl -sS --max-time 900 -X POST -H "x-bed-token: $BED" "https://$HOST/_bed/$1?repo=$ID${2:-}"; }
step() { printf '{"step":"%s","out":%s}\n' "$1" "$(echo "${2:-null}" | jq -c . 2>/dev/null || jq -Rn --arg s "$2" '$s')" >> steps.jsonl
  printf '%-34s %s\n' "$1" "$(echo "$2" | head -c 300)"; }
fail() { fails=$((fails + 1)); step "FAIL $1" "\"$2\""; }
push() { # ref; pushes HEAD to it
  local out; out=$(g -C src push --porcelain "$U" "HEAD:refs/heads/$1" 2>&1)
  if grep -q '^\(=\|\*\| \)	' <<<"$out" || grep -q "^[ *+=]	" <<<"$out"; then step "push $1" "\"$(git -C src rev-parse --short HEAD)\"";
  else fail "push $1" "$(tr '\n' ' ' <<<"$out" | head -c 300)"; fi
}
commit() { # branch n
  g -C src checkout -q "$1" 2>/dev/null || g -C src checkout -q -b "$1"
  for i in $(seq 1 "$2"); do
    printf '%s %s %s\n' "$1" "$i" "$RANDOM$RANDOM" >> "src/$1.txt"
    seq 1 $((i * 7)) | sed "s/^/$1-$i-/" > "src/lines-$1.txt"
    g -C src add -A && g -C src commit -q -m "$1 $i"
  done
}
idx() { bed log | jq '[.[] | select(.committed)] | max_by(.index) | .index'; }
RUN=bf18-$(date +%s)
echo "client: $(git --version), run $RUN, repo $REPO ($ID)"
g init -q src
# Refs under a fresh prefix, so the repository's earlier refs (none for a fresh repository) stay out of it.
P=$RUN

# 1. Pushes: small ones (inline packs), one over 1 MiB of random data (a stored pack with markers), more small ones.
commit main 3 && push "$P/main"
commit topic-a 2 && push "$P/topic-a"
commit main 2 && push "$P/main"
g -C src checkout -q main && g -C src merge -q --no-ff -m merge topic-a && push "$P/main"
g -C src checkout -q main && head -c 1500000 /dev/urandom > src/blob.bin && g -C src add blob.bin && g -C src commit -q -m "big blob" && push "$P/main"
commit topic-b 2 && push "$P/topic-b"
step "log before archive" "$(bed log | jq -c '[.[] | {i: .index, k: .kind, p: (.packs | map(if .stored then "s" else "i" end) | join(""))}]')"
TO=$(idx)
step "commit index before archive" "$TO"
# The second shard (site 1) serves the network before the archive and keeps its cursor there.
step "catchup shard 1" "$(bed catchup '&shard=1' | jq -c '{rehydrated: (.rehydrated != null), entries, packs, cursor}')"
step "catchup shard 0" "$(bed catchup | jq -c '{rehydrated: (.rehydrated != null), entries, packs, cursor}')"

# 2. Archive 1..TO: the range's inline packs promoted into one stored pack (`Promoted`), the archive object in two
# buckets, then `Archived`, after which every member truncates its stored log through TO.
A=$(bed archive "&from=1&to=$TO")
step "archive 1-$TO" "$A"
echo "$A" | jq -e '.archived_index' >/dev/null || fail "archive" "$A"
sleep 5
step "log after archive" "$(bed log | jq -c '[.[] | {i: .index, k: .kind}] | {first: .[0], n: length}')"
step "members after archive" "$(bed audit | jq -c '[.[] | {site, base: .r.base, commit: .r.commit, applied: .r.applied}]')"

# 3. Pushes after the archive, one of them a stored pack (over 1 MiB): rehydrate finds it by its marker or, past
# the last `Promoted`, by the log. (A stored pack whose marker write is killed is BF-19's, on the same leader path.)
commit main 2 && push "$P/main"
commit topic-c 2 && push "$P/topic-c"
g -C src checkout -q topic-a && head -c 1200000 /dev/urandom > src/blob-a.bin && g -C src add blob-a.bin && g -C src commit -q -m "big blob a"
push "$P/topic-a"
step "catchup shard 0 after" "$(bed catchup | jq -c '{rehydrated: (.rehydrated != null), entries, packs, cursor}')"

# 4. Kill both shards' containers, then the home shard rehydrates from markers and the log after the last Promoted.
step "kill shard 0" "$(bed kill)"
step "kill shard 1" "$(bed kill '&shard=1')"
step "shard 0 after kill" "$(bed shard_status)"
R=$(bed catchup)
step "rehydrate shard 0" "$(echo "$R" | jq -c '{rehydrated: .rehydrated, entries, packs, cursor}')"
echo "$R" | jq -e '.rehydrated != null' >/dev/null || fail "rehydrate shard 0" "no rehydrate"
F=$(bed fsck); step "fsck shard 0" "$F"
echo "$F" | jq -e '.ok == true' >/dev/null || fail "fsck shard 0" "$F"

# 5. Newer pushes while shard 1 is down and behind its cursor (it caught up last before the archive).
commit main 1 && push "$P/main"
commit topic-d 1 && push "$P/topic-d"
R=$(bed catchup '&shard=1')
step "shard 1 behind its cursor" "$(echo "$R" | jq -c '{rehydrated: .rehydrated, entries, packs, cursor}')"
commit topic-d 1 && push "$P/topic-d"
R=$(bed catchup '&shard=1')
step "shard 1 catches up" "$(echo "$R" | jq -c '{rehydrated: (.rehydrated != null), entries, packs, cursor}')"
F=$(bed fsck '&shard=1'); step "fsck shard 1" "$F"
echo "$F" | jq -e '.ok == true' >/dev/null || fail "fsck shard 1" "$F"

# 6. Every ref clones from each shard, v2 and v0, and matches the leader's refs; every branch alone from shard 1.
expected=$(bed refs | jq -r --arg p "refs/heads/$P/" '.[] | select(.[0] | startswith($p)) | "\(.[1]) \(.[0])"' | sort)
echo "$expected" > expected-refs.txt
step "refs" "$(echo "$expected" | wc -l)"
mkdir -p clones
for s in 0 1; do
  for v in 2 0; do
    d=clones/s$s-v$v
    if g -c "http.extraHeader=x-bed-token: $BED" -c "http.extraHeader=x-graft-shard: $s" -c protocol.version=$v clone -q --mirror "$U" "$d" 2> "$d.err"; then
      got=$(git -C "$d" for-each-ref --format='%(objectname) %(refname)' "refs/heads/$P/" | sort)
      if [ "$got" = "$expected" ] && git -C "$d" fsck --strict --no-dangling > "$d.fsck" 2>&1; then step "clone shard $s v$v" '"ok"';
      else fail "clone shard $s v$v" "refs or fsck: $(head -c 200 "$d.fsck")"; fi
    else fail "clone shard $s v$v" "$(head -c 300 "$d.err")"; fi
  done
done
for ref in $(echo "$expected" | awk '{print $2}' | sed 's|refs/heads/||'); do
  d=clones/one-${ref//\//_}
  want=$(echo "$expected" | awk -v r="refs/heads/$ref" '$2 == r {print $1}')
  if g -c "http.extraHeader=x-bed-token: $BED" -c "http.extraHeader=x-graft-shard: 1" clone -q --single-branch -b "$ref" "$U" "$d" 2> "$d.err" \
     && git -C "$d" fsck --strict > /dev/null 2>&1 && [ "$(git -C "$d" rev-parse HEAD)" = "$want" ]; then step "clone -b $ref from shard 1" '"ok"';
  else fail "clone -b $ref from shard 1" "$(head -c 300 "$d.err")"; fi
done
# 7. A newer push, then an incremental fetch from shard 1 into its v2 mirror.
commit main 1 && push "$P/main"
if g -C clones/s1-v2 -c "http.extraHeader=x-bed-token: $BED" -c "http.extraHeader=x-graft-shard: 1" fetch -q "$U" "+refs/heads/$P/main:refs/heads/$P/main" 2> fetch.err \
   && [ "$(git -C clones/s1-v2 rev-parse "refs/heads/$P/main")" = "$(git -C src rev-parse main)" ]; then step "fetch from shard 1" '"ok"';
else fail "fetch from shard 1" "$(head -c 300 fetch.err)"; fi
echo "failures: $fails"
exit $fails
