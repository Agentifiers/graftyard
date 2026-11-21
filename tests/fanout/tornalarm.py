#!/usr/bin/env python3
"""tornalarm.py REPO [REPO ...] -> each site's git shard `/status` for a network torn down at a row's end
(`teardown-stops-runs`'s flip: a torn-down shard's `alarm` reads null and its container stays stopped), read through the
test edge's `/_bed/shard_status?repo=&shard=` for shards 0, 1 and 2. The bed token is read from BED_TOKEN_FILE and sent on
curl's stdin, never on argv. Run with the install's env sourced."""
import json, os, subprocess, sys

edge, bed = os.environ["GRAFT_EDGE"], open(os.environ["BED_TOKEN_FILE"]).read().strip()
bad = 0
for repo in sys.argv[1:]:
    for shard in (0, 1, 2):
        r = subprocess.run(["curl", "-s", "-m", "60", "-H", "@-", f"{edge}/_bed/shard_status?repo={repo}&shard={shard}"],
                           input=f"x-bed-token: {bed}\n", capture_output=True, text=True)
        try:
            d = json.loads(r.stdout)
            line = {"running": d.get("running"), "alarm": d.get("alarm"), "cursor": d.get("cursor")}
            bad += bool(d.get("running") or d.get("alarm"))
        except ValueError:
            line = {"error": (r.stdout or f"curl {r.returncode}")[:120]}
        print(f"repo {repo} shard {shard}: {json.dumps(line)}", flush=True)
print("TORNALARM OK" if not bad else f"TORNALARM: {bad} shards running or with an alarm")
