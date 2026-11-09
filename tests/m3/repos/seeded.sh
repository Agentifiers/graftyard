#!/usr/bin/env bash
# The seeded graft-m2b roles behind the edge read gate: owner m2b, writer m2b-writer, and reader m2b-reader
# (tests/m1/integ/seed.ts; ~/.graft-m2b/tokens.txt lines 1-3) against the private m2b/<repo>, <rounds> times. Each
# round, per role: `gh api repos/m2b/<repo>` (200, with the role's permissions), `gh issue list` (rc 0), REST
# `GET /repos/m2b/<repo>/commits/main/statuses` (200; the edge serves no `commits/{ref}` itself), `git clone` (rc 0), and a `git push` of a new branch (owner and writer
# accepted, reader refused). Line 4's token, revoked by the seed, clones (401). gh reads its token from its own
# GH_CONFIG_DIR (0600 hosts.yml), git from a credential helper reading a 0600 file, curl from stdin; never argv.
#   bash tests/m3/repos/seeded.sh <repo> <rounds> <out.tsv>
set -uo pipefail
REPO=${1:?repo}; ROUNDS=${2:-10}; OUT=${3:?out}
source "$(dirname "$0")/../rows/env.sh"
HOST=${GRAFT_EDGE#https://}
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT; umask 077
cat > "$W/cred.sh" <<'EOF'
#!/bin/sh
[ "$1" = get ] || exit 0
echo username=x
echo "password=$(cat "$GRAFT_TOKFILE")"
EOF
chmod +x "$W/cred.sh"
export HOME=$W GRAFT_TOKFILE=$W/tok GIT_TERMINAL_PROMPT=0 GH_NO_UPDATE_NOTIFIER=1 GH_PROMPT_DISABLED=1 GH_HOST=$HOST
unset GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GH_DEBUG
g() { git -c credential.helper= -c credential.helper="$W/cred.sh" -c user.name=seeded -c user.email=seeded@x "$@"; }
tok() { sed -n "${1}p" "$HOME_M2B/tokens.txt" | cut -d' ' -f1; }
HOME_M2B=/home/user/.graft-m2b
roles=(owner writer reader)
printf 'round\trole\tgh_api\tperm\tgh_issue_list\trest_commit\tclone\tpush\n' > "$OUT"
for r in $(seq 1 "$ROUNDS"); do
  for i in 0 1 2; do
    role=${roles[$i]}
    t=$(tok $((i + 1))); [ -n "$t" ] || { echo "no token for $role" >&2; exit 2; }
    printf '%s' "$t" > "$GRAFT_TOKFILE"
    mkdir -p "$W/gh-$role"
    printf '%s:\n    oauth_token: %s\n    user: %s\n    git_protocol: https\n' "$HOST" "$t" "$role" > "$W/gh-$role/hosts.yml"
    export GH_CONFIG_DIR=$W/gh-$role
    api=$(gh api "repos/m2b/$REPO" --jq '[.permissions.admin,.permissions.push,.permissions.pull]|map(tostring)|join(",")' 2>"$W/err")
    arc=$?; [ $arc -eq 0 ] && st=200 || st=$(grep -oE 'HTTP [0-9]{3}' "$W/err" | head -1)
    gh issue list -R "m2b/$REPO" -L 3 >/dev/null 2>"$W/err"; il=$?
    rc=$(curl -s -m 60 -o /dev/null -w '%{http_code}' -H @- "$GRAFT_EDGE/api/v3/repos/m2b/$REPO/commits/main/statuses" <<<"authorization: token $t")
    rm -rf "$W/c"; g clone -q "$GRAFT_EDGE/m2b/$REPO.git" "$W/c" 2>"$W/err"; cl=$?
    pu=skip
    if [ $cl -eq 0 ]; then
      (cd "$W/c" && git checkout -q -b "seeded-$role-$r-$$" 2>/dev/null; echo "$r" > "s-$role.txt"; git add . && g commit -qm "seeded $role $r") >/dev/null 2>&1
      (cd "$W/c" && g push -q origin HEAD 2>"$W/err"); pu=$?
      [ "$pu" != 0 ] && pu="$pu:$(grep -oE '(40[0-9]|50[0-9])|denied|not found|Permission' "$W/err" | head -1)"
    fi
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$r" "$role" "$st" "$api" "$il" "$rc" "$cl" "$pu" | tee -a "$OUT"
  done
  t=$(tok 4); printf '%s' "$t" > "$GRAFT_TOKFILE"
  rv=$(curl -s -m 60 -o /dev/null -w '%{http_code}' -H @- "$GRAFT_EDGE/m2b/$REPO.git/info/refs?service=git-upload-pack" <<<"authorization: Basic $(printf 'x:%s' "$t" | base64 -w0)")
  rm -rf "$W/c"; g clone -q "$GRAFT_EDGE/m2b/$REPO.git" "$W/c" 2>/dev/null; rcl=$?
  printf '%s\trevoked\t-\t-\t-\tinfo/refs=%s\t%s\t-\n' "$r" "$rv" "$rcl" | tee -a "$OUT"
done
