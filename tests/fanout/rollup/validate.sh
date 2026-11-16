#!/bin/bash
# Roll promotions 0..7 (rollk.sh's K=8 shape), then time a validate of the produced pack as the shard's would run it:
# `index-pack` at nice 19 on 2 threads. -> ${OUT:-/tmp/rollk}/v.txt
set -u; mkdir -p ${OUT:-/tmp/rollk}
O=${OUT:-/tmp/rollk}/v.txt; : > $O
SRC=$HOME/geo-T/objects/pack; mapfile -t P < <(ls -tr $SRC/*.pack)
W=$HOME/rollk-v; rm -rf $W; git init -q --bare $W; names=""
for i in $(seq 0 7); do b=$(basename ${P[$i]} .pack); ln $SRC/$b.pack $SRC/$b.idx $W/objects/pack/; names="$names$b.pack\n"; done
n=$(printf "$names" | nice -n 19 git -C $W -c pack.threads=2 -c pack.windowMemory=256m pack-objects -q --stdin-packs --delta-base-offset $W/objects/pack/roll)
mkdir -p $W/v; cp $W/objects/pack/roll-$n.pack $W/v/x.pack
t0=$(date +%s.%N)
python3 -c '
import resource, subprocess, sys
r = subprocess.run(sys.argv[1:])
u = resource.getrusage(resource.RUSAGE_CHILDREN)
print("rc=%d cpuS=%.1f maxrssKB=%d" % (r.returncode, u.ru_utime + u.ru_stime, u.ru_maxrss))' \
  nice -n 19 git -C $W index-pack --threads=2 $W/v/x.pack >> $O
echo "pack=$(stat -c%s $W/v/x.pack) wall=$(python3 -c "print(round($(date +%s.%N)-$t0,1))")" >> $O
rm -rf $W; echo DONE >> $O
