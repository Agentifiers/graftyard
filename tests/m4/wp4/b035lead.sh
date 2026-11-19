#!/bin/bash
# B0-35's leader watcher on graft-m4a: every 15 s, once b035.sh has named its repository, each voter's `/status`
# (role, term, leader, vote, applied, promise) into tests/m4/raw/wp4/b035-$TAG.lead. A group with no leader (F67: three
# followers and no campaign) is then on record before anything touches it. Read-only: `/status` only, never
# `/campaign`.
#   TAG=<tag> bash tests/m4/wp4/b035lead.sh
set -uo pipefail
cd "$(dirname "$0")/../../.."
source tests/am1/lf13/env-m4a.sh >/dev/null 2>&1
TAG=${TAG:?}
ST=tests/m4/raw/wp4/b035-$TAG.status
OUT=tests/m4/raw/wp4/b035-$TAG.lead
while true; do
  id=$(grep -oE " [0-9]+ start$" "$ST" 2>/dev/null | tail -1 | awk '{print $1}')
  if [ -n "$id" ]; then
    timeout 40 node --input-type=module -e "
import { statuses, scan } from './tests/am1/hosted/lib.mjs';
const chain = 'repo:$id', out = [];
// statuses reads each site's current voter from the committed config, so a voter replaced since the group was made
// (m54's site 1, #3#f1) is read, not its superseded first-generation name.
for (const v of await statuses(chain).catch((e) => [{ site: -1, error: String(e).slice(0, 80) }])) {
  out.push({ s: v.site, name: v.doName, role: v.role, term: v.term, leader: v.leader, voted: v.votedFor, applied: v.applied,
    commit: v.commit, promiseInMs: v.promiseUntil && v.now ? v.promiseUntil - v.now : v.promiseUntil, boots: v.boots,
    err: v.error });
}
console.log(scan(new Date().toISOString() + ' ' + chain + ' ' + JSON.stringify(out)));
" >> "$OUT" 2>&1
  fi
  sleep 15
done
