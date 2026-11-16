#!/bin/bash
# G9 roll-up, second level: roll promotions 0..7 into A and 8..15 into B (as rollk.sh's K=8), then roll A+B, and
# compare with rollk.sh's direct K=16 roll of 0..15 (1,481,779,957 B). Same settings as rollk.sh. -> ${OUT:-/tmp/rollk}/l2.txt
set -u; mkdir -p ${OUT:-/tmp/rollk}
O=${OUT:-/tmp/rollk}/l2.txt; : > $O
SRC=$HOME/geo-T/objects/pack; mapfile -t P < <(ls -tr $SRC/*.pack)
W=$HOME/rollk-l2; rm -rf $W; git init -q --bare $W; git -C $W config gc.auto 0
roll() {  # roll the named packs in $W: prints out-name, cpu-s, maxrss-KB, wall-s
  local t0=$(date +%s.%N)
  printf "$1" | python3 -c '
import resource, subprocess, sys
r = subprocess.run(sys.argv[1:], stdin=sys.stdin, stdout=open("${OUT:-/tmp/rollk}/l2name", "w"))
u = resource.getrusage(resource.RUSAGE_CHILDREN)
open("${OUT:-/tmp/rollk}/l2cpu", "w").write("%.1f %d" % (u.ru_utime + u.ru_stime, u.ru_maxrss))
sys.exit(r.returncode)' nice -n 19 git -C $W -c pack.threads=2 -c pack.windowMemory=256m \
    pack-objects -q --stdin-packs --delta-base-offset $W/objects/pack/roll
  echo "$(cat ${OUT:-/tmp/rollk}/l2name) $(cat ${OUT:-/tmp/rollk}/l2cpu) $(python3 -c "print(round($(date +%s.%N)-$t0,1))")"
}
for g in 0 8; do
  names=""; in=0
  for i in $(seq $g $((g+7))); do b=$(basename ${P[$i]} .pack); ln $SRC/$b.pack $SRC/$b.idx $W/objects/pack/
    names="$names$b.pack\n"; in=$((in+$(stat -c%s ${P[$i]}))); done
  read n cpu rss wall <<< "$(roll "$names")"
  echo "level1 from=$g in=$in out=$(stat -c%s $W/objects/pack/roll-$n.pack) cpuS=$cpu maxrssKB=$rss wall=$wall" >> $O
  for i in $(seq $g $((g+7))); do b=$(basename ${P[$i]} .pack); rm $W/objects/pack/$b.pack $W/objects/pack/$b.idx; done
  mv $W/objects/pack/roll-$n.pack $W/objects/pack/L1-$g.pack; mv $W/objects/pack/roll-$n.idx $W/objects/pack/L1-$g.idx
  rm -f $W/objects/pack/roll-$n.*
done
in=$(( $(stat -c%s $W/objects/pack/L1-0.pack) + $(stat -c%s $W/objects/pack/L1-8.pack) ))
read n cpu rss wall <<< "$(roll "L1-0.pack\nL1-8.pack\n")"
echo "level2 in=$in out=$(stat -c%s $W/objects/pack/roll-$n.pack) cpuS=$cpu maxrssKB=$rss wall=$wall (direct K=16: 1481779957)" >> $O
rm -rf $W
echo DONE >> $O
