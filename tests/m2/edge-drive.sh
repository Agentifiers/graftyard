#!/usr/bin/env bash
# Drives gh 2.102.1, git, and git-lfs 3.8.0 against graft-m1-edge with an install test token (no GitHub credential).
# Usage: drive.sh <outdir> <owner/repo> <token-line> <command group...>
set -u
OUT=$1; REPO=$2; LINE=$3; shift 3
TOK=$(sed -n "${LINE}p" ${TOKENS:-$HOME/.graft-integ/tokens.txt} | cut -d" " -f1)
W=$(mktemp -d)
export HOME=$W/home GH_CONFIG_DIR=$W/gh GH_HOST=graft-m1-edge.<workers-subdomain>.workers.dev GH_ENTERPRISE_TOKEN=$TOK
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
cd $W
for g in "$@"; do
  case $g in
    issue)
      run /usr/bin/gh issue list -R $REPO
      run /usr/bin/gh issue create -R $REPO -t "edge issue" -b "made by the M2 edge run"
      run /usr/bin/gh issue view 1 -R $REPO
      run /usr/bin/gh issue comment 1 -R $REPO -b "hello from the edge"
      run /usr/bin/gh issue view 1 -R $REPO --comments
      ;;
    pr)
      run /usr/bin/gh pr list -R $REPO
      run /usr/bin/gh pr create -R $REPO -H f -B main -t "edge pr" -b "body" --draft
      run /usr/bin/gh pr view 1 -R $REPO
      run /usr/bin/gh pr checks 1 -R $REPO
      ;;
    lfs)
      run git lfs install
      run git clone -q https://$GH_HOST/$REPO.git work
      CWD=$W/work run git lfs track '*.bin'
      head -c 300000 /dev/urandom > $W/work/a.bin
      ( cd $W/work && git add .gitattributes a.bin && git commit -qm lfs )
      CWD=$W/work run git push -q origin HEAD:refs/heads/lfs-edge
      run git clone -q -b lfs-edge https://$GH_HOST/$REPO.git work2
      CWD=$W/work2 run git lfs pull
      cmp $W/work/a.bin $W/work2/a.bin && echo "lfs bytes match" | tee -a $OUT/summary.txt
      CWD=$W/work run git lfs locks
      ;;
    release)
      run /usr/bin/gh release create v-edge1 -R $REPO --notes x --target main
      echo hi > $W/app.tgz
      run /usr/bin/gh release upload v-edge1 $W/app.tgz -R $REPO
      run /usr/bin/gh release view v-edge1 -R $REPO
      mkdir -p $W/dl
      CWD=$W/dl run /usr/bin/gh release download v-edge1 -R $REPO
      CWD=$W/dl run /usr/bin/gh release download v-edge1 -R $REPO --archive=tar.gz
      ;;
  esac
done
echo "workdir $W" >> $OUT/summary.txt
