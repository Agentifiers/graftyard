#!/bin/bash
# LM-1 round 25's local gate for repack-keep: the shard's post-rehydrate repack on promoted packs as a cold rehydrate
# leaves them (33 complete 6-step ranges of B0-34's plan, 6.5 GB, as tests/fanout/georepack.sh built them) plus a
# pushed tail (12 one-step packs after them), timed as containers/git/server/main.go now runs it:
#   keep:  a .keep beside each promoted pack, then `nice -n 19 git -c pack.threads=2 -c pack.windowMemory=256m
#          repack -d -q --geometric=2`, `multi-pack-index write`, `commit-graph write --reachable --split`
#   bound: the same repack with no .keep (a network that has no promotions yet), fix 2 alone
# against round 24c's unkept, unbounded repack: 325 s, 588 CPU-s, maxrss 9.8 GB. Output: $O, ending DONE.
set -u
L=$HOME/b034/linux.git; P=$HOME/b034/plan.txt; R=$HOME/geo-K; O=${O:-/tmp/georepack-keep.out}
c() { sed -n "${1}p" $P | cut -d' ' -f1; }
pk() { git -C $L pack-objects -q --delta-base-offset --threads=4 $R/objects/pack/pack >/dev/null; }
build() {
  rm -rf $R; git init -q --bare $R; git -C $R config gc.auto 0
  git -C $L rev-list --objects $(c 8) | cut -d' ' -f1 | pk
  for s in $(seq 8 6 194); do e=$((s+6)); [ $e -gt 200 ] && e=200
    git -C $L rev-list --objects $(c $e) --not $(c $s) | cut -d' ' -f1 | pk; done
  ls $R/objects/pack/*.pack > /tmp/geo-promoted.txt
  for s in $(seq 200 211); do git -C $L rev-list --objects $(c $((s+1))) --not $(c $s) | cut -d' ' -f1 | pk; done
  git -C $R update-ref refs/heads/main $(c 212)
}
timed() { python3 - "$R" "$@" <<'P'
import resource, subprocess, sys, time
R, name, args = sys.argv[1], sys.argv[2], sys.argv[3:]
t = time.time(); r0 = resource.getrusage(resource.RUSAGE_CHILDREN)
p = subprocess.run(args, cwd=R)
r1 = resource.getrusage(resource.RUSAGE_CHILDREN)
cpu = (r1.ru_utime - r0.ru_utime) + (r1.ru_stime - r0.ru_stime); w = time.time() - t
print(f"  {name}: wall {w:.0f}s cpu {cpu:.0f}s ({cpu/max(w,1e-3):.2f} vCPU) maxrss {r1.ru_maxrss/1e6:.2f} GB rc {p.returncode}", flush=True)
P
}
packs() { echo "  packs $(ls $R/objects/pack/*.pack | wc -l) bytes $(du -cb $R/objects/pack/*.pack | tail -1 | cut -f1) kept $(ls $R/objects/pack/*.keep 2>/dev/null | wc -l)"; }
run() {
  timed repack nice -n 19 git -c pack.threads=2 -c pack.windowMemory=256m repack -d -q --geometric=2
  timed midx git multi-pack-index write
  timed commit-graph git commit-graph write --reachable --split --no-progress
  packs
  untouched=0; while read p; do [ -e "$p" ] && untouched=$((untouched+1)); done < /tmp/geo-promoted.txt
  echo "  promoted packs still on disk: $untouched of $(wc -l < /tmp/geo-promoted.txt)"
  git -C $R multi-pack-index verify --no-progress >/dev/null 2>&1 && echo "  midx verify ok" || echo "  midx verify FAILED"
  [ "$(git -C $R rev-list -1 refs/heads/main)" = "$(c 212)" ] && echo "  main ok"
}
{
build; echo "keep:"; packs
while read p; do touch "${p%.pack}.keep"; done < /tmp/geo-promoted.txt
run
build; echo "bound (no .keep):"; packs
run
} > $O 2>&1
rm -rf $R
echo DONE >> $O
