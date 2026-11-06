#!/usr/bin/env bash
# Pushes a new commit to three new branches of each repository, one per site, so no archive is cached in R2 (the cache
# key holds the commit and the branch's prefix, and every shard shares it), then asks each site's shard (`x-graft-shard`) for its codeload tarball and reads the shard's status. A 200 means the shard's container has
# `/archive` (image aa3038ea); a 503 "git cache archive: 404 404 page not found" means an image without it (5be873a0).
# Usage: shards.sh <edge https url> <owner> <tokens.txt> <repo:id ...>
set -u
E=$1; OWNER=$2; TOKENS=$3; shift 3
TOK=$(sed -n 2p "$TOKENS" | cut -d" " -f1)
B=$(cat "${BED_TOKEN_FILE:-$HOME/.graft-m2b/bed.token}")
# Tokens reach git through its environment and curl through a header file, never argv, which `ps` shows.
HDR=$(mktemp); chmod 600 "$HDR"; printf 'Authorization: token %s\nx-bed-token: %s\n' "$TOK" "$B" > "$HDR"
BEDH=$(mktemp); chmod 600 "$BEDH"; printf 'x-bed-token: %s\n' "$B" > "$BEDH"
export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=http.extraHeader GIT_CONFIG_VALUE_0="Authorization: Basic $(printf 'x:%s' "$TOK" | base64 -w0)"
W=$(mktemp -d); trap 'rm -rf $W "$HDR" "$BEDH"' EXIT
BR=sdf-$(date -u +%H%M%S)
for ri in "$@"; do
  r=${ri%:*}; id=${ri#*:}
  ( cd $W && rm -rf $r && git -c init.defaultBranch=main init -q $r && cd $r && echo "$BR" > probe.txt && git add probe.txt \
    && git -c user.name=a -c user.email=a@b commit -qm "$BR" \
    && git push -q "$E/$OWNER/$r.git" HEAD:refs/heads/$BR-0 HEAD:refs/heads/$BR-1 HEAD:refs/heads/$BR-2 ) 2>&1 | tail -1
  for s in 0 1 2; do
    out=$(mktemp)
    code=$(curl -s -m 150 -o "$out" -w "%{http_code}" -H @"$HDR" \
      -H "x-graft-shard: $s" "$E/_codeload/$OWNER/$r/legacy.tar.gz/refs/heads/$BR-$s")
    body=$(if [ "$code" = 200 ]; then echo "$(wc -c < "$out") bytes"; else head -c 100 "$out" | tr -d '\000\n'; fi)
    st=$(curl -s -m 60 -X POST -H @"$BEDH" "$E/_bed/shard_status?repo=$id&shard=$s")
    printf '%s %s/%s %s codeload=%s %s status=%s\n' "$(date -u +%H:%M:%S)" "$r" "$s" "$BR" "$code" "$body" "$st"
    rm -f "$out"
  done
done
