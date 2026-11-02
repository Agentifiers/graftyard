#!/usr/bin/env bash
# BF-19 sweeps on graft-m1-wp4 for one seeded run, in order. Seed first with `POST /start?run=<run>&n=1000` and wait
# for `GET /status?run=<run>` to say done. The sweeper's clock is a base (the real time, or the seed's first or last
# placement) plus a skew in minutes; nothing else is faked (tests/m1/results/m1-wp4.md, "How time was shortened").
# Usage: WP4_URL=https://graft-m1-wp4.<sub>.workers.dev WP4_TOKEN=... tests/m1/wp4/bf19.sh <run>
set -u
U=${WP4_URL:?} T=${WP4_TOKEN:?} R=${1:?run}
c() { echo "== $1 $2"; curl -s --max-time 900 -H "Authorization: Bearer $T" -X "$1" "$U$2"; echo; }
c GET "/status?run=$R"
c GET "/count?run=$R"
c POST "/sweep?run=$R&base=now&skew_m=0"       # marker repairs from the request rows
c POST "/sweep?run=$R&base=last&skew_m=1380"   # fills; nothing under 24 h is deleted
c GET "/count?run=$R"
c POST "/sweep?run=$R&base=first&skew_m=1441"  # the first minute's orphans and the uploads are past 24 h
c GET "/count?run=$R"
c POST "/sweep?run=$R&base=last&skew_m=1440"   # every orphan is past 24 h
c GET "/count?run=$R"
c POST "/sweep?run=$R&base=last&skew_m=4320"   # 72 h: nothing left to do, nothing committed deleted
c GET "/count?run=$R"
