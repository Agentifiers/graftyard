#!/bin/bash
# M1-WP6 git client rows against the deployed edge, with whatever unmodified git is on PATH (host git, or the
# graft-git239 / graft-git251 images, which mount this folder). SHARED's oracles: `git-http` (clone, push --atomic,
# fetch --filter=blob:none) and `git-protocol-v2` (GIT_TRACE_PACKET=1 git ls-remote shows ls-refs=unborn and
# fetch=shallow wait-for-done filter). Prints one "PASS|FAIL <row>" line per check and exits non-zero on any FAIL.
#
# Usage: clients.sh <host> <owner/repo> <write token> <read-only token> <revoked token>
# The repository may already hold refs from earlier runs; every ref this run writes is under a fresh run prefix.
set -uo pipefail
HOST=$1 REPO=$2 TOK=$3 RO=$4 REV=$5
RUN=r$(date +%s)$RANDOM
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
U="https://x:$TOK@$HOST/$REPO.git"
fails=0
ok() { echo "PASS $1"; }
no() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
g() { git -c user.name=wp6 -c user.email=wp6@graftyard.invalid -c credential.helper= -c init.defaultBranch=main "$@"; }
echo "client: $(git --version), run $RUN"

# git-protocol-v2: the advertisement git sees, from its own packet trace.
trace=$(GIT_TRACE_PACKET=1 g ls-remote "$U" 2>&1 >/dev/null)
for cap in "version 2" "ls-refs=unborn" "fetch=shallow wait-for-done filter" "server-option" "object-format=sha1"; do
  grep -q "git< $cap\$" <<<"$trace" && ok "v2 advertises '$cap'" || no "v2 advertises '$cap'" "absent"
done
extra=$(grep -oE 'git< [^ ]+.*' <<<"$trace" | sed -n '/git< version 2$/,/git< 0000$/p' | sed 's/^git< //' |
  grep -vE '^(version 2|agent=.*|ls-refs=unborn|fetch=shallow wait-for-done filter|server-option|object-format=sha1|0000)$' | sort -u)
[ -z "$extra" ] && ok "v2 advertises nothing else" || no "v2 advertises nothing else" "$extra"
grep -q "git< bundle-uri\|git< packfile-uris" <<<"$trace" && no "bundle-uri and packfile-uris off" "advertised" ||
  ok "bundle-uri and packfile-uris off"

# Push: a three-commit branch and two more branches in one --atomic push.
g init -q "$W/src" && cd "$W/src" || exit 2
for i in 1 2 3; do
  head -c 20000 /dev/urandom | base64 > "f$i"
  mkdir -p "d$i" && echo "$RUN $i" > "d$i/n"
  g add . && g commit -qm "$RUN c$i"
done
g branch "$RUN/a" HEAD~1 && g branch "$RUN/b" HEAD~2
out=$(g push --atomic --porcelain "$U" "HEAD:refs/heads/$RUN/main" "$RUN/a" "$RUN/b" 2>&1)
[ "$(grep -c '^\*' <<<"$out")" = 3 ] && ok "push --atomic of 3 new branches" || no "push --atomic of 3 new branches" "$out"
remote=$(g ls-remote "$U" "refs/heads/$RUN/*")
[ "$(grep -c "" <<<"$remote")" = 3 ] && grep -q "$(g rev-parse HEAD)	refs/heads/$RUN/main" <<<"$remote" &&
  ok "ls-remote shows the pushed tips" || no "ls-remote shows the pushed tips" "$remote"

# Atomic push where one ref's expect fails: a stale --force-with-lease on b, a good update of main.
echo more >> f1 && g commit -qam "$RUN c4"
out=$(g push --atomic --porcelain "$U" "HEAD:refs/heads/$RUN/main" \
  "--force-with-lease=refs/heads/$RUN/b:$(g rev-parse HEAD~1)" "HEAD:refs/heads/$RUN/b" 2>&1)
after=$(g ls-remote "$U" "refs/heads/$RUN/main" | cut -f1)
if grep -q '^!' <<<"$out" && [ "$after" = "$(g rev-parse HEAD~1)" ]; then ok "atomic push with one stale expect: refused whole"
else no "atomic push with one stale expect: refused whole" "main at $after; $out"; fi
# The same two updates without --atomic: main moves, b is refused.
out=$(g push --porcelain "$U" "HEAD:refs/heads/$RUN/main" \
  "--force-with-lease=refs/heads/$RUN/b:$(g rev-parse HEAD~1)" "HEAD:refs/heads/$RUN/b" 2>&1)
