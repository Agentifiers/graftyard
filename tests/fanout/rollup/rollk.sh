#!/bin/bash
# Round 32 ran KS="4 8 16 34" FROMS=0 (the work dir must be on the same disk as the fixture: hard links, not a tmpfs copy).
# G9 roll-up sizing (LM-1 round 32, design evidence): roll K consecutive promoted packs of round 27's fixture
# (~/geo-T: 33 complete 6-step promotions of B0-34's plan plus the base, each kept) into one pack, the way a bounded
# roll-up would: `pack-objects --stdin-packs` of exactly those packs, nice 19, pack.threads=2, windowMemory=256m.
# Samples every git process's RssAnon/RssFile each second. Writes ${OUT:-/tmp/rollk}/out.txt, ending DONE.
set -u; mkdir -p ${OUT:-/tmp/rollk}
O=${OUT:-/tmp/rollk}/out.txt; : > $O
SRC=$HOME/geo-T/objects/pack
mapfile -t PACKS < <(ls -tr $SRC/*.pack)   # oldest first: base, then promotions in plan order
log() { echo "$(date -u +%T) $*" >> $O; }
sample() {
  local peakA=0 peakF=0
  while kill -0 $1 2>/dev/null; do
    for p in $(pgrep -f 'git.*pack-objects'); do
      a=$(awk '/RssAnon/{print $2}' /proc/$p/status 2>/dev/null); f=$(awk '/RssFile/{print $2}' /proc/$p/status 2>/dev/null)
      [ -n "$a" ] && [ "$a" -gt "$peakA" ] && peakA=$a; [ -n "$f" ] && [ "$f" -gt "$peakF" ] && peakF=$f
    done; sleep 1
  done
  echo "$peakA $peakF"
}
for K in ${KS:-4 8 16}; do
  for FROM in ${FROMS:-1}; do
    W=$HOME/rollk-w; rm -rf $W; git init -q --bare $W; git -C $W config gc.auto 0
    in=0; names=""
    for i in $(seq $FROM $((FROM+K-1))); do [ $i -ge ${#PACKS[@]} ] && break
      p=${PACKS[$i]}; b=$(basename $p .pack)
      ln $SRC/$b.pack $SRC/$b.idx $W/objects/pack/
      in=$((in+$(stat -c%s $p))); names="$names$b.pack\n"
    done
    t0=$(date +%s.%N)
    ( printf "$names" | python3 -c '
import resource, subprocess, sys
r = subprocess.run(sys.argv[1:], stdin=sys.stdin, stdout=open("${OUT:-/tmp/rollk}/name", "w"))
u = resource.getrusage(resource.RUSAGE_CHILDREN)
open("${OUT:-/tmp/rollk}/cpu", "w").write("%.1f %d" % (u.ru_utime + u.ru_stime, u.ru_maxrss))
sys.exit(r.returncode)' nice -n 19 git -C $W -c pack.threads=2 -c pack.windowMemory=256m \
        pack-objects -q --stdin-packs --delta-base-offset $W/objects/pack/roll ) &
    pid=$!
    peaks=$(sample $pid); wait $pid; rc=$?
    t1=$(date +%s.%N)
    out=$(stat -c%s $W/objects/pack/roll-$(cat ${OUT:-/tmp/rollk}/name).pack 2>/dev/null || echo 0)
    log "K=$K from=$FROM in=$in out=$out ratio=$(python3 -c "print(round($out/$in,3))") wall=$(python3 -c "print(round($t1-$t0,1))") rc=$rc cpuS/maxrssKB=$(cat ${OUT:-/tmp/rollk}/cpu) peakRssAnonKB/RssFileKB=$peaks"
    rm -rf $W
  done
done
log DONE
