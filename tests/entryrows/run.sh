#!/usr/bin/env bash
# Rows written per member per entry, as Cloudflare bills them, counted by local workerd's per-statement
# `rows_written()` (`bed/src/lib.rs`). Prints one JSON line per case: each `fold_corpus.json` kind alone, the push and
# agent-push entries, the `DefineLabel` entry K12 measured hosted, every kind again with `WITHOUT ROWID` tier tables
# (`wr`), a blob-fold family's entry, the later deletes, and the test-only `arrival` row. `scripts/cost.py`'s
# `REPO_KIND_ROWS`, `BLOB_ENTRY_ROWS` and `ENTRY_DELETE_ROWS` cite this output. Needs worker-build and tests/m0's
# wrangler. About two minutes. OUT=path also writes the lines there.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
P=${P:-8799}; W=$root/tests/m0/node_modules/.bin/wrangler
# The bed's member DDL must be the product's, verbatim.
for s in "CREATE TABLE IF NOT EXISTS raft_state" "CREATE TABLE IF NOT EXISTS log " "CREATE TABLE IF NOT EXISTS fold " \
	"CREATE TABLE IF NOT EXISTS request " "CREATE INDEX IF NOT EXISTS request_at"; do
	line=$(grep -F "\"$s" "$here/bed/src/lib.rs" | sed 's/^ *//')
	grep -qF "$line" "$root/workers/site/src/host.rs" || { echo "bed DDL differs from host.rs SCHEMA: $line"; exit 1; }
done
for s in "CREATE TABLE IF NOT EXISTS arrival" "CREATE INDEX IF NOT EXISTS arrival_at"; do
	line=$(grep -F "\"$s" "$here/bed/src/lib.rs" | sed 's/^ *//')
	grep -qF "$line" "$root/workers/site/src/faults.rs" || { echo "bed DDL differs from faults.rs SCHEMA: $line"; exit 1; }
done
B=$(mktemp -d); cp -r "$here/bed/Cargo.toml" "$here/bed/wrangler.toml" "$here/bed/src" "$B/"
sed -i "s#\.\./\.\./\.\./\.\./\(workers\|crates\)#$root/\1#g" "$B/src/lib.rs"
sed -i "s#\.\./\.\./\.\./crates#$root/crates#g" "$B/Cargo.toml"
cd "$B"
CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$B/target} worker-build --release >"$B/build.log" 2>&1 \
	|| { tail -20 "$B/build.log"; exit 1; }
# Its own session: wrangler's shutdown signals its whole process group.
setsid "$W" dev --port "$P" --persist-to "$B/state" >"$B/dev.log" 2>&1 & DEV=$!
trap 's=$?; kill -- -$DEV 2>/dev/null; wait $DEV 2>/dev/null || true; rm -rf "$B"; exit $s' EXIT
until curl -s "http://127.0.0.1:$P/nope?o=x" 2>/dev/null | grep -q "not found"; do sleep 1; done
kinds=$(python3 -c "import json; print(' '.join(next(iter(e)) for e in json.load(open('$root/crates/git/tests/fold_corpus.json'))['events']))")
n=0
case_() {  # name, query
	n=$((n + 1))
	got=$(curl -s "http://127.0.0.1:$P$2&o=o$n$RANDOM")
	case $got in \{*) ;; *) echo "case $1 failed: $got"; exit 1;; esac
	line="{\"case\": \"$1\", \"rows\": $got}"
	echo "$line"; [ -n "${OUT:-}" ] && echo "$line" >>"$OUT"; true
}
[ -n "${OUT:-}" ] && : >"$OUT"
# Entries of more than one event: an agent push (`RepoCmd::Push` adds `Provenance` and `PathSet` to its `Refs`) and a
# PrSync round (merge-ref `Refs`, `TestMerged`, `ConsumerCursor`, `crates/git/src/prsync.rs`).
combos="Refs,Provenance,PathSet Refs,TestMerged,ConsumerCursor"
for wr in 0 1; do
	p=""; [ $wr = 1 ] && p="wr "
	for k in $kinds $combos; do case_ "$p$k" "/repo?kind=$k&wr=$wr"; done
	case_ "${p}DefineLabel (K12)" "/repo?setup=4&wr=$wr"
done
case_ "blob family entry" "/blob?"
case_ "later deletes" "/deletes?"
case_ "test-only arrival" "/arrival?"
# Storage per row as built and `WITHOUT ROWID`, at a snapshot's 65 B, ROW_KB's 620 B, and comment-sized rows.
for b in 65 620 1000 2000 3000 4700; do
	for wr in 0 1; do case_ "size b=$b wr=$wr" "/size?b=$b&wr=$wr"; done
done