after="$(g ls-remote "$U" "refs/heads/$RUN/main" | cut -f1) $(g ls-remote "$U" "refs/heads/$RUN/b" | cut -f1)"
if [ "$after" = "$(g rev-parse HEAD) $(g rev-parse HEAD~3)" ]; then ok "non-atomic push: main moves, stale b refused"
else no "non-atomic push: main moves, stale b refused" "remote $after; $out"; fi
# git checks a lease itself before it sends, so the server's expect check needs a race: a pre-push hook (it runs
# after git read the advertisement, before the POST) moves b from another clone. The atomic push then carries a
# stale old value for b and the edge must refuse every ref.
g clone -q "$U" "$W/racer" -b "$RUN/a" 2>/dev/null
mkdir -p .git/hooks && cat > .git/hooks/pre-push <<HOOK
#!/bin/sh
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_PREFIX
cd "$W/racer" && git -c user.name=r -c user.email=r@x commit -q --allow-empty -m racer &&
  git -c credential.helper= push -q "$U" "HEAD:refs/heads/$RUN/b" -f 2>/dev/null
HOOK
chmod +x .git/hooks/pre-push
echo six >> f3 && g commit -qam "$RUN c6"
out=$(g push --atomic --porcelain "$U" "HEAD:refs/heads/$RUN/main" "+HEAD:refs/heads/$RUN/b" 2>&1)
rm .git/hooks/pre-push
main=$(g ls-remote "$U" "refs/heads/$RUN/main" | cut -f1)
b=$(g ls-remote "$U" "refs/heads/$RUN/b" | cut -f1)
if [ "$main" = "$(g rev-parse HEAD~1)" ] && [ "$b" = "$(g -C "$W/racer" rev-parse HEAD)" ] &&
  [ "$(grep -c '^!' <<<"$out")" = 2 ]; then ok "atomic push raced by another writer: both refs refused by the edge"
else no "atomic push raced by another writer" "main $main b $b; $out"; fi
g reset -q --hard HEAD~1
# A racing writer: a second clone's push on the moved branch is a non-fast-forward.
g clone -q "$U" "$W/race" -b "$RUN/a" && cd "$W/race" && echo race > r && g add r && g commit -qm race
g push -q "$U" "HEAD:refs/heads/$RUN/a" 2>/dev/null
cd "$W/src" && echo other > o && g add o && g commit -qm other
out=$(g push --porcelain "$U" "HEAD~0:refs/heads/$RUN/a" 2>&1)
grep -q '^!.*rejected\|^!.*non-fast-forward\|^!.*fetch first' <<<"$out" && ok "non-fast-forward refused" ||
  no "non-fast-forward refused" "$out"
# A push larger than http.postBuffer: git first posts a bare flush (`probe_rpc`), then streams the body chunked.
g checkout -q -b "$RUN/big" && head -c 300000 /dev/urandom > big && g add big && g commit -qm "$RUN big"
out=$(g -c http.postBuffer=65536 push --porcelain "$U" "HEAD:refs/heads/$RUN/big" 2>&1)
grep -q '^\*' <<<"$out" && [ "$(g ls-remote "$U" "refs/heads/$RUN/big" | cut -f1)" = "$(g rev-parse HEAD)" ] &&
  ok "push over http.postBuffer (probe, then chunked)" || no "push over http.postBuffer" "$out"
g checkout -q "$RUN/main" 2>/dev/null || g checkout -q -
# Delete a branch.
out=$(g push --porcelain "$U" ":refs/heads/$RUN/b" 2>&1)
[ -z "$(g ls-remote "$U" "refs/heads/$RUN/b")" ] && ok "delete a branch" || no "delete a branch" "$out"

# Clone, twice: the second is a clone-cache hit, both fsck clean and at the same tip.
cd "$W" || exit 2
g clone -q -b "$RUN/main" "$U" c1 2>"$W/e1" && g -C c1 fsck --strict --no-dangling -q 2>&1 | grep -q . &&
  no "clone and fsck" "$(cat "$W/e1")" || { [ -d c1 ] && ok "clone and fsck" || no "clone and fsck" "$(cat "$W/e1")"; }
