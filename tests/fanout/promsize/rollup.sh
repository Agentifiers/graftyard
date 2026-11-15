#!/bin/bash
# Promoted packs as the shard makes them (complete, 6 plan steps each, oid list with no path names), then a roll-up.
set -u
L=$HOME/b034/linux.git; P=$HOME/b034/plan.txt
c() { sed -n "${1}p" $P | cut -d' ' -f1; }
R=$HOME/prs-R; rm -rf $R; git init -q --bare $R
t0=$(date +%s); tot=0
for s in $(seq 8 6 194); do
  e=$((s+6)); [ $e -gt 200 ] && e=200
  git -C $L rev-list --objects $(c $e) --not $(c $s) | cut -d' ' -f1 > /tmp/prs/oids
  n=$(git -C $L pack-objects -q --delta-base-offset --threads=4 $R/objects/pack/pack < /tmp/prs/oids)
  b=$(stat -c%s $R/objects/pack/pack-$n.pack); tot=$((tot+b)); echo "promo $s..$e $b"
done
echo "promoted total $tot in $(( $(date +%s)-t0 ))s"
# the objects reachable up to step 8 come from linux.git as one more complete pack, as the earlier promotions would
git -C $L rev-list --objects $(c 8) | cut -d' ' -f1 | git -C $L pack-objects -q --delta-base-offset --threads=4 $R/objects/pack/pack >/dev/null
git -C $R update-ref refs/heads/main $(c 200)
t0=$(date +%s); git -C $R repack -a -d -q --threads=4; echo "rollup reuse $(du -cb $R/objects/pack/*.pack | tail -1 | cut -f1) $(( $(date +%s)-t0 ))s"
echo DONE
