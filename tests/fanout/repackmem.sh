#!/bin/bash
# LM-1 round 27: what holds the shard repack's memory (round 26: the repack's pack-objects at 9.6 to 10.9 GB RSS on
# every m37 shard, beside up to 22 stock depth-1 upload-packs, with the container at 11.9 of 12 GiB).
# Builds once, in $R: round 25's promoted packs (33 complete 6-step ranges of B0-34's plan, each with a .keep, as a
# rehydrate leaves them), then caches a tail in $T: 12 one-step pushes after them and AGENTS agent pushes (one commit
# each: main's tree plus a1/<g>/<i>/f.txt of 30 to 100 KB random bytes, on its agent's branch, a1.mjs's shape), one
# pack per push. Each case restores the tail, then runs one command, sampling every git process's RssAnon and
# RssFile from /proc each second:
#   STEPS (default 21): one-step pushes past the last promotion, as m37's 21 stored tail packs.
#   CASE=repack     the shard's runRepack as on main (nice 19, pack.threads=2, windowMemory=256m, --geometric=2)
#   CASE=uploads    UPLOADS stock depth-1 v2 upload-packs of main at once, alone
#   CASE=keeppack   the repack with every kept pack also passed as --keep-pack (the fix candidate)
#   CASE=both-keeppack  that, with the upload-packs beside it
#   CASE=both       the repack with UPLOADS upload-packs started beside it
#   CASE=<other>    `$CMD` in $R (a fix candidate)
#   TAIL=12|agents|all  which tail packs (default all: 12 step packs and the agent packs)
#   bash tests/fanout/repackmem.sh            -> $O (default /tmp/repackmem.out), ending DONE
set -u
# One run at a time: every case restores the same $R.
exec 9>/tmp/repackmem.lock; flock -n 9 || { echo "another repackmem.sh runs" >&2; exit 1; }
L=$HOME/b034/linux.git; P=$HOME/b034/plan.txt; R=${R:-$HOME/geo-T}; T=${T:-$HOME/geo-T-tail}
O=${O:-/tmp/repackmem.out}; AGENTS=${AGENTS:-1200}; UPLOADS=${UPLOADS:-22}
c() { sed -n "${1}p" $P | cut -d' ' -f1; }
pk() { git -C $L pack-objects -q --delta-base-offset --threads=4 "$1" >/dev/null; }
log() { echo "$(date -u +%T) $*" >> $O; }

