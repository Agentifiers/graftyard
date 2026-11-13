#!/bin/bash
# Rehydrate's tail ingested in batches: BATCH packs concatenated into one (concat.py), one index-pack --strict
# --fix-thin of it, and one show-index, one CPU. DIR is the pack directory.
set -u
cd "${BENCH:-/tmp/rhbench}"
R=b-$$.git
rm -rf $R; git init -q --bare $R; git -C $R config gc.auto 0
ls $DIR/*.pack > list-$$
t0=$(date +%s%N); cat=0
split -l $BATCH list-$$ chunk-$$-
for c in chunk-$$-*; do
  c0=$(date +%s%N); python3 concat.py batch-$$.pack $(cat $c); cat=$((cat + $(date +%s%N) - c0))
  out=$(taskset -c 0 git -C $R index-pack --strict --fix-thin --stdin < batch-$$.pack) || { echo FAIL $c; break; }
  taskset -c 0 git -C $R show-index < $R/objects/pack/pack-${out#pack	}.idx > /dev/null
done
echo "$DIR batch $BATCH: total $(( ($(date +%s%N) - t0) / 1000000 )) ms, of it concat $((cat / 1000000)) ms, packs $(ls $R/objects/pack/*.pack | wc -l), objects $(git -C $R count-objects -v | grep in-pack)"
git -C $R fsck --connectivity-only 2>&1 | head -2
rm -rf $R batch-$$.pack list-$$ chunk-$$-*
