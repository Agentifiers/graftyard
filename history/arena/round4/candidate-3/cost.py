#!/usr/bin/env python3
"""Monthly AWS list cost of one candidate-3 Tributary install (chains in DynamoDB, bytes in S3, folds on NVMe).

Usage:
	python3 arena/round4/candidate-3/cost.py            # summary table
	python3 arena/round4/candidate-3/cost.py --detail medium paired

Reuses TIERS, ACTIVITY, prices, and every non-database line of scripts/cost_model.py unchanged, so the
comparison with the baseline differs only where the foundation differs. The baseline column fixes its two known
overcharges: cost_model.py's default backup retention, and 16 outbox partitions instead of 256.
Every request-unit count per operation below is a [guess] from the DESIGN.md "Shape" section.
"""

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts"))
import cost_model as base  # noqa: E402

price, HOURS, SECONDS, DAYS = base.price, base.HOURS, base.SECONDS, base.DAYS
HOME, PEER, WITNESS = base.HOME, base.PEER, base.WITNESS

# Request units per operation [guess], from DESIGN.md "Shape" and "Commit and read paths".
U = dict(
	entry_wru=1.0,          # one chain entry item under 1 KB, before inline pack bytes
	inline_share=0.5,       # share of pushes whose pack is at most 16 KiB and rides inside the entry
	inline_kb=6.0,          # average inline pack size of those pushes
	consumers=4,            # dirty-marker subscribers per entry (webhooks, actions trigger, notify, search)
	consumer_wru=3.0,       # per consumer per entry: set marker, delete marker, advance cursor
	consumer_rru=1.0,       # per consumer per entry: one strong tail read
	fresh_rru=2.0,          # owner chain plus repository chain strong tail reads per request
	token_miss=0.02,        # share of requests whose token or route misses the node cache
	job_wru=21.0,           # job item transitions, queue index, check run updates, job events
	job_rru=5.0,            # broker queue reads and check reads per job
	entry_kb_stored=2.0,    # average stored entry, kept 30 days in DynamoDB, then archived to S3
	job_kb_stored=12.0,     # job, run, check, and log-index items, kept 90 days
	stream_shards=16,       # stream shards across the four cell tables in a small install [guess]
	stream_polls_s=2.0,     # dispatcher GetRecords per shard per second
)

TOPOLOGIES = {
	"regional": dict(regions=[HOME], node_regions=[HOME], replicas=1, pack_copies=1, snapshot_copies=1),
	"paired": dict(regions=[HOME, PEER, WITNESS], node_regions=[HOME, PEER], replicas=2, pack_copies=3, snapshot_copies=2),
}


