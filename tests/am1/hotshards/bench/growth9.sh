#!/bin/bash
# Per-pack ingest time as the pack count grows, one CPU, the container's /ingest steps: index-pack (--strict unless
# STRICT=0) --fix-thin --stdin, then show-index. MIDX=k writes a multi-pack-index every k packs.
set -u
cd "${BENCH:-/tmp/rhbench}"
R=g-$$.git
rm -rf $R; git init -q --bare $R; git -C $R config gc.auto 0
S=--strict; [ "${STRICT:-1}" = 0 ] && S=
i=0; s=$(date +%s%N); t0=$s
for p in small9/*.pack; do
  out=$(taskset -c 0 git -C $R index-pack $S --fix-thin --stdin < $p)
  taskset -c 0 git -C $R show-index < $R/objects/pack/pack-${out#pack	}.idx > /dev/null
  i=$((i+1))
  if [ "${MIDX:-0}" -gt 0 ] && [ $((i % MIDX)) = 0 ]; then taskset -c 0 git -C $R multi-pack-index write ${MIDXFLAGS:-}; fi
  if [ $((i % 1000)) = 0 ]; then e=$(date +%s%N); echo "packs $i: $(( (e - s) / 1000000 )) ms/100 packs"; s=$e; fi
done
echo "total $(( ($(date +%s%N) - t0) / 1000000 )) ms"
rm -rf $R
