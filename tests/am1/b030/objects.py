#!/usr/bin/env python3
"""objects.py REPO FROM_ISO TO_ISO -> the Durable Object ids of repository REPO's group on $INSTALL (default graft-m1),
one a line: every Member and GitShard object whose Workers Logs name the group (`repo:REPO`, a shard's `repo=REPO`,
an archive key's `REPO/`), with the child group's (`repo-meta:REPO`) once it exists. usage.py counts only these
when B030_OBJECTS names this file (B0-30 rerun, 2026-11-14). The token goes in a header only."""
import json, os, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
repo, a, b = sys.argv[1], sys.argv[2], sys.argv[3]
ms = lambda iso: int(__import__("datetime").datetime.fromisoformat(iso.replace("Z", "+00:00")).timestamp() * 1000)
P = os.environ.get("INSTALL", "graft-m1")


def ids(needle):
    body = {"queryId": "q", "timeframe": {"from": ms(a), "to": ms(b)}, "view": "calculations", "limit": 500,
            "parameters": {"calculations": [{"operator": "count"}],
                           "groupBys": [{"type": "string", "value": "$workers.durableObjectId"},
                                        {"type": "string", "value": "$workers.entrypoint"}],
                           "filters": [{"key": "$metadata.message", "operation": "includes", "type": "string", "value": needle},
                                       {"key": "$workers.scriptName", "operation": "includes", "type": "string", "value": f"{P}-site-"}]}}
    r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
        data=json.dumps(body).encode(), headers={"authorization": f"Bearer {tok}", "content-type": "application/json"}, method="POST")
    agg = json.load(urllib.request.urlopen(r, timeout=60))["result"]["calculations"][0]["aggregates"]
    return {x["groups"][0]["value"] for x in agg if x["groups"][1]["value"] in ("Member", "GitShard")}


out = set()
for n in (f"repo:{repo}", f"repo={repo}", f"{repo}/", f"repo-meta:{repo}"):
    out |= ids(n)
print("\n".join(sorted(out)))
