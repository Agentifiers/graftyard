#!/usr/bin/env bash
# BM-3's first clause on the graft-m1 install: journey 4 over HTTPS with a stock git client, `rounds` times
# (`design/DESIGN.md`, journey 4; `build/BUILD-PLAN.md`, BM-3). Each round, against m3org/j4:
#   1. a fresh PAT for m3-member, which pushes (accepted);
#   2. revoke it (`DELETE /applications/{gh client}/token`, 204), then push again with it: must be refused (401);
#   3. a fresh `ghs_` from the App's installation (`POST /app/installations/{id}/access_tokens`), which pushes
#      (accepted); `DELETE /installation/token` (204); push again with it: must be refused (401);
# and once, at the end: remove m3-member from m3org (`DELETE /orgs/m3org/memberships/m3-member`, 204), then push
# with a live PAT of theirs: must be refused (403, or 404 for the private repository).
# A refusal counts only if git exits non-zero and the remote ref did not move. Tokens reach git through a credential
# helper reading a 0600 file and curl through stdin, never argv. Writes <out>/journey4.tsv and a summary line.
#
#   tests/m3/hosted/journey4.sh <state dir> <out dir> <rounds>
set -uo pipefail
state=${1:?state}; out=${2:?out}; rounds=${3:-100}
here=$(cd "$(dirname "$0")" && pwd)
EDGE=${GRAFT_EDGE:-https://graft-m1-edge.<workers-subdomain>.workers.dev}
GH_CLIENT=178c6fc778ccc68e1d6a
mkdir -p "$out"; umask 077
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
tok=$work/tok
cat > "$work/cred.sh" <<'EOF'
#!/bin/sh
[ "$1" = get ] || exit 0
echo username=x
echo "password=$(cat "$GRAFT_TOKFILE")"
EOF
chmod +x "$work/cred.sh"
export GRAFT_TOKFILE=$tok GIT_TERMINAL_PROMPT=0
g() { git -c credential.helper= -c credential.helper="$work/cred.sh" -c user.name=j4 -c user.email=j4@x "$@"; }
api() { # api METHOD PATH [body]: the token in $tok; prints the status, the body to $work/body
  curl -s -m 120 -o "$work/body" -w '%{http_code}' -X "$1" -H @- ${3:+-d "$3"} "$EDGE$2" <<<"authorization: token $(cat "$tok")"
}
remote_head() { g ls-remote "$EDGE/m3org/j4.git" refs/heads/main 2>/dev/null | cut -f1; }

# A clone to push from, as the admin.
awk '$1=="admin"{print $2}' "$state/m3-tokens.txt" > "$tok"
g clone -q "$EDGE/m3org/j4.git" "$work/j4" 2>"$work/clone.err" || true
cd "$work/j4" || exit 1
if ! git rev-parse -q --verify HEAD >/dev/null; then
  git checkout -q -b main; echo j4 > README; git add README; g commit -qm init
  g push -q origin main 2>>"$work/clone.err" || { cat "$work/clone.err"; exit 1; }
fi

# A fresh PAT for m3-member, stored and listed through the tools edge as the operator (as the seed does).
new_pat() {
  node "$here/newpat.mjs" "$state" member > "$tok"
}
# A fresh `ghs_` from the installation, with the App's JWT.
new_ghs() {
  node "$here/ghs.mjs" "$state" > "$tok"
}
push_one() { # push_one LABEL: a new commit on main; prints "ok" or "refused <http>" and whether the ref moved
  local before after rc
  before=$(git rev-parse HEAD)
  echo "$1 $(date +%s%N)" >> log.txt; git add log.txt; g commit -qm "$1"
  g push -q origin HEAD:main 2>"$work/push.err"; rc=$?
  after=$(remote_head)
  if [ $rc -eq 0 ] && [ "$after" = "$(git rev-parse HEAD)" ]; then
    echo ok
  else
    git reset -q --hard "$before"
    # git prints "Authentication failed" for a 401 it cannot retry with the helper's credential.
    local why; why=$(grep -oE "(401|403|404|503|Authentication failed|Bad credentials|Permission denied|not found)" "$work/push.err" | head -1)
    [ "$why" = "Authentication failed" ] && why=401
    echo "refused ${why:-rc$rc} moved=$([ "$after" = "$before" ] && echo no || echo yes)"
  fi
}

printf 'round\tstep\tresult\n' > "$out/journey4.tsv"
pat_ok=0; pat_refused=0; ghs_ok=0; ghs_refused=0; rev204=0; jti204=0
for r in $(seq 1 "$rounds"); do
  g pull -q --rebase origin main 2>/dev/null
  new_pat; a=$(push_one "pat-$r"); printf '%s\tpat push\t%s\n' "$r" "$a" >> "$out/journey4.tsv"; [ "$a" = ok ] && pat_ok=$((pat_ok+1))
  s=$(node "$here/revoke.mjs" "$tok" "$GH_CLIENT")
  printf '%s\tpat revoke\t%s\n' "$r" "$s" >> "$out/journey4.tsv"; [ "$s" = 204 ] && rev204=$((rev204+1))
  b=$(push_one "pat-revoked-$r"); printf '%s\tpat push after revoke\t%s\n' "$r" "$b" >> "$out/journey4.tsv"
  case $b in "refused "*"moved=no") pat_refused=$((pat_refused+1));; esac
  new_ghs; a=$(push_one "ghs-$r"); printf '%s\tghs push\t%s\n' "$r" "$a" >> "$out/journey4.tsv"; [ "$a" = ok ] && ghs_ok=$((ghs_ok+1))
  s=$(api DELETE /api/v3/installation/token); printf '%s\tghs revoke\t%s\n' "$r" "$s" >> "$out/journey4.tsv"; [ "$s" = 204 ] && jti204=$((jti204+1))
  b=$(push_one "ghs-revoked-$r"); printf '%s\tghs push after revoke\t%s\n' "$r" "$b" >> "$out/journey4.tsv"
  case $b in "refused "*"moved=no") ghs_refused=$((ghs_refused+1));; esac
done
# Member removal, once, with a live PAT.
new_pat; a=$(push_one "member-before"); printf 'end\tmember push before removal\t%s\n' "$a" >> "$out/journey4.tsv"
member_pat=$(cat "$tok")
awk '$1=="admin"{print $2}' "$state/m3-tokens.txt" > "$tok"
s=$(api DELETE /api/v3/orgs/m3org/memberships/m3-member); printf 'end\tremove member\t%s\n' "$s" >> "$out/journey4.tsv"
printf '%s' "$member_pat" > "$tok"
b=$(push_one "member-after"); printf 'end\tmember push after removal\t%s\n' "$b" >> "$out/journey4.tsv"
# Put the member back for the next run.
awk '$1=="admin"{print $2}' "$state/m3-tokens.txt" > "$tok"
s=$(api PUT /api/v3/orgs/m3org/memberships/m3-member '{"role":"member"}'); printf 'end\treadd member\t%s\n' "$s" >> "$out/journey4.tsv"
echo "rounds=$rounds pat_push_ok=$pat_ok pat_revoke_204=$rev204 pat_refused=$pat_refused ghs_push_ok=$ghs_ok jti_revoke_204=$jti204 ghs_refused=$ghs_refused member_after=[$b]" | tee "$out/journey4.summary"
