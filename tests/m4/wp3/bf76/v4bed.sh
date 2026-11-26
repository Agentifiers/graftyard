#!/usr/bin/env bash
# tests/edgeboot's BF-76 v4 check: `upload-artifact@v4` and `download-artifact@v4`, as the forks run them, against a
# local edge started with `GRAFTYARD_BED_TWIRP=1` (`workers/edge/src/artifacts.rs`, `bed_twirp`). Args: a scratch dir
# and the front's port. The bundles are upstream's `dist/` at the pinned tags, patched as build-forks.sh patches them.
set -uo pipefail
B=$1; FP=$2
here=$(cd "$(dirname "$0")" && pwd)
cache=${BF76_BUNDLES:-$B/bundles}
mkdir -p "$cache" "$B/v4"
fetch() { # repo sha path out
  [ -s "$cache/$4" ] && return 0
  curl -sfL -m 120 -o "$cache/$4" "https://raw.githubusercontent.com/actions/$1/$2/$3"
}
fetch upload-artifact ea165f8d65b6e75b540449e92b4886f43607fa02 dist/upload/index.js up.js \
  && fetch download-artifact d3f86a106a0bac45b974a628896c90dbdf5c8093 dist/index.js down.js \
  || { echo "v4: could not fetch the upstream bundles"; exit 1; }
python3 - "$cache" "$B/v4" <<'PY'
import sys
src, dst = sys.argv[1], sys.argv[2]
old = "    return !isGitHubHost && !isGheHost && !isLocalHost;\n"
new = "    if (process.env['GRAFTYARD_ACTIONS'] === 'true') {\n        return false;\n    }\n" + old
for f in ("up.js", "down.js"):
    s = open(f"{src}/{f}", encoding="utf-8").read()
    assert s.count(old) == 1, f
    open(f"{dst}/{f}", "w", encoding="utf-8").write(s.replace(old, new))
PY
TOK=$(node "$here/bedtoken.js")
W=$B/v4/ws; rm -rf "$W"; mkdir -p "$W/files/sub" "$W/tmp"
head -c 20000000 /dev/urandom >"$W/files/big.bin"; echo "v4 bed" >"$W/files/sub/a.txt"; : >"$W/files/empty"
run() { # bundle, then INPUT_* pairs
  local js=$1; shift
  (cd "$W" && env -u HTTP_PROXY -u HTTPS_PROXY -u http_proxy -u https_proxy GRAFTYARD_ACTIONS=true \
    GITHUB_SERVER_URL=https://edgeboot.example GITHUB_REPOSITORY=o/r GITHUB_WORKSPACE="$W" RUNNER_TEMP="$W/tmp" \
    GITHUB_OUTPUT="$W/out" ACTIONS_RUNTIME_TOKEN="$TOK" ACTIONS_RESULTS_URL="http://127.0.0.1:$FP/_services/results/" \
    "$@" timeout 180 node "$B/v4/$js")
}
ok=1
: >"$W/out"
run up.js INPUT_NAME=art INPUT_PATH=files INPUT_IF-NO-FILES-FOUND=error INPUT_RETENTION-DAYS=3 \
  INPUT_COMPRESSION-LEVEL=0 INPUT_OVERWRITE=false INPUT_INCLUDE-HIDDEN-FILES=false >"$B/v4/up.log" 2>&1
grep -q "Artifact art has been successfully uploaded" "$B/v4/up.log" || { tail -5 "$B/v4/up.log"; ok=0; }
grep -o "Final size is [0-9]* bytes" "$B/v4/up.log"
run up.js INPUT_NAME=art INPUT_PATH=files INPUT_IF-NO-FILES-FOUND=error INPUT_COMPRESSION-LEVEL=0 \
  INPUT_OVERWRITE=false INPUT_INCLUDE-HIDDEN-FILES=false >"$B/v4/up2.log" 2>&1
grep -q "already exists" "$B/v4/up2.log" && echo "v4: a second upload of art is refused" \
  || { echo "v4: a second upload of art was not refused"; tail -3 "$B/v4/up2.log"; ok=0; }
run down.js INPUT_NAME=art INPUT_PATH=got INPUT_REPOSITORY=o/r INPUT_RUN-ID=3 INPUT_MERGE-MULTIPLE=false \
  >"$B/v4/down.log" 2>&1
grep -q "Download artifact has finished successfully" "$B/v4/down.log" || { tail -5 "$B/v4/down.log"; ok=0; }
grep -q "digest validation failed" "$B/v4/down.log" && { echo "v4: the digest did not match"; ok=0; }
for f in big.bin sub/a.txt empty; do
  cmp -s "$W/files/$f" "$W/got/$f" || { echo "v4: $f differs"; ok=0; }
done
[ $ok = 1 ] && echo "v4 artifacts ok: $(sha256sum "$W/got/big.bin" | cut -c1-16) big.bin, a.txt and empty identical"
[ $ok = 1 ]
