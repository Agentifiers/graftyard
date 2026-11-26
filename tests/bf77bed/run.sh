#!/usr/bin/env bash
# bf77bed: BF-77's token-half routes through the edge's own wasm in local workerd, at $0 (`decisions/DECISIONS.md`,
# "`app-bot`"). It uses coldwrite's bed (`tests/coldwrite`: three site scripts on the site's head build, its seed,
# its member stand-in) with two edge scripts, as tests/plaindiff does: edge 0 runs the base build (BASE, default
# origin/main), edge 1 the head build, both with coldwrite's FORMATS_ENABLED plus 32.29. `drive.mjs` signs job tokens
# with BF-77's permissions and sends the issue-create and status probes to each edge: the base must answer what main
# answered on graft-m4a (issue create 404, a token's status 401), the head BF-77's lines (rw 201, default and ro 403).
#   bash tests/bf77bed/run.sh              # build site, base edge and head edge, serve, seed, drive, stop
#   B=<dir> bash tests/bf77bed/run.sh      # reuse builds already in <dir>/{site,edge,edgehead}
# Needs worker-build, git and tests/m0's wrangler. Run from the repository root.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd); cw=$root/tests/coldwrite
P=${P:-8792}; W=${WRANGLER:-$root/tests/m0/node_modules/.bin/wrangler}; BASE=${BASE:-origin/main}
[ -x "$W" ] || { echo "no wrangler at $W"; exit 1; }
B=${B:-$(mktemp -d)}; cp "$cw/site.mjs" "$cw/driver.mjs" "$B/"
build() { (cd "$1/workers/$2" && worker-build --release --out-dir "$B/$3" --features test-faults) >"$B/build-$3.log" 2>&1 \
  || { tail -20 "$B/build-$3.log"; exit 1; }; }
[ -f "$B/site/index.js" ] || build "$root" site site
[ -f "$B/edgehead/index.js" ] || build "$root" edge edgehead
if [ ! -f "$B/edge/index.js" ]; then
  git -C "$root" worktree add -q --detach "$B/base-tree" "$BASE" || exit 1
  CARGO_TARGET_DIR="$B/target-base" build "$B/base-tree" edge edge
  git -C "$root" worktree remove --force "$B/base-tree"
fi
ln -sfn "$B/site" "$B/out"
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
r = edit(r, '"main": f"{b}/edge/index.js"', '"main": f"{b}/edge/index.js" if n == 0 else f"{b}/edgehead/index.js"')
r = edit(r, '32.12,32.14,18.3"', '32.12,32.14,18.3,32.29"')
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
F=http://127.0.0.1:$P
node "$here/drive.mjs" "$F" "$B"; r=$?
echo "bf77bed: $([ $r = 0 ] && echo 'base as main, head BF-77 lines' || echo FAIL)"
[ $r = 0 ]
