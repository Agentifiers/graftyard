#!/bin/bash
# Round 24c: the shard's own geometric repack (`git repack -d -q --geometric=2`, containers/git/server/main.go
# runRepack) timed on promoted packs as the shard holds them after a cold rehydrate, then a depth-1 clone's
# upload-pack on the result. Promoted packs as round 23b made them (6-step complete ranges of B0-34's plan).
set -u
L=$HOME/b034/linux.git; P=$HOME/b034/plan.txt; R=$HOME/geo-R; O=/tmp/georepack.out
c() { sed -n "${1}p" $P | cut -d' ' -f1; }
rm -rf $R; git init -q --bare $R; mkdir -p /tmp/prs
{
git -C $L rev-list --objects $(c 8) | cut -d' ' -f1 | git -C $L pack-objects -q --delta-base-offset --threads=4 $R/objects/pack/pack >/dev/null
for s in $(seq 8 6 194); do e=$((s+6)); [ $e -gt 200 ] && e=200
  git -C $L rev-list --objects $(c $e) --not $(c $s) | cut -d' ' -f1 > /tmp/prs/oids
  git -C $L pack-objects -q --delta-base-offset --threads=4 $R/objects/pack/pack < /tmp/prs/oids >/dev/null; done
git -C $R update-ref refs/heads/main $(c 200)
echo "packs $(ls $R/objects/pack/*.pack | wc -l) bytes $(du -cb $R/objects/pack/*.pack | tail -1 | cut -f1)"
python3 - $R <<'P'
import resource, subprocess, sys, time
R = sys.argv[1]
def run(name, args):
    t = time.time(); r0 = resource.getrusage(resource.RUSAGE_CHILDREN)
    p = subprocess.run(args, cwd=R)
    r1 = resource.getrusage(resource.RUSAGE_CHILDREN)
    cpu = (r1.ru_utime - r0.ru_utime) + (r1.ru_stime - r0.ru_stime)
    print(f"{name} wall {time.time()-t:.0f}s cpu {cpu:.0f}s ({cpu/(time.time()-t):.2f} vCPU) maxrss {r1.ru_maxrss/1e6:.1f} GB rc {p.returncode}", flush=True)
run("geometric repack", ["git", "repack", "-d", "-q", "--geometric=2"])
run("commit-graph", ["git", "commit-graph", "write", "--reachable", "--split", "--no-progress"])
P
echo "after packs $(ls $R/objects/pack/*.pack | wc -l) bytes $(du -cb $R/objects/pack/*.pack | tail -1 | cut -f1)"
} > $O 2>&1
echo DONE >> $O
