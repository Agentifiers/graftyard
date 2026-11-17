#!/bin/bash
# The orb's own network beside a B0-30 run (b030r6, operator 2026-11-17: compare the load's "http 0" fetch failures
# with the orb's DNS and TCP before calling them client-side). Every 30 s: the edge host's DNS answer and lookup ms,
# one HTTPS round trip to the edge (connect and total ms, curl's exit code), and the kernel's TCP counters (nstat
# deltas: resets, retransmits, timeouts, aborts). One JSON line a sample to tests/am1/raw/$B030_RUN-net.jsonl.
cd "$(dirname "$0")/../../.."
H=graft-m1-edge.<workers-subdomain>.workers.dev
OUT=tests/am1/raw/${B030_RUN:-b030}-net.jsonl
nstat -n >/dev/null
while [ ! -e tests/am1/raw/b030-run.done ]; do
  t0=$(date +%s%3N); ip=$(getent ahostsv4 $H | awk 'NR==1{print $1}'); dns=$(( $(date +%s%3N) - t0 ))
  c=$(curl -s -o /dev/null -m 20 -w '%{http_code} %{time_connect} %{time_total}' https://$H/ ; echo " $?")
  n=$(nstat -z 2>/dev/null | awk '/TcpEstabResets|TcpRetransSegs|TcpExtTCPTimeouts|TcpExtTCPAbortOnData|TcpExtTCPAbortOnClose|TcpExtTCPAbortOnTimeout|TcpAttemptFails|TcpInErrs|TcpOutRsts/{printf "\"%s\":%s,", $1, $2}')
  echo "{\"at\":\"$(date -u +%FT%TZ)\",\"dnsMs\":$dns,\"ip\":\"$ip\",\"curl\":\"$c\",${n%,}}" >> $OUT
  sleep 30
done
