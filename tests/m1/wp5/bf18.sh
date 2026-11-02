#!/usr/bin/env bash
# BF-18 on graft-m1-wp5: archive a range of inline packs, kill all shards, rehydrate, and a second shard behind its
# IngestCursor. Real git on both ends: the pushes are packs built by `git pack-objects` from a local repository, as
# receive-pack would get them (thin against the refs the leader holds), and every check is an unmodified git clone.
#
# Usage: WP5_URL=https://graft-m1-wp5.<sub>.workers.dev WP5_TOKEN=... tests/m1/wp5/bf18.sh <workdir>
# Prints one line per step and a final table; the full JSON of each step is in <workdir>/steps.jsonl.
set -euo pipefail
W=${1:?workdir}
U=${WP5_URL:?}
T=${WP5_TOKEN:?}
mkdir -p "$W"
cd "$W"
rm -rf src clones steps.jsonl
: > steps.jsonl
ZERO=0000000000000000000000000000000000000000

api() { # method path [file]
  local m=$1 p=$2 f=${3:-}
  if [ -n "$f" ]; then
    curl -sS --max-time 900 -X "$m" -H "Authorization: Bearer $T" --data-binary @"$f" "$U$p"
  else
    curl -sS --max-time 900 -X "$m" -H "Authorization: Bearer $T" "$U$p"
  fi
}
step() { # name json
  printf '{"step":"%s","out":%s}\n' "$1" "$2" >> steps.jsonl
  printf '%-28s %s\n' "$1" "$(echo "$2" | head -c 300)"
}
# The refs the stub leader holds, as git sees them on the server.
server_ref() { api GET /refs | jq -r --arg r "$1" '.refs[] | select(.[0] == $r) | .[1]'; }

git init -q -b main src
git -C src config user.email bf18@graftyard.test
git -C src config user.name bf18
git -C src config core.bigFileThreshold 1m

declare -A have   # ref -> oid on the server
push() { # ref new [extra args for api]; sends the commits of ref not on the server as one thin pack
  local ref=$1 new=$2 extra=${3:-}
  local old=${have[$ref]:-$ZERO}
  local nots=()
  for r in "${!have[@]}"; do nots+=("^${have[$r]}"); done
  printf '%s\n' "$new" "${nots[@]}" | git -C src pack-objects --revs --stdout --thin --delta-base-offset -q > push.pack
  local out
  out=$(api POST "/push?u=refs/heads/$ref:$old:$new$extra" push.pack)
  step "push $ref $(stat -c %s push.pack)B" "$out"
  echo "$out" | jq -e '.index' > /dev/null || { echo "push failed: $out"; exit 1; }
  have[$ref]=$new
}
commit() { # branch n: n small commits on branch
  git -C src checkout -q "$1" 2>/dev/null || git -C src checkout -q -b "$1"
  for i in $(seq 1 "$2"); do
    printf '%s %s %s\n' "$1" "$i" "$RANDOM$RANDOM" >> "src/$1.txt"
    seq 1 $((i * 7)) | sed "s/^/$1-$i-/" > "src/lines-$1.txt"
    git -C src add -A && git -C src commit -q -m "$1 $i"
  done
  git -C src rev-parse HEAD
}

# 1. Pushes 1..8: small pushes, inline in their entries (main, two topics).
push main "$(commit main 3)"
push topic-a "$(commit topic-a 2)"
push main "$(commit main 2)"
push topic-b "$(commit topic-b 2)"
git -C src checkout -q main && git -C src merge -q --no-ff -m "merge topic-a" topic-a && push main "$(git -C src rev-parse HEAD)"
push topic-a "$(commit topic-a 1)"
# A push over 1 MiB of incompressible data: a stored pack, placed in the buckets with a marker.
git -C src checkout -q main && head -c 1500000 /dev/urandom > src/blob.bin && git -C src add blob.bin && git -C src commit -q -m "big blob"
push main "$(git -C src rev-parse HEAD)"
push topic-b "$(commit topic-b 1)"

# Both warm shards serve the network before the archive: the home shard on site A and the second warm site's on B.
step "catchup 0-0" "$(api POST /shard/0-0/catchup)"
step "catchup 1-0" "$(api POST /shard/1-0/catchup)"

# 2. Archive entries 1..8: their inline packs become one promoted DurablePack, then Archived truncates the log.
step "archive 1-8" "$(api POST '/archive?from=1&to=8')"

# 3. Pushes after the archive. The second shard is not called again until the end, so it falls behind its cursor.
push main "$(commit main 2)"
push topic-c "$(commit topic-c 2)"
# A stored push (over 1 MiB) whose marker write is killed: rehydrate must still find it, through the log after the
# last Promoted.
git -C src checkout -q topic-a && head -c 1200000 /dev/urandom > src/blob-a.bin && git -C src add blob-a.bin && git -C src commit -q -m "big blob on topic-a"
push topic-a "$(git -C src rev-parse HEAD)" "&kill_marker=1"
step "catchup 0-0 after" "$(api POST /shard/0-0/catchup)"
# The second shard catches up from its cursor (8) across the promotion and the truncated range.
step "catchup 1-0 across archive" "$(api POST /shard/1-0/catchup)"
push topic-c "$(commit topic-c 1)"

# 4. Kill every shard, then rehydrate the home shard from markers and the log tail.
step "kill" "$(api POST '/kill?shards=0-0,1-0')"
step "status after kill 0-0" "$(api POST /shard/0-0/status)"
step "rehydrate 0-0" "$(api POST /shard/0-0/catchup)"
step "fsck 0-0" "$(api POST /shard/0-0/fsck)"

