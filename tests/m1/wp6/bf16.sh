#!/bin/bash
# BF-16 (build/BUILD-PLAN.md): "Edge on N+1 with a new message format that is not enabled, plus one forced unknown
# variant. Pass: nothing sent early; unknown variant gives 503 with Retry-After and no state change."
#
# The edge build knows Push format 32.2 (REPO_FORMAT_V2, which adds the client's agent): it is release N+1. The
# stand-in members of bed/bf16 are release N: their `reads` is [32.1] (seed.sh). FORMATS_ENABLED=32.1 is what the
# Release group's `FormatsEnable` would have enabled. Phase 1 pushes with the edge as deployed and reads the members'
# receive counters: every command must arrive as 32.1. Phase 2 redeploys the same build with FORCE_FORMAT (a test
# fault that sends every push in the given format) at 32.9, a format no release reads, then at 32.2, the format N+1
# knows and N does not, and checks each gives 503 with Retry-After and changes nothing. The edge is redeployed as it
# was at the end.
#
# Usage: bf16.sh <host> <owner/repo> <repo id> <write token>; WP6_DIR holds bed.token. Run from the repository root
# after `worker-build --release` in workers/edge.
set -uo pipefail
HOST=$1 REPO=$2 ID=$3 TOK=$4
BED=$(cat "$WP6_DIR/bed.token")
RUN=r$(date +%s)
W=$(mktemp -d)
ROOT=$PWD
trap 'rm -rf "$W"' EXIT
U="https://x:$TOK@$HOST/$REPO.git"
fails=0
ok() { echo "PASS $1"; }
no() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
g() { git -c user.name=wp6 -c user.email=wp6@graftyard.invalid -c credential.helper= "$@"; }
bed() { curl -s -X POST -H "x-bed-token: $BED" "https://$HOST/_bed/$1?repo=$ID${2:-}"; }
events() { bed log | python3 -c '
import json,sys; print(sum(1 for e in json.load(sys.stdin) if e["kind"]=="events" or e["requests"]))'; }
recv() { bed status | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["received"], sort_keys=True))'; }
deploy() {
  (cd "$ROOT/workers/edge" && npx --prefix "$ROOT/tests/m0" wrangler deploy "$@" 2>&1 | grep -E "Version ID|FORCE")
  sleep 15
}
echo "client: $(git --version), run $RUN"

g init -q -b main "$W/src" && cd "$W/src" || exit 2
g commit -q --allow-empty -m "$RUN 0"
g push -q "$U" "HEAD:refs/heads/$RUN/main" 2>/dev/null

# Phase 1: N+1 edge, 32.2 not enabled. Three pushes.
r0=$(recv)
for i in 1 2 3; do
  g commit -q --allow-empty -m "$RUN $i"
  out=$(g push --porcelain "$U" "HEAD:refs/heads/$RUN/main" 2>&1)
  grep -q '^ ' <<<"$out" || no "phase 1 push $i" "$out"
done
r1=$(recv)
echo "  receive counters before: $r0"
echo "  receive counters after:  $r1"
python3 - "$r0" "$r1" <<'PY' && ok "nothing sent early: 3 more commands at 32.1, none at 32.2" || no "nothing sent early" "$r0 -> $r1"
import json, sys
a, b = (json.loads(x) for x in sys.argv[1:])
d = {k: int(b.get(k, 0)) - int(a.get(k, 0)) for k in set(a) | set(b)}
sys.exit(0 if d.get("recv:32.1") == 3 and not d.get("recv:32.2") and not d.get("recv:unsupported") else 1)
PY
[ "$(g ls-remote "$U" "refs/heads/$RUN/main" | cut -f1)" = "$(g rev-parse HEAD)" ] && ok "phase 1 pushes committed" ||
  no "phase 1 pushes committed" "remote differs"

# Phase 2: forced variants.
for f in 32.9 32.2; do
  deploy --var "FORCE_FORMAT:$f"
  e0=$(events) refs0=$(bed refs) r0=$(recv)
  g commit -q --allow-empty -m "$RUN forced $f"
  out=$(GIT_TRACE_CURL=1 GIT_TRACE_CURL_NO_DATA=1 g push --porcelain "$U" "HEAD:refs/heads/$RUN/main" 2>&1)
  status=$(grep -oE "Recv header: HTTP/2 [0-9]+" <<<"$out" | tail -1 | grep -oE "[0-9]+$")
  retry=$(grep -oiE "Recv header: retry-after: [0-9]+" <<<"$out" | tail -1 | grep -oE "[0-9]+$")
  e1=$(events) refs1=$(bed refs) r1=$(recv)
  echo "  forced $f: HTTP $status, Retry-After $retry; git: $(grep -m1 -E '^error:|^fatal:' <<<"$out")"
  echo "    counters $r0 -> $r1"
  [ "$status" = 503 ] && [ -n "$retry" ] && ok "forced $f: 503 with Retry-After: $retry" ||
    no "forced $f: 503 with Retry-After" "status $status retry '$retry'"
  [ "$e0" = "$e1" ] && [ "$refs0" = "$refs1" ] && ok "forced $f: no entry, refs unchanged" ||
    no "forced $f: no state change" "events $e0 -> $e1"
  grep -q '^ \|^\*' <<<"$out" && no "forced $f: no ref reported ok" "$out" || ok "forced $f: no ref reported ok"
  g reset -q --hard HEAD~1
done

deploy
g commit -q --allow-empty -m "$RUN after"
out=$(g push --porcelain "$U" "HEAD:refs/heads/$RUN/main" 2>&1)
grep -q '^ ' <<<"$out" && ok "after redeploy without the fault, a push commits" || no "after redeploy" "$out"

echo "fails: $fails"
exit $((fails > 0))
