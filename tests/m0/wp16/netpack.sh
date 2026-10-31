#!/bin/sh
# M0-WP16: build the replay network's pack for the predictor container, without linux's full history.
#
#   sh wp16/netpack.sh <repo> <tips file> <boundary commit> <out dir>
#
# The pack holds every object reachable from the replay's heads and bases that the boundary commit does not reach, plus
# each boundary commit of that walk (the boundary itself and the older commits topic branches fork from) with its whole
# tree. `shallow` lists those boundary commits, so the container's history stops there. The boundary is the octopus merge
# base of every merge base in the replay (v7.0-rc4), so no merge base the replay uses is cut off.
# Lines are "<sha> <path>" so pack-objects can pick deltas by path.
set -eu
repo=$1 tips=$2 boundary=$3 out=$4
mkdir -p "$out"
{ echo "$boundary"; git -C "$repo" rev-list --boundary --stdin --not "$boundary" < "$tips" | sed -n 's/^-//p'; } \
  | sort -u > "$out/shallow"
{
  git -C "$repo" rev-list --objects --stdin --not "$boundary" < "$tips"
  while read -r c; do
    echo "$c"
    git -C "$repo" rev-parse "$c^{tree}"
    git -C "$repo" ls-tree -r -t --format='%(objectname) %(path)' "$c"
  done < "$out/shallow"
} | sort -u -k1,1 > "$out/objects.txt"
wc -l < "$out/shallow"
wc -l < "$out/objects.txt"
git -C "$repo" pack-objects --stdout --threads=4 < "$out/objects.txt" > "$out/pack.pack"
ls -l "$out/pack.pack"
