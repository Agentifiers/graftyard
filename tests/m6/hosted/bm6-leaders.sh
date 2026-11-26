#!/usr/bin/env bash
# BM-6's lifeboat line 3 (`tests/m6/hosted/bm6-lifeboat.sh`, `leaders`): every committed voter's role and its `/stats`
# inbound `/log_rows` count on bm6's chains: its repository chain, `release`, `ids`, and bm6's `names` shard. M6-WP1 runs
# it in its own orb with the bench token (`bf50.sh`'s `bench`, read only from ~/.graft-m6a), once at fault -5 min and
# once at the start of the lifeboat's gate window, and sends the Lifeboat thread the output file. Read-only: `/configs`
# and `/member` with `/status` and `/stats`, no write.
#   bash tests/m6/hosted/bm6-leaders.sh <bm6 repository id> <out.jsonl>
set -uo pipefail
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
# shellcheck source=/dev/null
source "$ROOT/tests/m6/hosted/bf50.sh"
NET=${1:?bm6 repository id}; OUT=${2:?out.jsonl}
SHARD=$(python3 -c "import hashlib,struct,sys; print(struct.unpack('>I', hashlib.sha256(b'graftyard/names/repo/' + struct.pack('>q', int(sys.argv[1]))).digest()[:4])[0] % 1024)" "$NET")
: >"$OUT"
for c in "repo:$NET" release ids "names:$SHARD"; do
  cfg=$(bench /configs "{\"chains\":[{\"chain\":\"$c\",\"site\":1,\"do_name\":\"$c#1#f1\"}]}" | jq -c --arg c "$c" '.[$c]')
  echo "$cfg" | jq -c '(.cfg.config // .cfg) as $x | [($x.normal // empty), ($x.degraded.placement // empty), ($x.joint.old // empty),
      ($x.joint.new // empty)] | [.[].voters[]] | unique_by(.do_name) | .[]' |
  while read -r v; do
    s=$(echo "$v" | jq .site); n=$(echo "$v" | jq -r .do_name)
    st=$(bench /member "{\"site\":$s,\"do_name\":\"$n\",\"path\":\"/status\"}" | jq -c '.v // {}')
    sx=$(bench /member "{\"site\":$s,\"do_name\":\"$n\",\"path\":\"/stats\"}" | jq -c '.v // {}')
    jq -nc --arg c "$c" --argjson s "$s" --arg n "$n" --argjson st "$st" --argjson sx "$sx" --arg t "$(date -u +%FT%TZ)" \
      '{chain: $c, site: $s, do_name: $n, role: $st.role, term: $st.term, commit: $st.commit,
        log_rows: ($sx.inbound["/log_rows"] // 0), at: $t}' >>"$OUT"
  done
done
cat "$OUT"
