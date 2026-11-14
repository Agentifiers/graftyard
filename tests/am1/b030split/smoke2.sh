#!/bin/bash
# b030-split smoke 2 on graft-m1: a fresh repository m3org/b030-s2 seeded to TARGET (default 400 MB, a tenth of
# B0-30's tier, so the split costs a tenth of smoke 1's), its metadata snapshot forced, then smoke2.mjs.
#   amp orb service start b030split-smoke2 --command 'bash tests/am1/b030split/smoke2.sh'
cd "$(dirname "$0")/../../.."
export BED_TOKEN_FILE=$HOME/.graft-wp6/bed.token
L=tests/am1/raw/b030split-smoke2.log
{
  date -u +%FT%TZ
  node tests/am1/b030/mkrepo.mjs b030-s2 || exit 1
  [ -s tests/am1/raw/b030-seed.json ] && grep -q "\"repo:$(jq .repo tests/am1/raw/b030-repo.json)\"" tests/am1/raw/b030-seed.json \
    || node tests/am1/b030/seed.mjs "${TARGET:-400000000}" || exit 1
  node -e '
    import("./tests/am1/hosted/lib.mjs").then(async ({ member, log }) => {
      const c = `repo:${JSON.parse(require("fs").readFileSync("tests/am1/raw/b030-repo.json")).repo}`;
      for (const s of [0, 1, 2]) log(`force ${s}: ${JSON.stringify((await member(c, s, "/bench_seed", { force: 1 })).v)}`);
    });'
  node tests/am1/b030split/smoke2.mjs
  echo "exit $?"; date -u +%FT%TZ; echo DONE
} >> "$L" 2>&1
exec sleep infinity
