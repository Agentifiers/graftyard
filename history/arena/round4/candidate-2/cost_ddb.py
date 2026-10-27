#!/usr/bin/env python3
"""Monthly AWS list cost of the home-Region DynamoDB design (arena/round4/candidate-2/DESIGN.md, "Cost").

Usage:
	python3 arena/round4/candidate-2/cost_ddb.py            # summary table
	python3 arena/round4/candidate-2/cost_ddb.py --detail large two

Reuses scripts/cost_model.py unchanged: prices, TIERS, ACTIVITY, and every non-database line item (nodes, S3,
transfer, NLB, NAT, KMS, CloudWatch, Route 53, OpenSearch, SES). It removes the DSQL lines and the DSQL interface
endpoints and adds DynamoDB lines. "single" is one Region. "two" is a home and a peer Region with full node
capacity in each, plus an MRSC witness Region that stores nothing billable (research/dynamodb.md#28).
Every per-operation unit count below is a [guess] that Milestone 0 measures.
"""

import sys
from pathlib import Path
sys.dont_write_bytecode = True  # never write into scripts/__pycache__

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts"))
import cost_model as cm  # noqa: E402

P = lambda pid, region=cm.HOME: cm.price(pid, region)

# Per operation. wru: transactional write units in the home Region (2 per KB per item, research/dynamodb.md#3).
# jkb: KB of after-images the journal intent carries (plus a 1 KB mark item). skb: KB the standby applies with
# conditional PutItem (1 unit per KB). rru: strongly consistent read units (1 per 4 KB).
OPS = dict(
	push=dict(wru=38, jkb=19, skb=19, rru=8),          # ~12 items: refs, heads, push, pack row, TX marker, events, pending
	meta_write=dict(wru=20, jkb=10, skb=10, rru=6),    # issue or comment, 3 list entries, event, pending rows
	job_txn=dict(wru=4, jkb=2, skb=2, rru=2),          # one Actions job transition (25 per job)
	fetch=dict(rru=5),                                 # token, repo, grants, 40 heads in one Query, occasional delta
	api=dict(rru=5),                                   # a REST or GraphQL read
	consume=dict(wru=1, rru=0.25, lazy_kb=0.2),        # delete one pending row; batched lazy journal entry
)
JOB_TXNS = 25
CONSUMERS_PER_EVENT = 4
EVENTS = dict(push=1.2, meta_write=1.0, job_txn=0.12)
POLL = dict(consumers=10, partitions=16, per_s=1.0, rru=0.5)  # eventually consistent empty polls
NODE_LEASE_WRU_PER_S = 0.5                                    # one lease item per node, renewed every 2 s
HOME_LEASE_RWRU_PER_S = 2                                     # 1 KB lease item, two replicas, every second
ITEM_OVERHEAD = 1.3                                           # list entries and markers over the DSQL row size


