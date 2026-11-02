#!/bin/bash
# BF-17 (build/BUILD-PLAN.md): "Atomic push of 10,000 refs with a 1 MiB pack. A push whose updates exceed
# COMMAND_MAX. Pass: first: pack stored and push commits. Second: refused whole, no entry. Fail: partial push, or an
# entry over 1.5 MB." With unmodified git on PATH against the deployed edge and the stand-in group of bed/big.
# Also: 10,001 refs atomic is refused whole (the 10,000-ref limit), and a non-atomic push of 10,001 refs is not.
#
# Usage: bf17.sh <host> <owner/repo> <repo id> <write token>; WP6_DIR holds bed.token.
set -uo pipefail
HOST=$1 REPO=$2 ID=$3 TOK=$4
BED=$(cat "$WP6_DIR/bed.token")
RUN=r$(date +%s)
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
U="https://x:$TOK@$HOST/$REPO.git"
fails=0
ok() { echo "PASS $1"; }
no() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
g() { git -c user.name=wp6 -c user.email=wp6@graftyard.invalid -c credential.helper= "$@"; }
bed() { curl -s -X POST -H "x-bed-token: $BED" "https://$HOST/_bed/$1?repo=$ID${2:-}"; }
# The last entry carrying commands. When the bed's Group object restarts (it is evicted while a long push streams),
# its members restore from rows and the leader resumes `led_term` with two `Noop`s, in the same term; so the log's
# last index moves without any push. A push is refused "with no entry" when no `events` entry and no `request` row
# appeared.
last() { bed log | python3 -c '
import json,sys; l=[e for e in json.load(sys.stdin) if e["kind"]=="events" or e["requests"]]
print(l[-1]["index"] if l else 0)'; }
refs() { bed refs | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))'; }
echo "client: $(git --version), run $RUN"

g init -q -b main "$W/src" && cd "$W/src" || exit 2
# 1 MiB of incompressible content, so the pack is about 1 MiB.
head -c 1048576 /dev/urandom > blob && g add blob && g commit -qm "$RUN 1 MiB"
c=$(g rev-parse HEAD)
seq -f "refs/tags/$RUN/t%05g" 1 10000 | sed "s|^|create |; s|\$| $c|" | g update-ref --stdin

# 1. The atomic push of 10,000 refs with the 1 MiB pack.
before=$(last)
t0=$(date +%s%N)
out=$(g push --atomic --porcelain "$U" "refs/tags/$RUN/*:refs/tags/$RUN/*" 2>&1)
ms=$((($(date +%s%N) - t0) / 1000000))
okn=$(grep -c '^\*' <<<"$out")
pack=$(g pack-objects --stdout --revs <<<"$c" 2>/dev/null | wc -c)
log=$(bed log "&from=$((before + 1))")
read -r idx bytes nref nent < <(python3 -c '
import json,sys; l=[e for e in json.loads(sys.argv[1]) if e["kind"]=="events"]
print(l[-1]["index"], l[-1]["bytes"], l[-1]["ref_updates"], len(l)) if l else print(0, 0, 0, 0)' "$log")
echo "  atomic 10,000: ${ms} ms, pack $pack bytes, $okn refs ok, entry $idx: $bytes bytes, $nref ref updates"
[ "$okn" = 10000 ] && ok "atomic push of 10,000 refs: every ref ok" || no "atomic push of 10,000 refs" "$(tail -3 <<<"$out")"
[ "$nref" = 10000 ] && [ "$nent" = 1 ] && ok "one entry with 10,000 ref updates" || no "one entry" "$log"
[ "$bytes" -gt 0 ] && [ "$bytes" -le 1500000 ] && ok "entry $bytes bytes <= 1.5 MB (ENTRY_MAX)" || no "entry size" "$bytes"
markers=$(bed markers "&index=$idx")
echo "  markers: $markers"
grep -q '"stored":true' <<<"$markers" && ok "pack stored (not inline), with Committed markers" ||
  no "pack stored" "$markers"
rc=$(g ls-remote "$U" "refs/tags/$RUN/*" | grep -c "^$c")
[ "$rc" = 10000 ] && ok "ls-remote lists 10,000 tags at the commit" || no "ls-remote" "$rc"
g clone -q --bare "$U" "$W/clone" 2>"$W/e" && [ "$(g -C "$W/clone" tag -l "$RUN/*" | wc -l)" = 10000 ] &&
  g -C "$W/clone" fsck -q 2>&1 | grep -q . && no "clone" "fsck" ||
  { [ -d "$W/clone" ] && ok "clone has 10,000 tags and fscks clean" || no "clone" "$(cat "$W/e")"; }

# 2. 10,001 refs with --atomic: refused whole, no entry.
# Under a fresh prefix, so all 10,001 are updates (git sends only refs that differ from the advertisement).
seq -f "refs/tags/$RUN-b/t%05g" 1 10001 | sed "s|^|create |; s|\$| $c|" | g update-ref --stdin
before=$(last) nb=$(refs)
out=$(g push --atomic --porcelain "$U" "refs/tags/$RUN-b/*:refs/tags/$RUN-b/*" 2>&1)
ng=$(grep -c '^!' <<<"$out")
echo "  atomic 10,001: $ng refs refused; $(grep -m1 'remote:' <<<"$out")"
[ "$ng" = 10001 ] && [ "$(last)" = "$before" ] && [ "$(refs)" = "$nb" ] &&
  ok "atomic push of 10,001 refs: refused whole, no entry" || no "atomic 10,001" "ng $ng, last $(last), refs $(refs)"
grep -q "at most 10000 refs" <<<"$out" && ok "the refusal names the 10,000-ref limit" || no "message" "$(grep remote: <<<"$out")"

# 3. Updates over COMMAND_MAX: 12,000 non-atomic creates with 120-byte names (about 1.9 MB of updates).
pad=$(printf 'x%.0s' $(seq 1 90))
seq -f "refs/heads/$RUN/$pad/%05g" 1 12000 | sed "s|^|create |; s|\$| $c|" | g update-ref --stdin
before=$(last) nb=$(refs)
t0=$(date +%s%N)
out=$(g push --porcelain "$U" "refs/heads/$RUN/*:refs/heads/$RUN/*" 2>&1)
ms=$((($(date +%s%N) - t0) / 1000000))
ng=$(grep -c '^!' <<<"$out") okn=$(grep -c '^\*' <<<"$out")
echo "  over COMMAND_MAX: ${ms} ms, $ng refused, $okn ok; $(grep -m1 'remote:' <<<"$out")"
[ "$ng" = 12000 ] && [ "$okn" = 0 ] && ok "updates over COMMAND_MAX: ng on every ref" || no "over COMMAND_MAX" "ng $ng ok $okn"
[ "$(last)" = "$before" ] && [ "$(refs)" = "$nb" ] && ok "no entry, no ref changed" ||
  no "no entry" "last $(last) vs $before, refs $(refs) vs $nb"

# 4. The 10,000-ref limit is for atomic pushes only: 10,001 tags without --atomic commit.
before=$(last)
out=$(g push --porcelain "$U" "refs/tags/$RUN-b/*:refs/tags/$RUN-b/*" 2>&1)
okn=$(grep -c '^\*' <<<"$out")
[ "$okn" = 10001 ] && ok "non-atomic push of 10,001 refs commits" || no "non-atomic 10,001" "$okn; $(tail -2 <<<"$out")"

echo "fails: $fails"
exit $((fails > 0))