[ "$(g -C c1 rev-parse HEAD)" = "$(g -C src rev-parse "HEAD~1")" ] && ok "clone at the pushed tip" ||
  no "clone at the pushed tip" "$(g -C c1 rev-parse HEAD)"
g clone -q "$U" c2 2>/dev/null && [ "$(g -C c2 for-each-ref | grep -c "refs/remotes/origin/$RUN/")" -ge 2 ] &&
  ok "full clone lists the run's branches" || no "full clone lists the run's branches" "$(g -C c2 branch -r)"

# Partial clone and fetch with --filter=blob:none: no blobs until checkout needs them.
g clone -q --filter=blob:none --no-checkout -b "$RUN/main" "$U" p1 2>"$W/e2" || no "clone --filter=blob:none" "$(cat "$W/e2")"
if [ -d p1 ]; then
  missing=$(g -C p1 rev-list --objects --missing=print HEAD | grep -c '^?')
  [ "$missing" -ge 3 ] && ok "clone --filter=blob:none leaves $missing blobs missing" ||
    no "clone --filter=blob:none" "missing=$missing"
  g -C p1 checkout -q "$RUN/main" 2>"$W/e3" && [ -s p1/f3 ] && ok "lazy blob fetch on checkout" ||
    no "lazy blob fetch on checkout" "$(cat "$W/e3")"
  cd "$W/src" && echo five >> f2 && g commit -qam "$RUN c5" && g push -q "$U" "HEAD:refs/heads/$RUN/main" 2>/dev/null
  cd "$W/p1" && g fetch -q --filter=blob:none origin 2>"$W/e4" &&
    [ "$(g rev-parse "origin/$RUN/main")" = "$(g -C "$W/src" rev-parse HEAD)" ] &&
    ok "fetch --filter=blob:none sees the newer push" || no "fetch --filter=blob:none" "$(cat "$W/e4")"
  cd "$W"
fi
# An incremental fetch (haves, so never cached) into the first clone.
g -C c1 fetch -q origin 2>"$W/e5" && [ "$(g -C c1 rev-parse "origin/$RUN/main")" = "$(g -C src rev-parse HEAD)" ] &&
  ok "incremental fetch" || no "incremental fetch" "$(cat "$W/e5")"
# Shallow clone.
g clone -q --depth 1 -b "$RUN/main" "$U" s1 2>"$W/e6" && [ "$(g -C s1 rev-list --count HEAD)" = 1 ] &&
  ok "clone --depth 1" || no "clone --depth 1" "$(cat "$W/e6")"
# Protocol v0 clone (the shard's own advertisement).
g -c protocol.version=0 clone -q -b "$RUN/main" "$U" v0 2>"$W/e7" && ok "protocol v0 clone" ||
  no "protocol v0 clone" "$(cat "$W/e7")"

# Refusals: dumb HTTP, a read-only token's push, a revoked token, no credential.
code=$(curl -s -o /dev/null -w '%{http_code}' -A "git/2.39" "https://x:$TOK@$HOST/$REPO.git/info/refs")
[ "$code" = 403 ] && ok "dumb info/refs is 403" || no "dumb info/refs is 403" "$code"
cd "$W/src" || exit 2
out=$(g push "https://x:$RO@$HOST/$REPO.git" "HEAD:refs/heads/$RUN/ro" 2>&1)
grep -q "403\|denied\|Permission" <<<"$out" && ok "read-only token cannot push" || no "read-only token cannot push" "$out"
out=$(GIT_TERMINAL_PROMPT=0 g push "https://x:$REV@$HOST/$REPO.git" "HEAD:refs/heads/$RUN/rev" 2>&1)
grep -qi "401\|Authentication failed\|could not read" <<<"$out" && ok "revoked token is refused" ||
  no "revoked token is refused" "$out"
out=$(GIT_TERMINAL_PROMPT=0 g push "https://$HOST/$REPO.git" "HEAD:refs/heads/$RUN/anon" 2>&1)
grep -qi "401\|Authentication failed\|could not read" <<<"$out" && ok "anonymous push is refused" ||
  no "anonymous push is refused" "$out"
[ -z "$(g ls-remote "$U" "refs/heads/$RUN/ro" "refs/heads/$RUN/rev" "refs/heads/$RUN/anon")" ] &&
  ok "no refused push wrote a ref" || no "no refused push wrote a ref" "refs exist"

echo "fails: $fails"
exit $((fails > 0))
