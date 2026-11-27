#!/usr/bin/env bash
# plaindiff: a local byte diff of two edge builds for a client that does not opt in to the agent layer (AGENTS.md, the
# GHES invariant), in one local `wrangler dev` at $0. It uses coldwrite's bed (`tests/coldwrite`: three site scripts on
# the site's own build, its seed, its member stand-in) with two edge scripts: edge 0 runs the base build, edge 1 the
# head build. `diff.mjs` sends the same plain-PAT git, REST and GraphQL requests to both and compares status, headers
# and body. No git cache shard runs, so a fetch reaches the edge's shard-read failure branch.
#   BASE=<rev> bash tests/plaindiff/run.sh [ALLOW]    # BASE defaults to origin/main; head is the working tree
#   B=<dir> bash tests/plaindiff/run.sh [ALLOW]       # reuse builds already in <dir>/{site,edge,edgehead}
# ALLOW is passed to diff.mjs (`fetch=2` for edge-shed-429). Builds both edges and the site with test-faults (the seed
# needs `/_bed/identity`). Needs worker-build, git and tests/m0's wrangler. Run from the repository root.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd); cw=$root/tests/coldwrite
P=${P:-8793}; W=${WRANGLER:-$root/tests/m0/node_modules/.bin/wrangler}; BASE=${BASE:-origin/main}
[ -x "$W" ] || { echo "no wrangler at $W"; exit 1; }
B=${B:-$(mktemp -d)}; cp "$cw/site.mjs" "$B/"
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
# coldwrite's front with REST and GraphQL paths sent to the install's HOST, which the edge's API routes match on; and
# coldwrite's bed for two edges, edge 1 on the head build, keeping this front. Each edit must match once.
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
r = edit(r, 'here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)', f"here={cw}; root={root}")
r = edit(r, 'B=${B:-$(mktemp -d)}; cp "$here"/site.mjs "$here"/driver.mjs "$B/"', 'B=${B:-$(mktemp -d)}')
open(f"{b}/bed.sh", "w").write(r)
PY
# A fresh bed each run: the seed creates its rows once. coldwrite's bed starts wrangler with setsid and, under KEEP,
# leaves it up, so this stops it by its process group, also when the seed fails.
rm -rf "$B/state" "$B/seed.json"
P=$P B=$B EDGES=2 KEEP=1 bash "$B/bed.sh" >"$B/bed.log" 2>&1; tail -3 "$B/bed.log"
pid=$(pgrep -f -n "wrangler dev --port $P ")
stop() { [ -n "$pid" ] && { kill -- -"$pid" 2>/dev/null || kill "$pid" 2>/dev/null; }; }
trap stop EXIT
[ -f "$B/seed.json" ] || { echo "the bed did not seed"; exit 1; }
node "$here/seed2.mjs" "http://127.0.0.1:$P" "$B" || exit 1
node "$here/diff.mjs" "http://127.0.0.1:$P" "$B" "${1:-}"
