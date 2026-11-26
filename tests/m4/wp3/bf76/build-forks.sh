#!/usr/bin/env bash
# BF-76: the `graftyard-actions` forks of upload-artifact@v4 and download-artifact@v4, built reproducibly from the
# upstream tags (design/SHARED.md, "Toolkit hostname heuristics and SHA pins"). Each fork is the upstream tag's tree
# with one change in every bundled `isGhes()` (`dist/**/index.js`): it also answers false when the job sets
# GRAFTYARD_ACTIONS=true. `isGhes()` is in the bundled `@actions/artifact`, not in the action's `src/`, so `dist/` is
# the only place it exists in the repository. The commit is fixed (author, committer, dates, message) and its parent
# is the upstream tag's commit, so the same inputs give the same fork SHA on any machine.
#
#   bash tests/m4/wp3/bf76/build-forks.sh <out dir> [<mirror dir>]
#
# <mirror dir> holds upload-artifact.git and download-artifact.git (bare clones of github.com/actions/*); without it
# they are cloned. Prints one line per fork: repository, tag, upstream SHA, fork SHA. Each fork is left at
# <out dir>/<repo>.git with the branch `main` and the tags `v4` and the upstream tag on the fork commit.
set -euo pipefail
mkdir -p "$1"; out=$(cd "$1" && pwd); mir=${2:+$(cd "$2" && pwd)}
export GIT_AUTHOR_NAME="graftyard-actions" GIT_AUTHOR_EMAIL="actions@graftyard.invalid"
export GIT_COMMITTER_NAME="$GIT_AUTHOR_NAME" GIT_COMMITTER_EMAIL="$GIT_AUTHOR_EMAIL"
export GIT_AUTHOR_DATE="2026-11-25T00:00:00Z" GIT_COMMITTER_DATE="2026-11-25T00:00:00Z"
for spec in upload-artifact:v4.6.2:ea165f8d65b6e75b540449e92b4886f43607fa02 \
            download-artifact:v4.3.0:d3f86a106a0bac45b974a628896c90dbdf5c8093; do
  IFS=: read -r repo tag want <<<"$spec"
  src="$out/$repo.src.git"
  rm -rf "$src" "$out/$repo.git" "$out/$repo.wt"
  if [ -n "$mir" ]; then git clone -q --bare "$mir/$repo.git" "$src"
  else git clone -q --bare "https://github.com/actions/$repo.git" "$src"; fi
  up=$(git -C "$src" rev-parse "$tag^{commit}")
  [ "$up" = "$want" ] || { echo "$repo $tag is $up, not $want" >&2; exit 1; }
  git -C "$src" worktree add -q --detach "$out/$repo.wt" "$up"
  n=0
  while IFS= read -r f; do
    python3 - "$out/$repo.wt/$f" <<'PY'
import sys
p = sys.argv[1]
s = open(p, encoding="utf-8").read()
old = "    return !isGitHubHost && !isGheHost && !isLocalHost;\n"
new = ("    if (process.env['GRAFTYARD_ACTIONS'] === 'true') {\n        return false;\n    }\n" + old)
assert s.count(old) == 1, p
open(p, "w", encoding="utf-8").write(s.replace(old, new))
PY
    n=$((n + 1))
  done < <(git -C "$out/$repo.wt" grep -l "return !isGitHubHost && !isGheHost && !isLocalHost;" -- 'dist/*.js')
  [ "$n" -ge 1 ] || { echo "$repo: no isGhes() in dist" >&2; exit 1; }
  git -C "$out/$repo.wt" commit -q -a -m "graftyard-actions: isGhes() also trusts GRAFTYARD_ACTIONS=true ($tag)"
  fork=$(git -C "$out/$repo.wt" rev-parse HEAD)
  git init -q --bare "$out/$repo.git"
  git -C "$out/$repo.wt" push -q "$out/$repo.git" "HEAD:refs/heads/main" "HEAD:refs/tags/v4" "HEAD:refs/tags/$tag"
  git -C "$src" worktree remove --force "$out/$repo.wt"
  echo "$repo $tag upstream $up fork $fork ($n bundle(s))"
done
