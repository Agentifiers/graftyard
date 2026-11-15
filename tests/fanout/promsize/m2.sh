#!/bin/bash
cd $HOME/b034/linux.git
P=$HOME/b034/plan.txt
c() { sed -n "${1}p" $P | cut -d' ' -f1; }
t0=$(date +%s); s=$(printf "%s\n^%s\n" $(c 200) $(c 8) | git pack-objects --revs --thin --stdout -q --delta-base-offset --threads=4 | wc -c); echo "thinone 8..200 $s $(( $(date +%s)-t0 ))s"
t0=$(date +%s); s=$(printf "%s\n^%s\n" $(c 200) $(c 176) | git pack-objects --revs --stdout -q --delta-base-offset --threads=4 --no-reuse-delta | wc -c); echo "complete-noreuse 24 $s $(( $(date +%s)-t0 ))s"
t0=$(date +%s); s=$(printf "%s\n^%s\n" $(c 200) $(c 176) | git pack-objects --revs --stdout -q --delta-base-offset --threads=4 --window=50 --no-reuse-delta | wc -c); echo "complete-w50 24 $s $(( $(date +%s)-t0 ))s"
t0=$(date +%s); s=$(printf "%s\n^%s\n" $(c 200) $(c 176) | git pack-objects --revs --thin --stdout -q --delta-base-offset --threads=4 --no-reuse-delta | wc -c); echo "thin-noreuse 24 $s $(( $(date +%s)-t0 ))s"
echo DONE
