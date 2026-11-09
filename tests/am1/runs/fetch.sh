#!/usr/bin/env bash
# B0-37's fetch-half smoke on the new shard image, for what graft-m2b can run without 32.12/32.14 and `OPS`: as
# m3-admin, <rounds> times against m3org/<repo>, a writer push to `refs/graftyard/sessions/<x>` (refused), then
# `ls-remote` over v0 and v2, `clone --mirror`, and a raw v2 `ls-refs` with `ref-prefix refs/graftyard/sessions/`,
# each counting the `refs/graftyard/` lines advertised. No session ref can exist here (only `graftyard-workspace`
# with a live task writes one), so this checks the hiding and the refusal, not the session-ref listing.
# git reads the token from a credential helper over a 0600 file, curl from stdin; never argv.
#   bash tests/am1/runs/fetch.sh <repo> <rounds> <out.tsv>
set -uo pipefail
REPO=${1:?repo}; ROUNDS=${2:-10}; OUT=${3:?out}
source "$(dirname "$0")/../../m3/rows/env.sh"
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT; umask 077
awk '$1=="admin"{print $2}' "$M3_STATE/m3-tokens.txt" | tr -d '\n' > "$W/tok"
cat > "$W/cred.sh" <<'EOF'
#!/bin/sh
[ "$1" = get ] || exit 0
echo username=x
echo "password=$(cat "$GRAFT_TOKFILE")"
EOF
chmod +x "$W/cred.sh"
export GRAFT_TOKFILE=$W/tok GIT_TERMINAL_PROMPT=0
g() { git -c credential.helper= -c credential.helper="$W/cred.sh" -c user.name=b037 -c user.email=b037@x "$@"; }
URL=$GRAFT_EDGE/m3org/$REPO.git
g clone -q "$URL" "$W/wt" 2>"$W/err" || { cat "$W/err" >&2; exit 2; }
main=$(git -C "$W/wt" rev-parse HEAD)
pkt() { printf '%04x%s' $((${#1} + 4)) "$1"; }
printf 'round\tpush_refs_graftyard\tv0_refs\tv0_graftyard\tv2_refs\tv2_graftyard\tmirror_refs\tmirror_graftyard\tlsrefs_session_prefix_status\tlsrefs_session_lines\n' > "$OUT"
for r in $(seq 1 "$ROUNDS"); do
  p=$(g -C "$W/wt" push origin "$main:refs/graftyard/sessions/b037-$r-$$" 2>&1); prc=$?
  [ $prc -eq 0 ] && push=accepted || push="refused:$(grep -oE '\[remote rejected\][^\n]*|! \[[a-z ]*\][^\n]*' <<<"$p" | head -1 | tr '\t' ' ' | cut -c1-80)"
  v0=$(g -c protocol.version=0 ls-remote "$URL" 2>/dev/null); v2=$(g -c protocol.version=2 ls-remote "$URL" 2>/dev/null)
  rm -rf "$W/m"; g clone -q --mirror "$URL" "$W/m" 2>/dev/null; mr=$(git -C "$W/m" for-each-ref --format='%(refname)' 2>/dev/null)
  body=$(printf '%s0001%s0000' "$(pkt 'command=ls-refs
')" "$(pkt 'ref-prefix refs/graftyard/sessions/
')")
  code=$(curl -s -m 60 -o "$W/ls" -w '%{http_code}' -X POST -H @- -H 'content-type: application/x-git-upload-pack-request' \
    -H 'git-protocol: version=2' --data-binary "$body" "$URL/git-upload-pack" <<<"authorization: Basic $(printf 'x:%s' "$(cat "$W/tok")" | base64 -w0)")
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$r" "$push" \
    "$(grep -c . <<<"$v0")" "$(grep -c 'refs/graftyard/' <<<"$v0")" "$(grep -c . <<<"$v2")" "$(grep -c 'refs/graftyard/' <<<"$v2")" \
    "$(grep -c . <<<"$mr")" "$(grep -c 'refs/graftyard/' <<<"$mr")" "$code" "$(grep -ac 'refs/graftyard/' "$W/ls")" >> "$OUT"
done
