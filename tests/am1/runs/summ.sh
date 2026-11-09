#!/bin/bash
# The gate quantities of one a1.mjs run's last row: bash tests/am1/runs/summ.sh <jsonl>
jq -c 'select(.row) | {row, run, byKind: (.byKind|map_values({n, perS, p50, p99, codes})), feed, leader: (.leader|{before, after, terms, commitMs, reqPerS, feedPollsOnLeader, failedCalls}), feedMembers}' "$1"
