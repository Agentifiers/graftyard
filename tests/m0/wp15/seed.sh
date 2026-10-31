#!/usr/bin/env bash
# M0-WP15: seed one deployment identically (fixed git dates, so both get the same object ids): main and feature
# branches by git push as alice, then issue #1 and PR #2 by REST. Usage: wp15/seed.sh <origin> <alice token>
set -e
H=$1; T=$2
export GIT_AUTHOR_DATE="1790000000 +0000" GIT_COMMITTER_DATE="1790000000 +0000" GIT_AUTHOR_NAME=m0 GIT_AUTHOR_EMAIL=m0@example.com GIT_COMMITTER_NAME=m0 GIT_COMMITTER_EMAIL=m0@example.com
D=$(mktemp -d); cd "$D"; git init -q -b main
mkdir -p pkg/auth; echo "package auth" > pkg/auth/session.go; echo "# mono" > README.md; git add .; git commit -qm "seed"
git push -q "https://x:$T@${H#https://}/acme/mono.git" main
git checkout -qb feature; echo "x" >> README.md; git commit -qam "feature"; git push -q "https://x:$T@${H#https://}/acme/mono.git" feature
cd /; rm -rf "$D"
curl -sf -o /dev/null -X POST "$H/api/v3/repos/acme/mono/issues" -H "authorization: Bearer $T" -d '{"title":"First issue","body":"Hello <!-- hidden -->"}'
curl -sf -o /dev/null -X POST "$H/api/v3/repos/acme/mono/pulls" -H "authorization: Bearer $T" -d '{"head":"feature","base":"main","title":"Feature","body":"pr body"}'
echo "seeded $H"
