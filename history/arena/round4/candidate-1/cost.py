#!/usr/bin/env python3
"""Monthly AWS list cost of one Tributary install on the DynamoDB repository-log foundation.

Usage: python3 arena/round4/candidate-1/cost.py [--detail TIER TOPOLOGY]

Workload (TIERS, ACTIVITY) and every non-database line come from scripts/cost_model.py, imported unchanged,
so the comparison with the baseline differs only where the stores differ. Lines that this design removes:
every DSQL line and the DSQL interface endpoints (DynamoDB and S3 use free gateway endpoints). Lines it
replaces: DynamoDB requests, storage, PITR, Streams reads, and S3 requests (no freshness boards).
Topology "regional" is single-Region tables; "paired" is MRSC with replicas in us-east-2 and us-west-2 and a
witness in us-east-1, billed as rWRU in both replica Regions (research/dynamodb.md findings 28 and 29).
Every unit count per operation below is a guess that Milestone 0 measures.
"""

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts"))
import cost_model as cm  # noqa: E402

SECONDS = cm.SECONDS
GROUPS = 6           # consumer groups that get a dirty item per commit: views, webhooks, actions, pr-sync, notify, search
DIRTY_PARTS = 4      # dirty-set partitions per group in an install
RV_PARTS = 4         # revocation-log partitions in an install
LEASE_ITEMS = 20     # leases renewed every 2 s (pumps, consumer groups, applier, operator)

WRU = dict(          # write request units per operation, before replication
	push=4 + 2 + 2 * cm.ACTIVITY["inline_push_kb"] + 5 + 3 * GROUPS,  # head CAS, log item, inline pack put and later delete, views, dirty mark+delete+cursor
	job=35,          # job and run state, leases near expiry, check runs and their index entries, log chunk index
	meta=2 + 6 + 3 * 3,  # item, index deltas, dirty work for three groups
	delivery=2,      # webhook delivery record per push
)
RRU = dict(fetch=3, api=4, push=2, job=20, consume=GROUPS)


def dynamodb(tier, topo):
	t, a = cm.TIERS[tier], cm.ACTIVITY
	per_month = lambda x: t["devs"] * x * cm.DAYS
	pushes, jobs = per_month(a["pushes"]), per_month(a["ci_jobs"])
	fetches, api, meta = per_month(a["dev_fetches"]), per_month(a["api_requests"]), per_month(a["metadata_writes"])
	replicas = 2 if topo == "paired" else 1
	writes = (pushes * (WRU["push"] + WRU["delivery"]) + jobs * WRU["job"] + meta * WRU["meta"]
		+ LEASE_ITEMS / 2 * SECONDS)
	reads = ((fetches + jobs) * RRU["fetch"] + api * RRU["api"] + pushes * (RRU["push"] + RRU["consume"])
		+ jobs * RRU["job"] + GROUPS * DIRTY_PARTS * 0.5 * SECONDS + RV_PARTS * SECONDS)
	gb = 1.3 * (t["core_gb"] + t["aux_gb"])   # baseline row volume plus index entries and the retained log
	stream_calls = 16 * 4 * SECONDS            # 16 shards polled every 250 ms, one pump per shard
	return {
		"DynamoDB writes (WRU or rWRU per replica Region)": writes * replicas * cm.price("ddb.ondemand.wru") / 1e6,
		"DynamoDB reads": reads * cm.price("ddb.ondemand.rru") / 1e6,
		"DynamoDB storage": gb * replicas * cm.price("ddb.storage.standard"),
		"DynamoDB PITR (35 days, every replica)": gb * replicas * cm.price("ddb.pitr"),
		"DynamoDB Streams reads": max(0.0, stream_calls - 2.5e6) * cm.price("ddb.streams.read") / 1e6,
	}


def s3_requests(tier, topo):
	t, a = cm.TIERS[tier], cm.ACTIVITY
	per_month = lambda x: t["devs"] * x * cm.DAYS
	pushes, jobs = per_month(a["pushes"]), per_month(a["ci_jobs"])
	regions = 2 if topo == "paired" else 1
	nodes = t["nodes"] * regions
	puts = pushes * 2 + jobs * 20 + regions * SECONDS / 10 + regions * SECONDS  # packs, Actions, status, projection heads
	gets = pushes * 10 + jobs * 20 + nodes * SECONDS                           # packs, Actions, projection heads
	return {"S3 requests (packs, Actions, status, projection heads)":
		puts / 1000 * cm.price("s3.standard.put_copy_post_list") + gets / 1000 * cm.price("s3.standard.get")}


DROP = ("DSQL", "S3 requests")


def model(tier, topo):
	base = cm.model(tier, topo, partitions=16)
	lines = {k: v for k, v in base.items() if not k.startswith(DROP)}
	lines.update(dynamodb(tier, topo))
	lines.update(s3_requests(tier, topo))
	return lines


def main():
	if len(sys.argv) == 4 and sys.argv[1] == "--detail":
		lines = model(sys.argv[2], sys.argv[3])
		for k, v in sorted(lines.items(), key=lambda kv: -kv[1]):
			print(f"| {k} | {cm.money(v)} |")
		print(f"| Total | {cm.money(sum(lines.values()))} |")
		return
	print("| Developers | Topology | Baseline after fixes | This design | DynamoDB lines | Difference |")
	print("| --- | --- | --- | --- | --- | --- |")
	for tier in ("small", "medium", "large"):
		for topo in ("regional", "paired"):
			base = sum(cm.model(tier, topo, partitions=16).values())
			lines = model(tier, topo)
			ddb = sum(v for k, v in lines.items() if k.startswith("DynamoDB"))
			mine = sum(lines.values())
			print(f"| {cm.TIERS[tier]['devs']:,} | {topo} | {cm.money(base)} | {cm.money(mine)} | {cm.money(ddb)} | {cm.money(mine - base)} |")


if __name__ == "__main__":
	main()
