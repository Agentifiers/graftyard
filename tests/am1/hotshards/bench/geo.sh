#!/bin/bash
# The container's /ingest steps per pack (index-pack --strict --fix-thin --stdin, show-index), one CPU, over DIR's
# packs, with MODE: none; geo (git repack -d -q --geometric=2 every EVERY packs, EXTRA repack options); midx
# (multi-pack-index write every EVERY packs). Prints the time per 500 packs and the total.
set -u
cd "${BENCH:-/tmp/rhbench}"
R=geo-$$.git
rm -rf $R; git init -q --bare $R; git -C $R config gc.auto 0
i=0; s=$(date +%s%N); t0=$s; rt=0
for p in $DIR/*.pack; do
  out=$(taskset -c 0 git -C $R index-pack --strict --fix-thin --stdin < $p)
  taskset -c 0 git -C $R show-index < $R/objects/pack/pack-${out#pack	}.idx > /dev/null
  i=$((i+1))
  if [ $MODE != none ] && [ $((i % EVERY)) = 0 ]; then
    r0=$(date +%s%N)
    case $MODE in
      geo) taskset -c 0 git -C $R ${EXTRA:-} repack -d -q --geometric=2 ;;
      midx) taskset -c 0 git -C $R multi-pack-index write ;;
    esac
    rt=$((rt + $(date +%s%N) - r0))
  fi
  if [ $((i % 500)) = 0 ]; then e=$(date +%s%N); echo "  $i: $(( (e - s) / 1000000 )) ms/500, packs $(ls $R/objects/pack/*.pack | wc -l)"; s=$e; fi
done
echo "$DIR $MODE ${EVERY:-} ${EXTRA:-}: total $(( ($(date +%s%N) - t0) / 1000000 )) ms, of it $MODE $((rt/1000000)) ms"
rm -rf $R
