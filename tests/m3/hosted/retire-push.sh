#!/usr/bin/env bash
# Kid 1's retire, over HTTPS with stock git: each token file given pushes a new commit to m3org/j4's main `n` times.
# A push counts as accepted only if git exits 0 and the remote ref is the new commit; as refused only if git exits
# non-zero and the remote ref did not move. Tokens reach git through a credential helper reading the 0600 file,
# never argv. Prints one line per push and a summary per token; no token text.
#
#   tests/m3/hosted/retire-push.sh <state dir> <n> <token file>...
set -uo pipefail
state=${1:?state}; n=${2:?n}; shift 2
EDGE=${GRAFT_EDGE:-https://graft-m1-edge.<workers-subdomain>.workers.dev}
umask 077
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
cat > "$work/cred.sh" <<'EOF'
#!/bin/sh
[ "$1" = get ] || exit 0
echo username=x
echo "password=$(cat "$GRAFT_TOKFILE")"
EOF
chmod +x "$work/cred.sh"
export GIT_TERMINAL_PROMPT=0
g() { git -c credential.helper= -c credential.helper="$work/cred.sh" -c user.name=retire -c user.email=retire@x "$@"; }
remote_head() { g ls-remote "$EDGE/m3org/j4.git" refs/heads/main 2>/dev/null | cut -f1; }
awk '$1=="admin"{print $2}' "$state/m3-tokens.txt" > "$work/admin"
GRAFT_TOKFILE=$work/admin g clone -q "$EDGE/m3org/j4.git" "$work/j4" || exit 1
cd "$work/j4" || exit 1
for f in "$@"; do
  name=$(basename "$f"); ok=0; refused=0; other=0
  for i in $(seq 1 "$n"); do
    GRAFT_TOKFILE=$work/admin g pull -q --rebase origin main 2>/dev/null
    before=$(git rev-parse HEAD)
    echo "$name $i $(date +%s%N)" >> retire.txt; git add retire.txt; g commit -qm "retire $name $i"
    GRAFT_TOKFILE=$f g push -q origin HEAD:main 2>"$work/err"; rc=$?
    after=$(GRAFT_TOKFILE=$work/admin remote_head)
    why=$(grep -oE "(401|403|404|503|Authentication failed|Bad credentials|Permission denied)" "$work/err" | head -1)
    if [ $rc -eq 0 ] && [ "$after" = "$(git rev-parse HEAD)" ]; then
      ok=$((ok+1)); echo "$name $i accepted"
    elif [ $rc -ne 0 ] && [ "$after" = "$before" ]; then
      refused=$((refused+1)); echo "$name $i refused (${why:-?}), ref not moved"; git reset -q --hard "$before"
    else
      other=$((other+1)); echo "$name $i other rc=$rc"; git reset -q --hard "$before"
    fi
  done
  echo "summary $name accepted=$ok refused=$refused other=$other of $n"
done
