#!/bin/bash
# Pushed thin bytes per plan step, and a complete pack's bytes for ranges of N steps ending at step 200.
cd $HOME/b034/linux.git
P=$HOME/b034/plan.txt
c() { sed -n "${1}p" $P | cut -d' ' -f1; }
END=200
for i in $(seq 9 $END); do
  s=$(printf "%s\n^%s\n" $(c $i) $(c $((i-1))) | git pack-objects --revs --thin --stdout -q --delta-base-offset --threads=4 | wc -c)
  echo "thin $i $s"
done
for N in 6 24 96 192; do
  B=$((END-N))
  t0=$(date +%s)
  s=$(printf "%s\n^%s\n" $(c $END) $(c $B) | git pack-objects --revs --stdout -q --delta-base-offset --threads=4 | wc -c)
  echo "complete $N $B..$END $s $(( $(date +%s) - t0 ))s"
done
t0=$(date +%s)
s=$(printf "%s\n" $(c $END) | git pack-objects --revs --stdout -q --delta-base-offset --threads=4 | wc -c)
echo "whole 1..$END $s $(( $(date +%s) - t0 ))s"
echo DONE
