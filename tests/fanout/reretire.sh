#!/usr/bin/env bash
# Re-retire networks retired before `retire-sticks` deployed (`decisions/DECISIONS.md`, "`retire-sticks`", Finding):
# their members are not joined and carry no `retired` row, so a create on first use can build the group again. Sends
# every name `/retire` through `retirenets.mjs` with NO_PURGE=1 (their buckets were purged already) and exits 1 unless
# every name answers not joined with the row. It needs a site day from a main with `retire-unjoined` first, since an
# older site answers an unjoined member's `/retire` 409 and writes nothing. Run with the install's env sourced:
#   bash tests/fanout/reretire.sh LIST [TAG]
# LIST holds networks as `N` or `FIRST-LAST`, separated by spaces, commas or newlines; `#` starts a comment.
set -uo pipefail
list=${1:?usage: reretire.sh LIST [TAG]}; tag=${2:-$(basename "$list" .txt)}
nets=$(sed 's/#.*//' "$list" | tr ',\n' '  ' | xargs)
[ -n "$nets" ] || { echo "no networks in $list"; exit 1; }
NO_PURGE=1 RETIRE_LOG=${RETIRE_LOG:-tests/fanout/raw/reretire-$tag.log} node tests/fanout/retirenets.mjs $nets
rc=$?
[ $rc = 0 ] && echo "RERETIRE OK $tag" || echo "RERETIRE FAILED $tag (exit $rc)"
exit $rc
