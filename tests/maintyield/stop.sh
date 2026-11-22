#!/bin/bash
# maint-yield: SIGTERM a geometric repack and its commit-graph write at random points, then check the repository is whole
# (decisions/DECISIONS.md, "`maint-yield`", Stopping). N trials, signals in (0.05 s, MAXD s); a full run takes about 12 s.
# Run: N=40 bash tests/maintyield/stop.sh; N=60 MAXD=12.5 bash tests/maintyield/stop.sh
set -u
mkdir -p ${TMPDIR:-/tmp}/maintyield
SRC=${TMPDIR:-/tmp}/maintyield/repo-src
if [ ! -d $SRC ]; then
  git init -q --bare $SRC; W=${TMPDIR:-/tmp}/maintyield/wt2; rm -rf $W; git init -q $W; cd $W
  head -c 3000000 /dev/urandom > base.bin; prev=
  for i in $(seq 1 70); do
    cp base.bin f$i.bin; printf 'edit %d' $i | dd of=f$i.bin bs=1 seek=$((i*1000)) conv=notrunc 2>/dev/null
    head -c 200000 /dev/urandom > r$i.bin
    git add -A >/dev/null; git -c user.name=a -c user.email=a@b commit -qm c$i; tip=$(git rev-parse HEAD)
    if [ -z "$prev" ]; then echo $tip | git pack-objects --revs --stdout > ${TMPDIR:-/tmp}/maintyield/p.pack
    else printf '%s\n^%s\n' $tip $prev | git pack-objects --revs --stdout > ${TMPDIR:-/tmp}/maintyield/p.pack; fi
    (cd $SRC && git index-pack --stdin < ${TMPDIR:-/tmp}/maintyield/p.pack >/dev/null); prev=$tip
  done
  git -C $SRC update-ref refs/heads/main $prev
  # three kept packs, as promotions are
  for p in $(ls $SRC/objects/pack/*.pack | head -3); do touch ${p%.pack}.keep; done
  git -C $SRC multi-pack-index write
fi
ok=0; bad=0; done_before=0
for trial in $(seq 1 ${N:-40}); do
  R=${TMPDIR:-/tmp}/maintyield/trial; rm -rf $R; cp -a $SRC $R; cd $R
  delay=$(python3 -c "import random;print(round(random.uniform(0.05,${MAXD:-2.5}),3))")
  keeps=$(for k in objects/pack/*.keep; do basename $k; done | sed 's/keep$/pack/' | sed 's/^/--keep-pack=/' | tr '\n' ' ')
  setsid bash -c "nice -n 19 git -c pack.threads=1 -c pack.window=250 -c pack.windowMemory=256m repack -d -q --geometric=2 --write-midx $keeps && git commit-graph write --reachable --split --no-progress" >/dev/null 2>&1 &
  pg=$!
  sleep $delay
  if kill -0 $pg 2>/dev/null; then kill -TERM -- -$pg 2>/dev/null; else done_before=$((done_before+1)); fi
  wait $pg 2>/dev/null
  sleep 0.05
  f=$(git fsck --connectivity-only --no-progress 2>&1 | grep -v '^dangling' | grep -v '^notice' ); fr=$?
  m=$(git multi-pack-index verify 2>&1); mr=$?
  c=$(git rev-list --objects --all 2>&1 | wc -l)
  k=$(ls objects/pack/*.keep | wc -l)
  stray=$(ls objects/pack objects/info 2>/dev/null | grep -c -E 'tmp|lock')
  # a later repack must still work
  git -c pack.threads=2 repack -d -q --geometric=2 --write-midx $keeps >/dev/null 2>&1; lr=$?
  f2=$(git fsck --connectivity-only --no-progress 2>&1 | grep -v '^dangling'); 
  if [ -z "$f" ] && [ $mr -eq 0 ] && [ "$c" = "$(git -C $SRC rev-list --objects --all | wc -l)" ] && [ $k -eq 3 ] && [ $lr -eq 0 ] && [ -z "$f2" ]; then ok=$((ok+1)); else bad=$((bad+1)); echo "trial $trial delay $delay: fsck[$f] midx[$mr $m] objs $c keeps $k later $lr fsck2[$f2] stray $stray"; fi
  [ $stray -gt 0 ] && echo "trial $trial delay $delay: $stray tmp/lock files left: $(ls objects/pack objects/info | grep -E 'tmp|lock')"
done
echo "ok $ok bad $bad (finished before the signal: $done_before)"
