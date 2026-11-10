#!/bin/bash
cd /home/user/workspace/repo/.worktrees/fanout
out=tests/fanout/raw/bench.jsonl
mkdir -p tests/fanout/raw
[ -e tests/fanout/raw/.bench.started ] && exec sleep infinity
touch tests/fanout/raw/.bench.started
: > $out
for cpus in 1 4; do for img in graft-git:before graft-m1-site-gitshard-git:fanout; do
  python3 tests/fanout/bench.py $img --cpus $cpus --secs 20 >> $out 2>> tests/fanout/raw/bench.err
done; done
python3 tests/fanout/bench.py graft-git:before --cpus 1 --secs 20 --mode push --packs 1500 >> $out 2>> tests/fanout/raw/bench.err
python3 tests/fanout/bench.py graft-m1-site-gitshard-git:fanout --cpus 1 --secs 20 --mode push --packs 1500 >> $out 2>> tests/fanout/raw/bench.err
echo done >> tests/fanout/raw/bench.err
exec sleep infinity