def model(tier_name, topo_name):
	t, topo, a = base.TIERS[tier_name], TOPOLOGIES[topo_name], base.ACTIVITY
	per_month = lambda per_dev_day: t["devs"] * per_dev_day * DAYS
	pushes, jobs, fetches = per_month(a["pushes"]), per_month(a["ci_jobs"]), per_month(a["dev_fetches"])
	api, meta = per_month(a["api_requests"]), per_month(a["metadata_writes"])
	lines = {}

	n_nodes = t["nodes"] * len(topo["node_regions"])
	lines["EC2 nodes (same fleet as the baseline)"] = n_nodes * price(f"ec2.{t['node']}.ondemand") * HOURS
	lines["EBS root volumes"] = n_nodes * 50 * price("ebs.gp3.storage")

	consumer_w = U["consumers"] * U["consumer_wru"]
	push_w = U["entry_wru"] + U["inline_share"] * U["inline_kb"] + consumer_w
	writes = pushes * push_w + meta * (U["entry_wru"] + consumer_w) + jobs * U["job_wru"]
	reads_strong = ((pushes + fetches + jobs + api) * U["fresh_rru"] + (pushes + meta) * U["consumers"] * U["consumer_rru"]
		+ jobs * U["job_rru"])
	reads_eventual = (api + fetches + jobs) * U["token_miss"]
	# rWRU is billed in every Region holding a replica (research/dynamodb.md#28); single-Region tables bill WRU once.
	wru_price = price("ddb.ondemand.rwru") if topo["replicas"] > 1 else price("ddb.ondemand.wru")
	lines["DynamoDB writes (chains, consumers, Actions)"] = writes * topo["replicas"] * wru_price / 1e6
	lines["DynamoDB reads (strong tail reads, cache misses)"] = (reads_strong + reads_eventual * 0.5) * price("ddb.ondemand.rru") / 1e6
	stored_gb = (pushes + meta) * U["entry_kb_stored"] / 1e6 + jobs * 3 * U["job_kb_stored"] / 1e6
	lines["DynamoDB storage and PITR"] = stored_gb * topo["replicas"] * (price("ddb.storage.standard") + price("ddb.pitr"))
	stream_reads = U["stream_shards"] * U["stream_polls_s"] * SECONDS * topo["replicas"]
	lines["DynamoDB Streams reads (dispatcher)"] = max(0.0, stream_reads - 2.5e6) * price("ddb.streams.read") / 1e6

	churn = 1 + a["large_repo_share"] * (7 + 35) / 7
	lines["S3 git packs (copies and noncurrent versions)"] = t["git_gb"] * churn * topo["pack_copies"] * price("s3.standard.storage")
	snapshot_gb = t["core_gb"] * 2 * topo["snapshot_copies"] + (pushes + meta) * 12 * U["entry_kb_stored"] / 1e6
	lines["S3 fold snapshots and chain archive"] = snapshot_gb * price("s3.standard.storage")
	lines["S3 Actions cache, artifacts, logs"] = t["actions_gb"] * price("s3.standard.storage")
	puts = (pushes * (1 - U["inline_share"]) * topo["pack_copies"] + jobs * 20 + len(topo["regions"]) * SECONDS / 10
		+ (pushes + meta) / 100 * topo["snapshot_copies"])
	gets = pushes * 10 + jobs * 20
	lines["S3 requests (packs, snapshots, status, Actions)"] = (puts / 1000 * price("s3.standard.put_copy_post_list")
		+ gets / 1000 * price("s3.standard.get"))

	ci_gb = jobs * t["checkout_mb"] / 1024
	dev_gb = (fetches * a["dev_fetch_mb"] + api * a["api_response_kb"] / 1024) / 1024
	lines["Data transfer out to the internet (developers)"] = base.internet_egress(dev_gb)
	lines["Same-Region transfer to CI runners (install side)"] = ci_gb * price("dt.inter_az")
	lines["Inter-AZ transfer inside the install"] = (dev_gb + ci_gb) * a["cross_az_share"] * 2 * price("dt.inter_az")
	churn_gb = t["git_gb"] * a["large_repo_share"] * 4.3
	pushed_gb = pushes * a["push_mb"] / 1024
	if topo_name == "paired":
		lines["Inter-Region pack and snapshot copies"] = (pushed_gb + churn_gb) * (
			price("dt.inter_region_to_usw2") + price("dt.inter_region_to_use1"))

	n_regions = len(topo["node_regions"])
	lines["Network Load Balancer"] = n_regions * price("nlb.hour") * HOURS + (dev_gb + ci_gb) * price("nlb.nlcu")
	nats = 3 * n_regions
	remote_pack_gb = (pushed_gb + churn_gb) * 2 if topo_name == "paired" else 0.0
	lines["NAT gateways and public IPv4"] = (nats * price("vpc.natgw.hour") * HOURS
		+ (50 + remote_pack_gb) * price("vpc.natgw.processed") + (nats + 3 * n_regions) * price("vpc.public_ipv4.hour") * HOURS)
	# DynamoDB and S3 use free gateway endpoints; one interface endpoint per Region for DynamoDB Streams [unverified need].
	lines["Interface endpoint for DynamoDB Streams"] = n_regions * 3 * price("vpc.interface_endpoint.az_hour") * HOURS

	keys = 5 + 2 * (len(topo["regions"]) - 1)
	lines["KMS keys and requests"] = keys * price("kms.key.month") + n_regions * SECONDS / 10 / 10_000 * price(
		"kms.requests.asymmetric")
	logs = t["logs_gb_day"] * DAYS
	lines["CloudWatch metrics, alarms, logs"] = ((n_nodes * 40 + 60) * price("cloudwatch.custom_metric.first10k")
		+ 60 * price("cloudwatch.alarm.standard") + logs * (price("cloudwatch.logs.ingest_standard") + price("cloudwatch.logs.storage")))
	lines["Route 53 zone, health checks, queries"] = (price("route53.hosted_zone") + n_regions * (
		price("route53.health_check.aws_basic") + 2 * price("route53.health_check.aws_option"))
		+ (api + jobs + fetches) * 0.1 / 1e6 * price("route53.queries.standard"))
	lines["OpenSearch (code and issue search)"] = (t["search_nodes"] * price(f"opensearch.{t['search_type']}") * HOURS
		+ t["search_gb"] * 2 * price("opensearch.gp3.storage"))
	lines["SES email"] = per_month(a["emails"]) / 1000 * price("ses.outbound")
	return lines


def money(x):
	return f"${x:,.0f}"


def summary():
	print("| Developers | Topology | This design per month | Database lines | Baseline, fixed | Difference |")
	print("| --- | --- | --- | --- | --- | --- |")
	for tier in base.TIERS:
		for topo in ("regional", "paired"):
			lines = model(tier, topo)
			total = sum(lines.values())
			db = sum(v for k, v in lines.items() if k.startswith("DynamoDB"))
			baseline = sum(base.model(tier, topo, partitions=16).values())
			print(f"| {base.TIERS[tier]['devs']:,} | {topo} | {money(total)} | {money(db)} | {money(baseline)} | {money(total - baseline)} |")


def detail(tier, topo):
	lines = model(tier, topo)
	total = sum(lines.values())
	print(f"| {tier}, {topo}: line item | Per month |")
	print("| --- | --- |")
	for name, cost in sorted(lines.items(), key=lambda kv: -kv[1]):
		print(f"| {name} | {money(cost)} |")
	print(f"| Total | {money(total)} |")


if __name__ == "__main__":
	args = sys.argv[1:]
	if args[:1] == ["--detail"]:
		detail(args[1], args[2])
	else:
		summary()
