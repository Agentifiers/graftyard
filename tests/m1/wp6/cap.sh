#!/bin/bash
# The edge's own cap (EDGE_CAP_BYTES, 80 MB on graft-m1-edge) below the plan limit (Pro and workers.dev: 100 MB,
# tests/m0/results/m0-wp6.md): a push of about 85 MB streams past the cap, and git must print the edge's refusal,
# which names HTTPS as the default, with ng on every ref and no ref written. One over 100 MB is answered by
# Cloudflare with 413 before the edge sees it; that row was measured in M0-WP6 and is not repeated here.
#
# Usage: cap.sh <host> <owner/repo> <write token>
set -uo pipefail
HOST=$1 REPO=$2 TOK=$3
RUN=r$(date +%s)
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
U="https://x:$TOK@$HOST/$REPO.git"
fails=0
ok() { echo "PASS $1"; }
no() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
g() { git -c user.name=wp6 -c user.email=wp6@graftyard.invalid -c credential.helper= "$@"; }
echo "client: $(git --version), run $RUN"
g init -q -b main "$W/src" && cd "$W/src" || exit 2
head -c 85000000 /dev/urandom > big && g add big && g commit -qm "$RUN 85 MB"
g branch "$RUN/second" && g branch "$RUN/third"
out=$(g push --porcelain "$U" "HEAD:refs/heads/$RUN/main" "$RUN/second" "$RUN/third" 2>&1)
echo "$out" | grep -E "^remote:|^!|^error|^fatal" | sed 's/[[:space:]]*$//'
grep -q "remote: Push refused: over 80 MB" <<<"$out" && ok "the edge's refusal reaches git" || no "refusal" "$out"
grep -q "HTTPS stays the default" <<<"$out" && ok "the message keeps HTTPS as the default" || no "HTTPS default" "-"
grep -q "Split it into smaller pushes, or ask your admin about the Enterprise plan" <<<"$out" &&
  ok "Pro with the SSH option off: split, or Enterprise" || no "Pro message" "-"
[ "$(grep -c '^!' <<<"$out")" = 3 ] && ok "ng on all 3 refs" || no "ng on every ref" "$(grep -c '^!' <<<"$out")"
[ -z "$(g ls-remote "$U" "refs/heads/$RUN/*")" ] && ok "no ref written" || no "no ref written" "refs exist"
echo "fails: $fails"
exit $((fails > 0))
