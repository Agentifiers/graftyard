#!/bin/bash
# The plants as an orb service: output in tests/fanout/raw/plants.log.
cd "$(dirname "$0")/../.."
[ -e tests/fanout/raw/.plants.started ] && exec sleep infinity
touch tests/fanout/raw/.plants.started
python3 tests/fanout/plant.py > tests/fanout/raw/plants.log 2>&1
echo DONE >> tests/fanout/raw/plants.log
exec sleep infinity