build() {
  rm -rf $R $T; git init -q --bare $R; git -C $R config gc.auto 0; mkdir -p $T/step $T/agent
  git -C $L rev-list --objects $(c 8) | cut -d' ' -f1 | pk $R/objects/pack/pack
  for s in $(seq 8 6 194); do e=$((s+6)); [ $e -gt 200 ] && e=200
    git -C $L rev-list --objects $(c $e) --not $(c $s) | cut -d' ' -f1 | pk $R/objects/pack/pack; done
  for p in $R/objects/pack/*.pack; do touch "${p%.pack}.keep"; done
  for s in $(seq 200 $((199+${STEPS:-21}))); do git -C $L rev-list --objects $(c $((s+1))) --not $(c $s) | cut -d' ' -f1 | pk $T/step/pack; done
  # The step packs' objects, so the agent commits can be built in $R before the tail is taken out again.
  cp $T/step/* $R/objects/pack/
  git -C $R update-ref refs/heads/main $(c $((200+${STEPS:-21})))
  python3 - "$R" "$T/agent" "$AGENTS" <<'PY'
import os, random, subprocess, sys
R, out, n = sys.argv[1], sys.argv[2], int(sys.argv[3])
g = lambda *a, inp=None: subprocess.run(["git", "-C", R, *a], input=inp, capture_output=True, check=True).stdout
main = g("rev-parse", "main").decode().strip()
root = g("ls-tree", main + "^{tree}").decode()
mk = lambda lines: g("mktree", inp="".join(lines).encode()).decode().strip()
tips = {}
for k in range(n):
    a = k % 500; grp = a % 50
    blob = g("hash-object", "-w", "--stdin", inp=os.urandom(random.randint(30_000, 100_000))).decode().strip()
    t_i = mk([f"100644 blob {blob}\tf.txt\n"]); t_g = mk([f"040000 tree {t_i}\t{a}\n"])
    t_a1 = mk([f"040000 tree {t_g}\t{grp}\n"]); t_root = mk([root, f"040000 tree {t_a1}\ta1\n"])
    parent = tips.get(a, main)
    c = g("commit-tree", t_root, "-p", parent, "-m", f"a1 {k}", inp=b"").decode().strip()
    tips[a] = c
    subprocess.run(["git", "-C", R, "pack-objects", "-q", f"{out}/pack"], input="\n".join([c, t_root, t_a1, t_g, t_i, blob]).encode(),
                   capture_output=True, check=True)
    g("update-ref", f"refs/heads/a1/{grp}/{a}", c)
g("prune-packed")
PY
  git -C $R for-each-ref --format='%(objectname) %(refname)' > $T/refs
  log "built: $(ls $R/objects/pack/*.keep | wc -l) kept, $(ls $T/step/*.pack | wc -l) step, $(ls $T/agent/*.pack | wc -l) agent packs"
}

# Puts the cached tail back (and the refs), after taking out every unkept pack and the midx a case left.
restore() {
  for p in $R/objects/pack/pack-*.pack; do [ -e "${p%.pack}.keep" ] || rm -f "${p%.pack}".*; done
  rm -f $R/objects/pack/multi-pack-index* $R/objects/info/commit-graph* ; rm -rf $R/objects/info/commit-graphs
  case ${TAIL:-all} in 12) cp $T/step/* $R/objects/pack/;; agents) cp $T/agent/* $R/objects/pack/;;
    *) cp $T/step/* $T/agent/* $R/objects/pack/;; esac
  # TAIL=12 holds no agent pack, so it gets main alone: an agent ref without its objects fails connectivity.
  git -C $R for-each-ref --format='delete %(refname)' refs/heads/a1 | git -C $R update-ref --stdin
  { [ "${TAIL:-all}" = 12 ] && grep ' refs/heads/main$' $T/refs || cat $T/refs; } |
    sed 's/^\([0-9a-f]*\) \(.*\)$/update \2 \1/' | git -C $R update-ref --stdin
  sync; echo 3 | sudo -n tee /proc/sys/vm/drop_caches >/dev/null 2>&1
  log "restored tail ${TAIL:-all}: $(ls $R/objects/pack/*.pack | wc -l) packs, $(ls $R/objects/pack/*.keep | wc -l) kept"
}

# Every second while $1 runs: the largest git process's RssAnon and RssFile, and the sum over all git processes.
sample() {
  python3 - "$1" <<'PY' >> $O
import os, sys, time
pid = int(sys.argv[1]); peak = {"anon": 0, "file": 0, "sum_anon": 0, "sum_file": 0, "procs": 0, "top": ""}
def kb(st, k):
    for l in st.splitlines():
        if l.startswith(k): return int(l.split()[1])
    return 0
t0 = time.time()
while os.path.exists(f"/proc/{pid}") and open(f"/proc/{pid}/stat").read().split()[2] != "Z":
    sa = sf = n = 0
    for p in os.listdir("/proc"):
        if not p.isdigit(): continue
        try:
            cmd = open(f"/proc/{p}/cmdline").read().replace("\0", " ")
            if "git" not in cmd.split(" ")[0]: continue
            st = open(f"/proc/{p}/status").read()
        except OSError: continue
        a, f = kb(st, "RssAnon:"), kb(st, "RssFile:"); sa += a; sf += f; n += 1
        if a > peak["anon"]: peak.update(anon=a, file=f, top=cmd[:70], at=round(time.time() - t0))
    peak["wall"] = round(time.time() - t0)
    if sa > peak["sum_anon"]: peak.update(sum_anon=sa, sum_file_then=sf, procs=n)
    peak["sum_file"] = max(peak["sum_file"], sf)
    time.sleep(1)
g = lambda k: round(peak.get(k, 0) / 1e6, 2)
print(f"  peak: largest RssAnon {g('anon')} GB (its RssFile {g('file')} GB) at {peak.get('at')} s in [{peak['top']}]; "
      f"summed RssAnon {g('sum_anon')} GB over {peak['procs']} procs; summed RssFile max {g('sum_file')} GB; wall {peak.get('wall')} s", flush=True)
PY
}

timed() { local t=$(date +%s.%N); "$@"; local rc=$?; log "  $1...: rc $rc wall $(python3 -c "import time;print(round(time.time()-$t))") s"; }

uploads() {
  local main; main=$(git -C $R rev-parse main)
  python3 -c "
import sys
p = lambda s: b'%04x' % (len(s) + 4) + s
sys.stdout.buffer.write(p(b'command=fetch\\n') + p(b'object-format=sha1\\n') + b'0001' + b''.join(p(x) for x in
  [b'thin-pack\\n', b'no-progress\\n', b'include-tag\\n', b'ofs-delta\\n', b'deepen 1\\n', b'want $main\\n', b'want $main\\n', b'done\\n']) + b'0000')" > /tmp/upl.req
  for i in $(seq 1 $UPLOADS); do
    (GIT_PROTOCOL=version=2 git upload-pack --stateless-rpc $R < /tmp/upl.req | wc -c > /tmp/upl-$i.bytes) &
  done
  wait
}
repack() { nice -n 19 git -C $R -c pack.threads=2 -c pack.windowMemory=256m repack -d -q --geometric=2 --write-midx; }
# The fix candidate: every kept pack also named with --keep-pack, so pack-objects marks it kept in core and the
# --stdin-packs hint walk stops at it (it checks only in-core keeps: builtin/pack-objects.c, read_stdin_packs).
keeppack() { nice -n 19 git -C $R -c pack.threads=2 -c pack.windowMemory=256m repack -d -q --geometric=2 --write-midx \
  $(for k in $R/objects/pack/pack-*.keep; do echo "--keep-pack=$(basename ${k%.keep}).pack"; done); }
after() { log "  after: $(ls $R/objects/pack/*.pack | wc -l) packs, $(ls $R/objects/pack/*.keep | wc -l) kept, unkept bytes $(for p in $R/objects/pack/pack-*.pack; do [ -e "${p%.pack}.keep" ] || stat -c %s $p; done | awk '{s+=$1} END {print s+0}'), main $( [ "$(git -C $R rev-parse main)" = "$(c $((200+${STEPS:-21})))" ] && echo ok || echo WRONG), fsck-connectivity $(git -C $R rev-list --objects --all --quiet >/dev/null 2>&1 && echo ok || echo FAILED)"; }

: > $O
[ -s $T/refs ] || build
for CASE in ${CASES:-repack}; do
  restore; log "case $CASE"
  case $CASE in
    repack) repack & pid=$!; sample $pid; wait $pid; log "  rc $?";;
    uploads) uploads & pid=$!; sample $pid; wait $pid; log "  bytes $(cat /tmp/upl-1.bytes)";;
    both) (uploads) & u=$!; sleep 2; repack & pid=$!; sample $pid; wait $pid; log "  rc $?"; wait $u;;
    keeppack) keeppack & pid=$!; sample $pid; wait $pid; log "  rc $?";;
    both-keeppack) (uploads) & u=$!; sleep 2; keeppack & pid=$!; sample $pid; wait $pid; log "  rc $?"; wait $u;;
    *) (cd $R && eval "$CMD") & pid=$!; sample $pid; wait $pid; log "  rc $?";;
  esac
  after
done
echo DONE >> $O
