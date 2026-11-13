#!/bin/bash
# The two-owner smoke against graft-m3c as an orb service: tests/am1/pool/smoke.mjs into
# tests/am1/raw/pool/smoke-<tag>.log, then every shard of its networks and every pool shard torn down.
cd "$(dirname "$0")/../../.."
source tests/am1/lf13/env-m3c.sh
L=tests/am1/raw/pool/smoke-$1.log
grep -q '^exit' "$L" 2>/dev/null && exec sleep infinity
ln -sf $HOME/.graft-m3c/tools.token tests/m0/.m0-token
node tests/am1/pool/smoke.mjs >> "$L" 2>&1
echo "exit $? $(date -u +%FT%TZ)" >> "$L"
python3 tests/fanout/teardown.py --all-nets --pools >> "$L" 2>&1
rm -f tests/m0/.m0-token
exec sleep infinity
