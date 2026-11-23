#!/usr/bin/env bash
# refusalshape: the Retry-After half of tests/fanout/refusalshape.py, checked against the edge's own build in local
# workerd at $0. It uses coldwrite's bed (`tests/coldwrite`: three site scripts on the site's own build, its seed) with
# this directory's site entry, whose members shed every whole-tier rows read and whose shard answers `warming`
# (`site.mjs`), and two edge scripts: edge 0 the head build (the working tree), edge 1 BASE's. `probe.mjs` sends one
# Agent-class write, one plain-PAT write and one fetch to each, counts them with a1.mjs's tally, and refusalshape.py
# judges each edge's row. The head must meet the line; with BASE before edge-shed-429 (ff111741), the base must not.
#   BASE=ff111741 bash tests/refusalshape/run.sh
#   B=<dir> bash tests/refusalshape/run.sh    # reuse builds already in <dir>/{site,edge,edgebase}
# Builds with test-faults. Needs worker-build, git and tests/m0's wrangler. Run from the repository root.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd); cw=$root/tests/coldwrite
P=${P:-8794}; BASE=${BASE:-ff111741}
B=${B:-$(mktemp -d)}
build() { (cd "$1/workers/$2" && worker-build --release --out-dir "$B/$3" --features test-faults) >"$B/build-$3.log" 2>&1 \
  || { tail -20 "$B/build-$3.log"; exit 1; }; }
[ -f "$B/site/index.js" ] || build "$root" site site
[ -f "$B/edge/index.js" ] || build "$root" edge edge
if [ ! -f "$B/edgebase/index.js" ]; then
  git -C "$root" worktree add -q --detach "$B/base-tree" "$BASE" || exit 1
  CARGO_TARGET_DIR="$B/target-base" build "$B/base-tree" edge edgebase
  git -C "$root" worktree remove --force "$B/base-tree"
fi
cp "$cw/site.mjs" "$B/cw-site.mjs"; cp "$here/site.mjs" "$B/"
# coldwrite's front with REST paths sent to the install's HOST, which the edge's API routes match on (as plaindiff's),
# and coldwrite's bed with this site entry in place and edge 1 on the base build. Each edit must match once.
python3 - "$cw" "$B" "$root" <<'PY' || exit 1
import sys
cw, b, root = sys.argv[1:]
def edit(text, old, new):
    assert text.count(old) == 1, f"not found once: {old!r}"
    return text.replace(old, new)
d = open(f"{cw}/driver.mjs").read()
d = edit(d, "    return env[`EDGE${n}`].fetch(req);\n",
         '    if (url.pathname.startsWith("/api/")) { url.hostname = "cw.example"; '
         "return env[`EDGE${n}`].fetch(new Request(url, req)); }\n    return env[`EDGE${n}`].fetch(req);\n")
open(f"{b}/driver.mjs", "w").write(d)
r = open(f"{cw}/run.sh").read()
r = edit(r, '"main": f"{b}/edge/index.js"', '"main": f"{b}/edge/index.js" if n == 0 else f"{b}/edgebase/index.js"')
r = edit(r, 'here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)', f"here={cw}; root={root}")
r = edit(r, 'B=${B:-$(mktemp -d)}; cp "$here"/site.mjs "$here"/driver.mjs "$B/"', 'B=${B:-$(mktemp -d)}')
open(f"{b}/bed.sh", "w").write(r)
PY
rm -rf "$B/state" "$B/seed.json"
P=$P B=$B EDGES=2 KEEP=1 bash "$B/bed.sh" >"$B/bed.log" 2>&1; tail -3 "$B/bed.log"
pid=$(pgrep -f -n "wrangler dev --port $P ")
stop() { [ -n "$pid" ] && { kill -- -"$pid" 2>/dev/null || kill "$pid" 2>/dev/null; }; }
trap stop EXIT
[ -f "$B/seed.json" ] || { echo "the bed did not seed"; exit 1; }
rc=0
for n in 0 1; do
  name=$([ $n = 0 ] && echo head || echo "base $BASE")
  echo "== edge $n ($name)"
  node "$here/probe.mjs" "http://127.0.0.1:$P" "$B" $n "$B/row$n.jsonl" || { rc=1; continue; }
  out=$(python3 "$root/tests/fanout/refusalshape.py" "$B/row$n.jsonl"); echo "$out"
  verdict=$(echo "$out" | tail -1)
  if [ $n = 0 ]; then [[ $verdict == *": met" ]] || rc=1; else [[ $verdict == *"not met" ]] || rc=1; fi
done
[ $rc = 0 ] && echo "PASS: the head meets the line and the base does not" || echo FAIL
exit $rc
