#!/usr/bin/env bash
# The fold audit's SQLITE_LOCKED in local workerd (`src/lib.rs`): per arm, a fresh object seeded with 200 log rows,
# the archive alarm's walks in one request, the audit's DROP TABLE in the next; and `each_row` stopped after one row.
# Exits 1 unless arm=fix drops cleanly every time. ARM=main also expects main's reads to fail it (exit 1 otherwise).
# Then the page bound: 1,000 short rows and 200 of 300 KB, walked in a later request; fails unless every row comes back
# once and the walk grew wasm memory by under 16 MiB (main's pages fetched about 55 MB at once). LOGREAD=<file> builds
# that logread.rs in place of this branch's, to show the check failing on another version.
# Needs worker-build and tests/m0's wrangler.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../../.." && pwd)
P=${P:-8797}; W=$root/tests/m0/node_modules/.bin/wrangler
B=$(mktemp -d); cp -r "$here/Cargo.toml" "$here/wrangler.toml" "$here/src" "$B/"
sed -i "s#\.\./\.\./\.\./crates#$root/crates#; " "$B/Cargo.toml"
sed -i "s#\.\./\.\./\.\./\.\./workers#$root/workers#" "$B/src/lib.rs"
if [ -n "${LOGREAD:-}" ]; then cp "$LOGREAD" "$B/logread.rs"; sed -i "s#$root/workers/site/src/logread.rs#$B/logread.rs#" "$B/src/lib.rs"; fi
cd "$B" && CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$B/target} worker-build --release >"$B/build.log" 2>&1 \
  || { tail -20 "$B/build.log"; exit 1; }
"$W" dev --port $P >"$B/dev.log" 2>&1 & DEV=$!
until curl -s "http://127.0.0.1:$P/nope?o=x" 2>/dev/null | grep -q "not found"; do sleep 1; done
u() { curl -s "http://127.0.0.1:$P$1"; }
ok=1
for arm in ${ARMS:-main fix}; do
  o=$arm-$RANDOM; u "/seed?o=$o" >/dev/null
  walk=$(u "/walk?o=$o&arm=$arm"); d1=$(u "/drop?o=$o"); d2=$(u "/drop?o=$o")
  u "/seed?o=e$o" >/dev/null; each=$(u "/each?o=e$o&arm=$arm")
  echo "arm=$arm walk: $walk | audit drop: $d1 | again: $d2 | each_row: $each"
  if [ "$arm" = fix ]; then [ "$d1" = ok ] && [ "$d2" = ok ] && [ "${each##*drop=}" = ok ] || ok=0; fi
  if [ "$arm" = main ]; then [ "$d1" != ok ] && [ "${each##*drop=}" != ok ] || ok=0; fi
done
u "/seedbig?o=big" >/dev/null; pages=$(u "/pages?o=big"); echo "pages: $pages"
case "$pages" in rows=1200\ bytes=61500000\ grew_mb=*) [ "${pages##*grew_mb=}" -lt 16 ] || ok=0 ;; *) ok=0 ;; esac
kill $DEV; wait $DEV 2>/dev/null; rm -rf "$B"
[ $ok = 1 ] && echo PASS || { echo FAIL; exit 1; }
