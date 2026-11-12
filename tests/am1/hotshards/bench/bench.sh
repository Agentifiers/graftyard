#!/bin/bash
# Times rehydrate's ingest options on one CPU (taskset 0, a standard-2's 1 vCPU) over the packs in ./packs.
set -u
cd "${BENCH:-/tmp/rhbench}"
N=$(ls packs/*.pack | wc -l)
BYTES=$(du -sb packs | cut -f1)
fresh() { rm -rf r.git; git init -q --bare r.git; git -C r.git config gc.auto 0; }
t() { local s=$(date +%s%N); "$@"; echo $(( ($(date +%s%N) - s) / 1000000 ))ms; }
echo "packs $N bytes $BYTES"

# A: today: index-pack --strict --fix-thin --stdin per pack, then show-index (record) per pack.
fresh
a=$(t bash -c 'for p in packs/*.pack; do taskset -c 0 git -C r.git index-pack --strict --fix-thin --stdin < $p > "${BENCH:-/tmp/rhbench}"/out; n=$(grep -oE "[0-9a-f]{40}" "${BENCH:-/tmp/rhbench}"/out); taskset -c 0 git -C r.git show-index < r.git/objects/pack/pack-$n.idx > /dev/null; done')
echo "A per-pack strict+record: $a s"

# B: per pack without --strict (the pack was validated when it was pushed).
fresh
b=$(t bash -c 'for p in packs/*.pack; do taskset -c 0 git -C r.git index-pack --stdin < $p > /dev/null; done')
echo "B per-pack plain: $b s"

# C: pack and .idx copied in as files (the .idx kept in R2 beside the pack), then one multi-pack-index.
mkdir -p idx; rm -rf idx/*
for p in packs/*.pack; do cp $p idx/; git index-pack idx/$(basename $p) >/dev/null; done
fresh
c=$(t bash -c 'for p in idx/*.pack; do n=$(basename $p .pack); cp idx/$n.pack idx/$n.idx r.git/objects/pack/; done; taskset -c 0 git -C r.git multi-pack-index write')
echo "C copy pack+idx, midx: $c s"

# D: index-pack with threads on 4 vCPUs (a standard-4), per pack.
fresh
d=$(t bash -c 'for p in packs/*.pack; do git -C r.git index-pack --threads=4 --stdin < $p > /dev/null; done')
echo "D per-pack 4 vCPU: $d s"

# E: one pack of all of them (git pack-objects from the repo C built), indexed once: a promoted base.
fresh
for p in idx/*.pack; do n=$(basename $p .pack); cp idx/$n.pack idx/$n.idx r.git/objects/pack/; done
git -C r.git cat-file --batch-all-objects --batch-check='%(objectname)' > objs
git -C r.git pack-objects --stdout < objs > big.pack 2>/dev/null
fresh
e=$(t bash -c 'taskset -c 0 git -C r.git index-pack --strict --stdin < big.pack > /dev/null')
echo "E one combined pack, strict: $e s ($(stat -c %s big.pack) bytes)"
