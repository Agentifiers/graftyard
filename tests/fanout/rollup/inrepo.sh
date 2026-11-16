#!/bin/bash
# Round 33 (G9 roll-up build): a roll as the shard's /roll runs it, inside a repository that holds every pack, not a
# fresh one with only the inputs (rollk.sh). K consecutive promotions of round 27's fixture (~/geo-T) go to
# `pack-objects --stdin-packs --stdout`; with EXCLUDE=1 every other pack on disk is listed as `^pack-X.pack`, so the
# name-hash walk stops at them (as `--keep-pack` does for the repack, `repack-keep-pack`); with EXCLUDE=0 it is not.
# nice 19, pack.threads=2, windowMemory=256m. Writes ${OUT:-/tmp/rollk}/inrepo.txt, ending DONE.
set -u; OUT=${OUT:-/tmp/rollk}; mkdir -p $OUT; O=$OUT/inrepo.txt; : > $O
SRC=$HOME/geo-T/objects/pack
mapfile -t PACKS < <(ls -tr $SRC/*.pack)
log() { echo "$(date -u +%T) $*" >> $O; }
W=$HOME/rollk-in; rm -rf $W; git init -q --bare $W; git -C $W config gc.auto 0
for p in "${PACKS[@]}"; do b=$(basename $p .pack); ln $SRC/$b.pack $SRC/$b.idx $W/objects/pack/; touch $W/objects/pack/$b.keep; done
# The fixture's refs, so a walk has commits to start from as on a shard.
git -C $HOME/geo-T for-each-ref --format='%(objectname) %(refname)' | while read o r; do git -C $W update-ref $r $o; done
for EX in ${EXS:-1 0}; do for K in ${KS:-8}; do FROM=${FROM:-8}
  names=""; in=0
  for i in "${!PACKS[@]}"; do b=$(basename ${PACKS[$i]} .pack)
    if [ $i -ge $FROM ] && [ $i -lt $((FROM+K)) ]; then names="$names$b.pack\n"; in=$((in+$(stat -c%s ${PACKS[$i]})))
    elif [ $EX = 1 ]; then names="$names^$b.pack\n"; fi
  done
  t0=$(date +%s.%N)
  printf "$names" | python3 -c '
import resource, subprocess, sys
r = subprocess.run(sys.argv[2:], stdin=sys.stdin, stdout=open(sys.argv[1], "wb"))
u = resource.getrusage(resource.RUSAGE_CHILDREN)
print("%.1f %d %d" % (u.ru_utime + u.ru_stime, u.ru_maxrss, r.returncode))' $OUT/inrepo.pack nice -n 19 git -C $W -c pack.threads=2 \
    -c pack.windowMemory=256m pack-objects --stdin-packs --stdout -q --delta-base-offset > $OUT/inrepo.cpu
  t1=$(date +%s.%N); out=$(stat -c%s $OUT/inrepo.pack)
  log "EXCLUDE=$EX K=$K from=$FROM in=$in out=$out ratio=$(python3 -c "print(round($out/$in,3))") wall=$(python3 -c "print(round($t1-$t0,1))") cpuS/maxrssKB/rc=$(cat $OUT/inrepo.cpu)"
  rm -f $OUT/inrepo.pack
done; done
rm -rf $W
log DONE