def ddb_lines(tier, topo):
	t, a = cm.TIERS[tier], cm.ACTIVITY
	devs = t["devs"]
	month = lambda per_dev_day: devs * per_dev_day * cm.DAYS
	two = topo == "two"
	n = dict(push=month(a["pushes"]), meta_write=month(a["metadata_writes"]),
		job_txn=month(a["ci_jobs"]) * JOB_TXNS, fetch=month(a["ci_jobs"] + a["dev_fetches"]), api=month(a["api_requests"]))
	events = sum(n[k] * v for k, v in EVENTS.items())
	n["consume"] = events * CONSUMERS_PER_EVENT
	wru = sum(n[k] * OPS[k].get("wru", 0) for k in n)
	rru = sum(n[k] * OPS[k].get("rru", 0) for k in n)
	nodes = t["nodes"] * (2 if two else 1)
	wru += nodes * NODE_LEASE_WRU_PER_S * cm.SECONDS
	rru += POLL["consumers"] * POLL["partitions"] * POLL["per_s"] * POLL["rru"] * cm.SECONDS
	lines = {
		"DynamoDB writes in the home Region": wru / 1e6 * P("ddb.ondemand.wru"),
		"DynamoDB reads in the home Region": rru / 1e6 * P("ddb.ondemand.rru"),
	}
	gb = (t["core_gb"] + t["aux_gb"]) * ITEM_OVERHEAD
	lines["DynamoDB storage, home"] = gb * P("ddb.storage.standard")
	lines["DynamoDB PITR, home (35 days)"] = gb * P("ddb.pitr")
	lines["DynamoDB weekly on-demand backups, 4 kept"] = gb * 4 * P("ddb.backup.ondemand")
	if two:
		# Journal: intent (jkb) plus mark (1 KB), written then deleted, in two MRSC replicas.
		sync = sum(n[k] * (OPS[k]["jkb"] + 1) for k in ("push", "meta_write", "job_txn"))
		lazy = n["consume"] * OPS["consume"]["lazy_kb"]
		rwru = (sync + lazy) * 2 * 2 + HOME_LEASE_RWRU_PER_S * cm.SECONDS
		lines["DynamoDB MRSC journal (rWRU, two replicas)"] = rwru / 1e6 * P("ddb.ondemand.rwru")
		standby = sum(n[k] * OPS[k]["skb"] for k in ("push", "meta_write", "job_txn")) + lazy
		lines["DynamoDB standby apply in the peer"] = standby / 1e6 * P("ddb.ondemand.wru", cm.PEER)
		lines["DynamoDB Streams reads (journal applier)"] = 2 * cm.SECONDS / 1e6 * P("ddb.streams.read")
		lines["DynamoDB storage, standby and journal"] = (gb * 1.02) * P("ddb.storage.standard", cm.PEER)
		lines["DynamoDB PITR, standby"] = gb * P("ddb.pitr", cm.PEER)
	return lines


def model(tier, topo):
	base = cm.model(tier, "paired" if topo == "two" else "regional")
	lines = {k: v for k, v in base.items() if "DSQL" not in k and not k.startswith("S3 requests")}
	t, a = cm.TIERS[tier], cm.ACTIVITY
	pushes, jobs = t["devs"] * a["pushes"] * cm.DAYS, t["devs"] * a["ci_jobs"] * cm.DAYS
	regions = 2 if topo == "two" else 1
	# No freshness boards: S3 carries packs, Actions objects, and one status document per Region every 10 s.
	puts = pushes * 2 * regions + jobs * 20 + regions * cm.SECONDS / 10
	gets = pushes * 10 + jobs * 20
	lines["S3 requests (packs, Actions, status)"] = (puts / 1000 * P("s3.standard.put_copy_post_list")
		+ gets / 1000 * P("s3.standard.get"))
	if topo == "two":
		# Two pack copies (home and peer), not the baseline's three; the witness bucket is written only while one is down.
		lines["S3 git packs (copies and superseded versions)"] *= 2 / 3
		pushed_gb = t["devs"] * a["pushes"] * cm.DAYS * a["push_mb"] / 1024
		churn_gb = t["git_gb"] * a["large_repo_share"] * 4.3
		lines["Inter-Region pack copies"] = (pushed_gb + churn_gb) * P("dt.inter_region_to_usw2")
	lines.update(ddb_lines(tier, topo))
	return lines


def baseline(tier, topo):
	"""DESIGN.md after the rubric's two fixes: GFS backups (cost_model default) and 16 outbox partitions."""
	return sum(cm.model(tier, "paired" if topo == "two" else "regional", partitions=16).values())


def summary():
	print("| Developers | Install | This design per month | Database lines | Baseline (fixed) per month |")
	print("| --- | --- | --- | --- | --- |")
	for tier in cm.TIERS:
		for topo in ("single", "two"):
			lines = model(tier, topo)
			db = sum(v for k, v in lines.items() if k.startswith("DynamoDB"))
			print(f"| {cm.TIERS[tier]['devs']:,} | {topo} | {cm.money(sum(lines.values()))} | {cm.money(db)} | {cm.money(baseline(tier, topo))} |")


def detail(tier, topo):
	lines = model(tier, topo)
	total = sum(lines.values())
	print(f"| {tier}, {topo}: line item | Per month |")
	print("| --- | --- |")
	for k, v in sorted(lines.items(), key=lambda kv: -kv[1]):
		print(f"| {k} | {cm.money(v)} |")
	print(f"| Total | {cm.money(total)} |")


if __name__ == "__main__":
	if sys.argv[1:2] == ["--detail"]:
		detail(sys.argv[2], sys.argv[3])
	else:
		summary()
