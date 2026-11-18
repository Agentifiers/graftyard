#!/usr/bin/env bash
# Drives gh 2.102.1, git, and git-lfs 3.8.0 against graft-m1-edge with an install test token (no GitHub credential).
# Usage: drive.sh <absolute outdir> <owner/repo> <token-line> <command group...>. PR_HEAD names the pull request's head
# branch (default f) and RUN names the release tag v-RUN and the LFS branch lfs-RUN (default edge1).
set -u
OUT=$1; REPO=$2; LINE=$3; shift 3
TOK=$(sed -n "${LINE}p" ${TOKENS:-$HOME/.graft-integ/tokens.txt} | cut -d" " -f1)
# Without a token gh sends an empty `Authorization` header and every reply is a correct 401, which reads like a server
# fault (the `final3` run in tests/m2/results/m2-edge.md).
[ -n "$TOK" ] || { echo "no token on line $LINE of ${TOKENS:-$HOME/.graft-integ/tokens.txt}" >&2; exit 2; }
W=$(mktemp -d)
export HOME=$W/home GH_CONFIG_DIR=$W/gh GH_HOST=graft-m1-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev GH_ENTERPRISE_TOKEN=$TOK
export GIT_TERMINAL_PROMPT=0 GH_PROMPT_DISABLED=1 GH_NO_UPDATE_NOTIFIER=1 GH_DEBUG=api
mkdir -p $HOME $OUT
git config --global user.name a; git config --global user.email a@b; git config --global init.defaultBranch main
git config --global credential.helper "!f() { echo username=x; echo password=$TOK; }; f"
n=0
run() {
  n=$((n + 1)); local tag; tag=$(printf '%02d' $n)
  echo "+ $*" > $OUT/$tag.cmd
  ( cd "${CWD:-$W}" && timeout 120 "$@" ) > $OUT/$tag.out 2> $OUT/$tag.err
  local rc=$?
  echo "$tag rc=$rc $*" | tee -a $OUT/summary.txt
}
# The number at the end of the URL the last command printed (`gh issue create`, `gh pr create`), else 1.
last_number() { local u; u=$(tail -1 $OUT/$(printf '%02d' $n).out 2>/dev/null); u=${u##*/}; echo "${u:-1}"; }
cd $W
for g in "$@"; do
  case $g in
    issue)
      run /usr/bin/gh issue list -R $REPO
      run /usr/bin/gh issue create -R $REPO -t "edge issue" -b "made by the M2 edge run"
      I=$(last_number)
      run /usr/bin/gh issue view $I -R $REPO
      run /usr/bin/gh issue comment $I -R $REPO -b "hello from the edge"
      run /usr/bin/gh issue view $I -R $REPO --comments
      ;;
    pr)
      run /usr/bin/gh pr list -R $REPO
      run /usr/bin/gh pr create -R $REPO -H ${PR_HEAD:-f} -B main -t "edge pr" -b "body" --draft
      P=$(last_number)
      run /usr/bin/gh pr view $P -R $REPO
      run /usr/bin/gh pr checks $P -R $REPO
      ;;
    lfs)
      run git lfs install
      run git clone -q https://$GH_HOST/$REPO.git work
      CWD=$W/work run git lfs track '*.bin'
      head -c 300000 /dev/urandom > $W/work/a.bin
      ( cd $W/work && git add .gitattributes a.bin && git commit -qm lfs )
      CWD=$W/work run git push -q origin HEAD:refs/heads/lfs-${RUN:-edge1}
      run git clone -q -b lfs-${RUN:-edge1} https://$GH_HOST/$REPO.git work2
      CWD=$W/work2 run git lfs pull
      cmp $W/work/a.bin $W/work2/a.bin && echo "lfs bytes match" | tee -a $OUT/summary.txt
      CWD=$W/work run git lfs locks
      ;;
    release)
      run /usr/bin/gh release create v-${RUN:-edge1} -R $REPO --notes x --target main
      echo hi > $W/app.tgz
      run /usr/bin/gh release upload v-${RUN:-edge1} $W/app.tgz -R $REPO
      run /usr/bin/gh release view v-${RUN:-edge1} -R $REPO
      mkdir -p $W/dl
      CWD=$W/dl run /usr/bin/gh release download v-${RUN:-edge1} -R $REPO
      CWD=$W/dl run /usr/bin/gh release download v-${RUN:-edge1} -R $REPO --archive=tar.gz
      ;;
  esac
done
echo "workdir $W" >> $OUT/summary.txt
