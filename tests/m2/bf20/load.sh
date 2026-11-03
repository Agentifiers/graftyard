#!/bin/bash
# load.sh NAME RATE SECS [EDITS PER SEEDED]  -> raw/NAME.{load,stats0,stats1,stats2}.json
N=$1; R=$2; S=$3; E=${4:-0}; P=${5:-1}; SD=${6:-0}
H=$(sed -n 2p ~/.graft-bf20/tokens.txt | cut -d' ' -f2)
# A probe push wakes the group; its trace names the member that answered as leader.
PROBE=$(cat /tmp/bf20/probe.${REPO:-196627} 2>/dev/null || echo null); Q=$(python3 -c "import os; print(os.urandom(20).hex())")
L=${LEADER:-$(REF=probe.$$ /tmp/bf20/push.sh null $Q | python3 -c "import json,sys; v=json.load(sys.stdin); print(v['trace'][-2]['name'][-1])")}
echo $Q > /tmp/bf20/probe.${REPO:-196627}
echo "leader $L"
for s in 0 1 2; do /tmp/bf20/m.sh $s '/stats?reset=1' > /dev/null; done
python3 -c "import json,sys; a=sys.argv; print(json.dumps({'load':{'rate':int(a[1]),'secs':int(a[2]),'tag':a[3],'account':7000002,'token':a[4],'edits':int(a[5]),'per':int(a[6]),'seeded':int(a[7])}}))" $R $S $N $H $E $P $SD > /tmp/bf20/raw/$N.body.json
TO=$((S*3+120)) DL=$(((S*3+110)*1000)) /tmp/bf20/m.sh $L /bench_seed "$(cat /tmp/bf20/raw/$N.body.json)" > /tmp/bf20/raw/$N.load.json
sleep 3
for s in 0 1 2; do /tmp/bf20/m.sh $s /stats > /tmp/bf20/raw/$N.stats$s.json; done
python3 /tmp/bf20/sum.py $N
