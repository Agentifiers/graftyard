#!/usr/bin/env bash
# BM-3 on the graft-m1 install at M3's close: journey 4 over HTTPS with a stock git client, `rounds` times, with the
# member's removal inside the run (`build/BUILD-PLAN.md`, BM-3; `design/DESIGN.md`, journey 4). journey4.sh's round,
# against m3org/j4:
#   1. a fresh PAT for m3-member pushes (accepted); it is revoked (204); a push with it must be refused (401);
#   2. a fresh `ghs_` from installation 7190001 pushes (accepted); `DELETE /installation/token` (204); a push with it
#      must be refused (401);
# and at round `remove_at`: m3-admin removes m3-member from m3org (204), a push with the member's live PAT must be
# refused (403 or 404), and m3-admin adds the member back (200), so the rounds after it run as before.
# A refusal counts only if git exits non-zero and the remote ref did not move.
#
# Each revoke's curl writes its timing (`-w`: connect, TLS, first byte, total, remote IP) to <out>/revokes.tsv, so a
# revoke that gets no answer (round 97 of the last run) shows where it stopped. Such a revoke is sent once more under
# the same token, which `DELETE /installation/token` treats as a second delete of one token (204), and both attempts
# are kept; the push after it then tests the token as the second attempt left it.
# Tokens reach git through a credential helper reading a 0600 file and curl through stdin, never argv.
#
#   tests/m3/hosted/journey4-close.sh <state dir> <out dir> <rounds> [remove_at]
set -uo pipefail
state=${1:?state}; out=${2:?out}; rounds=${3:-100}; remove_at=${4:-50}
here=$(cd "$(dirname "$0")" && pwd)
EDGE=${GRAFT_EDGE:-https://graft-m1-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev}
GH_CLIENT=178c6fc778ccc68e1d6a
mkdir -p "$out"; out=$(cd "$out" && pwd); umask 077
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
W='%{http_code}\t%{time_connect}\t%{time_appconnect}\t%{time_starttransfer}\t%{time_total}\t%{remote_ip}\t%{exitcode}'
api() { # api METHOD PATH [body]: the token in $tok; prints curl's -w line, the body to $work/body
  curl -s -m 120 -o "$work/body" -w "$W" -X "$1" -H @- ${3:+-d "$3"} "$EDGE$2" <<<"authorization: token $(cat "$tok")"
}
remote_head() { g ls-remote "$EDGE/m3org/j4.git" refs/heads/main 2>/dev/null | cut -f1; }

awk '$1=="admin"{print $2}' "$state/m3-tokens.txt" > "$tok"
g clone -q "$EDGE/m3org/j4.git" "$work/j4" 2>"$work/clone.err" || { cat "$work/clone.err"; exit 1; }
cd "$work/j4" || exit 1

new_pat() { node "$here/newpat.mjs" "$state" member > "$tok"; }
new_ghs() { node "$here/ghs.mjs" "$state" > "$tok"; }
push_one() { # push_one LABEL: a new commit on main; prints "ok" or "refused <http> moved=<yes|no>"
  local before after rc
  before=$(git rev-parse HEAD)
  echo "$1 $(date +%s%N)" >> log.txt; git add log.txt; g commit -qm "$1"
  g push -q origin HEAD:main 2>"$work/push.err"; rc=$?
  after=$(remote_head)
  if [ $rc -eq 0 ] && [ "$after" = "$(git rev-parse HEAD)" ]; then
    echo ok
  else
    git reset -q --hard "$before"
    local why; why=$(grep -oE "(401|403|404|503|Authentication failed|Bad credentials|Permission denied|not found)" "$work/push.err" | head -1)
    [ "$why" = "Authentication failed" ] && why=401
    echo "refused ${why:-rc$rc} moved=$([ "$after" = "$before" ] && echo no || echo yes)"
  fi
}
row() { printf '%s\t%s\t%s\n' "$1" "$2" "$3" >> "$out/journey4.tsv"; }

printf 'round\tstep\tresult\n' > "$out/journey4.tsv"
printf 'round\tattempt\tat\tstatus\tconnect\ttls\tfirst_byte\ttotal\tremote_ip\tcurl_exit\n' > "$out/revokes.tsv"
pat_ok=0; pat_refused=0; ghs_ok=0; ghs_refused=0; rev204=0; jti204=0; jti_retried=0; member_after=none
for r in $(seq 1 "$rounds"); do
  g pull -q --rebase origin main 2>/dev/null
  new_pat; a=$(push_one "pat-$r"); row "$r" "pat push" "$a"; [ "$a" = ok ] && pat_ok=$((pat_ok+1))
  if [ "$r" = "$remove_at" ]; then
    member_pat=$(cat "$tok")
    awk '$1=="admin"{print $2}' "$state/m3-tokens.txt" > "$tok"
    s=$(api DELETE /api/v3/orgs/m3org/memberships/m3-member | cut -f1); row "$r" "remove member" "$s"
    printf '%s' "$member_pat" > "$tok"
    member_after=$(push_one "member-after-$r"); row "$r" "member push after removal" "$member_after"
    awk '$1=="admin"{print $2}' "$state/m3-tokens.txt" > "$tok"
    s=$(api PUT /api/v3/orgs/m3org/memberships/m3-member '{"role":"member"}' | cut -f1); row "$r" "readd member" "$s"
    printf '%s' "$member_pat" > "$tok"
  fi
  s=$(node "$here/revoke.mjs" "$tok" "$GH_CLIENT")
  row "$r" "pat revoke" "$s"; [ "$s" = 204 ] && rev204=$((rev204+1))
  b=$(push_one "pat-revoked-$r"); row "$r" "pat push after revoke" "$b"
  case $b in "refused "*"moved=no") pat_refused=$((pat_refused+1));; esac
  new_ghs; a=$(push_one "ghs-$r"); row "$r" "ghs push" "$a"; [ "$a" = ok ] && ghs_ok=$((ghs_ok+1))
  for attempt in 1 2; do
    at=$(date -u +%FT%T.%3NZ); w=$(api DELETE /api/v3/installation/token)
    printf '%s\t%s\t%s\t%s\n' "$r" "$attempt" "$at" "$w" >> "$out/revokes.tsv"
    s=${w%%$'\t'*}
    [ "$s" = 000 ] || break
    jti_retried=$((jti_retried+1))
  done
  row "$r" "ghs revoke" "$s"; [ "$s" = 204 ] && jti204=$((jti204+1))
  b=$(push_one "ghs-revoked-$r"); row "$r" "ghs push after revoke" "$b"
  case $b in "refused "*"moved=no") ghs_refused=$((ghs_refused+1));; esac
done
echo "rounds=$rounds pat_push_ok=$pat_ok pat_revoke_204=$rev204 pat_refused=$pat_refused ghs_push_ok=$ghs_ok jti_revoke_204=$jti204 jti_retried=$jti_retried ghs_refused=$ghs_refused remove_at=$remove_at member_after=[$member_after]" | tee "$out/journey4.summary"
