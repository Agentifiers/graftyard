#!/usr/bin/env bash
# `gh auth login --web` against the graft-m1 edge, gh's device flow (`design/SHARED.md`, `oauth-device-flow`): gh
# posts `/login/device/code` and prints the user code; this script reads it from gh's output, approves it as
# m3-member with their PAT through the test-only `POST /login/oauth/device/approve` (the browser page is not built),
# and gh's next poll of `/login/oauth/access_token` gets the token. Then `gh api user` and `gh auth status`. gh runs
# with its own GH_CONFIG_DIR and `--insecure-storage`, so its token stays in that directory (0700); GH_DEBUG=api's
# log is kept for gh-replies.py with the token redacted.
#
#   tests/m3/hosted/gh-login.sh <state dir> <out dir> [runs]
set -uo pipefail
state=${1:?state}; out=${2:?out}; runs=${3:-1}
host=${GRAFT_HOST:-graft-m1-edge.<workers-subdomain>.workers.dev}
mkdir -p "$out/gh-login"; umask 077
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
awk '$1=="member"{print $2}' "$state/m3-tokens.txt" > "$work/member.tok"
export GH_NO_UPDATE_NOTIFIER=1 GH_PROMPT_DISABLED=1 BROWSER=true
ok=0
for n in $(seq 1 "$runs"); do
  export GH_CONFIG_DIR=$work/gh-$n; mkdir -p "$GH_CONFIG_DIR"
  : > "$work/login.out"
  GH_DEBUG=api gh auth login --hostname "$host" --web --git-protocol https --skip-ssh-key --insecure-storage \
    > "$work/login.out" 2> "$work/login.err" < /dev/null &
  pid=$!
  code=
  for _ in $(seq 1 60); do
    code=$(grep -hoE '[A-Z0-9]{4}-[A-Z0-9]{4}' "$work/login.out" "$work/login.err" 2>/dev/null | head -1)
    [ -n "$code" ] && break; sleep 0.5
  done
  if [ -n "$code" ]; then
    status=$(curl -s -m 60 -o "$work/approve" -w '%{http_code}' -X POST -H @- -d "user_code=$code" \
      "https://$host/login/oauth/device/approve" <<<"authorization: token $(cat "$work/member.tok")")
  else
    status=none
  fi
  wait $pid; rc=$?
  who=$(gh api user --hostname "$host" --jq .login 2>"$work/api.err")
  st=$(gh auth status --hostname "$host" 2>&1 | grep -oE "Logged in to [^ ]+ account [^ ]+" | head -1)
  echo "run $n: code=${code:+yes} approve=$status login_rc=$rc api_user=$who status=[$st]" | tee -a "$out/gh-login/runs.txt"
  [ "$rc" = 0 ] && [ "$who" = m3-member ] && ok=$((ok+1))
  # gh's debug log, with every gh token and device code redacted, for gh-replies.py.
  sed -E 's/(gh[opsur]_)[A-Za-z0-9_.-]+/\1<redacted>/g; s/("device_code":")[^"]+/\1<redacted>/g; s/(device_code=)[^&" ]+/\1<redacted>/g' \
    "$work/login.err" > "$out/gh-login/login-$n.err"
  sed -E 's/(gh[opsur]_)[A-Za-z0-9_.-]+/\1<redacted>/g' "$work/api.err" > "$out/gh-login/api-$n.err"
done
echo "gh device-flow logins: $ok of $runs" | tee -a "$out/gh-login/runs.txt"