# 5. More pushes while the second shard is down and behind.
push main "$(commit main 1)"
git -C src checkout -q main && git -C src merge -q --no-ff -m "merge topic-c" topic-c && push main "$(git -C src rev-parse HEAD)"
push topic-d "$(commit topic-d 1)"

# 6. The second shard: its container was killed, so its cursor describes a lost disk; it rehydrates and then serves the
# newer pushes. Then a push lands and the same container catches up from its cursor without a rehydrate.
step "catchup 1-0" "$(api POST /shard/1-0/catchup)"
push topic-d "$(commit topic-d 1)"
step "catchup 1-0 behind" "$(api POST /shard/1-0/catchup)"
step "fsck 1-0" "$(api POST /shard/1-0/fsck)"

# 7. Every ref clones from each shard with unmodified git, protocol v2 and v0, and matches the server's refs.
expected=$(api GET /refs | jq -r '.refs[] | "\(.[1]) \(.[0])"' | sort)
echo "$expected" > expected-refs.txt
mkdir -p clones
fails=0
H="http.extraHeader=Authorization: Bearer $T"
for s in 0-0 1-0; do
  for v in 2 0; do
    d=clones/$s-v$v
    if git -c "$H" -c protocol.version=$v clone -q --mirror "$U/git/$s" "$d" 2> "$d.err"; then
      got=$(git -C "$d" for-each-ref --format='%(objectname) %(refname)' refs/heads | sort)
      fsck=$(git -C "$d" fsck --strict --no-dangling 2>&1 | tail -1 || true)
      if [ "$got" = "$expected" ] && git -C "$d" fsck --strict --no-dangling > /dev/null 2>&1; then r=ok; else r=MISMATCH; fails=$((fails + 1)); fi
      step "clone $s v$v" "{\"result\":\"$r\",\"refs\":$(echo "$got" | grep -c .),\"fsck\":\"$fsck\"}"
    else
      fails=$((fails + 1))
      step "clone $s v$v" "{\"result\":\"FAILED\",\"err\":$(jq -Rs . < "$d.err")}"
    fi
  done
done
# Each branch also clones on its own (a single-branch clone of every ref), from the shard that was behind.
for ref in $(echo "$expected" | awk '{print $2}' | sed 's|refs/heads/||'); do
  d=clones/one-$ref
  if git -c "$H" clone -q --single-branch -b "$ref" "$U/git/1-0" "$d" 2> "$d.err" && git -C "$d" fsck --strict > /dev/null 2>&1 \
     && [ "$(git -C "$d" rev-parse HEAD)" = "$(echo "$expected" | awk -v r="refs/heads/$ref" '$2 == r {print $1}')" ]; then
    step "clone -b $ref from 1-0" '{"result":"ok"}'
  else
    fails=$((fails + 1))
    step "clone -b $ref from 1-0" "{\"result\":\"FAILED\",\"err\":$(jq -Rs . < "$d.err")}"
  fi
done
# Fetch into the first clone after one more push, from the behind shard: an incremental fetch with haves.
push main "$(commit main 1)"
if git -C clones/1-0-v2 -c "$H" fetch -q origin 2> fetch.err && [ "$(git -C clones/1-0-v2 rev-parse refs/heads/main)" = "${have[main]}" ]; then
  step "fetch from 1-0" '{"result":"ok"}'
else
  fails=$((fails + 1)); step "fetch from 1-0" "{\"result\":\"FAILED\",\"err\":$(jq -Rs . < fetch.err)}"
fi
echo "failures: $fails"

# 8. The LRU of bare repositories on the second shard's real container: three empty filler networks next to the
# BF-18 network, then make_room with the disk budget set so that the BF-18 repository and one filler fit. The BF-18
# network is the one being served (kept), the fillers are older, so two of them go; the BF-18 cursor stays.
expected=$(api GET /refs | jq -r '.refs[] | "\(.[1]) \(.[0])"' | sort)
NET=$(api POST "/shard/1-0/lru?fill=3" | tee lru-fill.json | jq '[.before[].network] | min')
big=$(jq --argjson n "$NET" '[.before[] | select(.network == $n) | .bytes] | add' lru-fill.json)
small=$(jq --argjson n "$NET" '[.before[] | select(.network != $n) | .bytes] | max' lru-fill.json)
disk=$(( (big + small) * 100 / 80 + 100 ))
lru=$(api POST "/shard/1-0/lru?disk=$disk")
step "lru 1-0 disk=$disk" "$lru"
if [ "$(echo "$lru" | jq '.room.evicted | length')" = 2 ] &&
  [ "$(echo "$lru" | jq --argjson n "$NET" '[.after[].network] | contains([$n])')" = true ] &&
  [ "$(echo "$lru" | jq '.after | length')" = 2 ] && [ "$(echo "$lru" | jq '.cursor')" != null ]; then
  step "lru result" '{"result":"ok"}'
else
  fails=$((fails + 1)); step "lru result" '{"result":"FAILED"}'
fi
# The BF-18 network still serves after the eviction, with every ref and no rehydrate.
if git -c "$H" clone -q --mirror "$U/git/1-0" clones/after-lru 2> after-lru.err &&
  [ "$(git -C clones/after-lru for-each-ref --format='%(objectname) %(refname)' refs/heads | sort)" = "$expected" ]; then
  step "clone 1-0 after lru" '{"result":"ok"}'
else
  fails=$((fails + 1)); step "clone 1-0 after lru" "{\"result\":\"FAILED\",\"err\":$(jq -Rs . < after-lru.err)}"
fi
echo "failures after lru: $fails"
