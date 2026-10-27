#!/usr/bin/env python3
"""Monthly AWS cost of one BYOC Graftyard install for one company.

Usage:
	python3 scripts/cost_model.py                      # summary for every tier and topology
	python3 scripts/cost_model.py --detail medium paired
	python3 scripts/cost_model.py --sensitivity medium

Prices come from research/prices.json (AWS Price List files, 2026-10-24). Resource shapes come
from history/DESIGN.md ("Deployment modes", "Install operations"). Every workload number in TIERS and
ACTIVITY is a guess, and every DPU-per-operation number is derived from the DSQL billing
examples quoted in research/pricing.md. Vendor license fees, customer-run Actions runners,
and Kinesis for DSQL CDC are not included.
"""

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
HOURS = 730
SECONDS = HOURS * 3600
DAYS = 30.4
HOME, PEER, WITNESS = "us-east-2", "us-west-2", "us-east-1"

TIERS = {
	"small": dict(devs=50, git_gb=30, actions_gb=400, core_gb=5, aux_gb=10, search_gb=40, logs_gb_day=1,
		node="i8g.xlarge", nodes=3, search_nodes=2, search_type="m7g.large.search", checkout_mb=20),
	"medium": dict(devs=300, git_gb=300, actions_gb=4500, core_gb=25, aux_gb=60, search_gb=200, logs_gb_day=6,
		node="i8g.2xlarge", nodes=3, search_nodes=2, search_type="m7g.large.search", checkout_mb=40),
	"large": dict(devs=2000, git_gb=4000, actions_gb=30000, core_gb=200, aux_gb=500, search_gb=1500, logs_gb_day=40,
		node="i8g.4xlarge", nodes=6, search_nodes=3, search_type="r7g.large.search", checkout_mb=80),
}

ACTIVITY = dict(
	pushes=8,            # per developer per day
	ci_jobs=30,          # per developer per day, each with one shallow checkout
	dev_fetches=15,      # per developer per day
	dev_fetch_mb=3,
	api_requests=2500,   # per developer per day: web, IDE plugins, gh, bots, Renovate
	api_response_kb=10,
	metadata_writes=20,  # per developer per day: comments, reviews, issue and PR edits
	inline_push_kb=40,   # average inline pack written into DSQL per push
	push_mb=1,           # average pushed bytes
	large_repo_share=0.5,  # share of git bytes in networks above 1 GiB, which repack in full weekly
	cross_az_share=0.2,    # share of served bytes that cross an AZ inside the install
	emails=10,           # per developer per day
)

DPU = dict(
	fetch_read=0.012,        # one 40-row ref_shard range read plus compute (docs: 100-row scan 0.026)
	api_read=0.03,           # several point and range reads per request
	write_base=0.3,          # push commit rows: refs, shards, ref_log, push, outbox
	write_rest=0.03,         # read and compute share of a write transaction
	event_write=0.1,         # outbox event plus one pending row per subscribed consumer
	consume=0.062,           # read and delete one pending row (docs: single insert 0.062)
	consumers_per_event=4,
	job_txns=25,             # job state, lease, check relay, logs index per Actions job
	job_txn_write=0.06,
	job_txn_rest=0.01,
	meta_write=0.08,
	poll_read=0.0065,        # an empty-partition poll (docs: point lookup 0.00675)
	partitions=256,          # outbox partitions per cell (history/DESIGN.md "Event outbox")
	consumers=10,            # outbox consumers that poll each partition (history/DESIGN.md `Consumer`)
	lease_renew_s=2,         # ConsumerLease renewal interval; the design gives none, other leases use 2 s
	write_dpu_per_byte=0.00004883,
)

TOPOLOGIES = {
	"evaluation": dict(regions=[HOME], node_regions=[HOME], dsql_storage_regions=1, multi_region=False,
		pack_copies=1, nats_per_region=1, nodes_override=1, node_override="i8g.xlarge", search=False, dr=False),
	"regional": dict(regions=[HOME], node_regions=[HOME], dsql_storage_regions=1, multi_region=False,
		pack_copies=1, nats_per_region=3, search=True, dr=False),
	"regional_dr": dict(regions=[HOME, PEER], node_regions=[HOME], dsql_storage_regions=2, multi_region=False,
		pack_copies=2, nats_per_region=3, search=True, dr=True),
	"paired": dict(regions=[HOME, PEER, WITNESS], node_regions=[HOME, PEER], dsql_storage_regions=2, multi_region=True,
		pack_copies=3, nats_per_region=3, search=True, dr=False),
}


def load_prices():
	table = {}
	for p in json.loads((ROOT / "research" / "prices.json").read_text()):
		table[(p["id"], p.get("region"))] = p["usd"]
	return table


PRICES = load_prices()


def price(pid, region=HOME):
	for key in ((pid, region), (pid, "global"), (pid, None)):
		if key in PRICES:
			return PRICES[key]
	raise KeyError(f"no price for {pid} in {region}")


def internet_egress(gb):
	tiers = [(10_240, "first10tb"), (40_960, "next40tb"), (102_400, "next100tb"), (float("inf"), "over150tb")]
	cost, left = 0.0, gb
	for size, name in tiers:
		take = min(left, size)
		cost += take * price(f"dt.internet_out.{name}")
		left -= take
		if left <= 0:
			break
	return cost


def model(tier_name, topo_name, ci="inregion", poll_s=1.0, savings_plan=False, search=None, node=None, batched_polls=False,
		lease_renewal="batched", backup="gfs", partitions=None):
	t, topo, a = TIERS[tier_name], TOPOLOGIES[topo_name], ACTIVITY
	devs = t["devs"]
	per_month = lambda per_dev_day: devs * per_dev_day * DAYS
	pushes, jobs = per_month(a["pushes"]), per_month(a["ci_jobs"])
	fetches = per_month(a["dev_fetches"])
	api = per_month(a["api_requests"])
	lines = {}

	node = node or topo.get("node_override", t["node"])
	nodes_per_region = topo.get("nodes_override", t["nodes"])
	n_nodes = nodes_per_region * len(topo["node_regions"])
	rate = price(f"ec2.{node}.csp_1yr_noupfront" if savings_plan else f"ec2.{node}.ondemand")
	lines["EC2 git, api, edge, worker nodes"] = n_nodes * rate * HOURS
	lines["EBS root volumes"] = n_nodes * 50 * price("ebs.gp3.storage")
	if topo["dr"]:
		lines["EC2 dr-apply nodes in the recovery Region"] = 2 * price("ec2.m8g.xlarge.ondemand", PEER) * HOURS

	writes = (pushes * (DPU["write_base"] + a["inline_push_kb"] * 1024 * DPU["write_dpu_per_byte"])
		+ (pushes * 3 + jobs * 3) * DPU["event_write"]
		+ (pushes * 3 + jobs * 3) * DPU["consumers_per_event"] * (DPU["consume"] - 0.01)
		+ jobs * DPU["job_txns"] * DPU["job_txn_write"]
		+ per_month(a["metadata_writes"]) * DPU["meta_write"])
	reads = ((jobs + fetches + pushes) * DPU["fetch_read"] + api * DPU["api_read"]
		+ pushes * DPU["write_rest"] + jobs * DPU["job_txns"] * DPU["job_txn_rest"]
		+ (pushes * 3 + jobs * 3) * DPU["consumers_per_event"] * 0.01)
	parts = partitions or DPU["partitions"]
	if batched_polls:
		polling = DPU["consumers"] / poll_s * 0.02 * SECONDS
	else:
		polling = DPU["consumers"] * parts / poll_s * DPU["poll_read"] * SECONDS
	polling += 2 * 10 * 0.0075 * SECONDS + n_nodes * 0.5 * DPU["poll_read"] * SECONDS
	if lease_renewal == "batched":
		renew_writes = DPU["consumers"] * parts * 128 * DPU["write_dpu_per_byte"] / DPU["lease_renew_s"] * SECONDS
		renew_rest = n_nodes * (0.00375 + 0.01) / DPU["lease_renew_s"] * SECONDS
	else:
		renew_writes = DPU["consumers"] * parts * 0.05 / DPU["lease_renew_s"] * SECONDS
		renew_rest = DPU["consumers"] * parts * 0.01175 / DPU["lease_renew_s"] * SECONDS
	writes += renew_writes
	multi = writes if topo["multi_region"] else 0.0
	cdc = writes * 0.0477 if topo["dr"] else 0.0
	dpu = price("dsql.dpu") / 1e6
	lines["DSQL requests (git, API, Actions, events)"] = (writes - renew_writes + reads) * dpu
	lines["DSQL background polling"] = polling * dpu
	lines["DSQL outbox lease renewals"] = (renew_writes + renew_rest) * dpu
	if multi:
		lines["DSQL multi-Region write replication"] = multi * dpu
	if cdc:
		lines["DSQL CDC to the recovery Region (Kinesis not priced)"] = cdc * dpu
	db_gb = t["core_gb"] + t["aux_gb"]
	lines["DSQL storage (core and aux)"] = db_gb * topo["dsql_storage_regions"] * price("dsql.storage")
	if backup == "design":
		home_copies, peer_copies, copies_per_month = 24 * 35, 0, 0
	elif topo_name == "evaluation":
		home_copies, peer_copies, copies_per_month = 7, 0, 0
	else:
		home_copies, peer_copies, copies_per_month = 24 + 7 + 4, 7 + 4, 30
	lines["DSQL backups (full copies in AWS Backup)"] = db_gb * (
		(home_copies + peer_copies) * price("backup.dsql.warm") + copies_per_month * price("backup.dsql.copy_to_usw2"))

	churn = 1 + a["large_repo_share"] * (7 + 35) / 7
	git_stored = t["git_gb"] * churn * topo["pack_copies"]
	lines["S3 git packs (copies and superseded versions)"] = git_stored * price("s3.standard.storage")
	lines["S3 Actions cache, artifacts, logs"] = t["actions_gb"] * price("s3.standard.storage")
	members = n_nodes
	board_regions = len(topo["node_regions"])
	puts = members * SECONDS + pushes * 2 + jobs * 20 + len(topo["regions"]) * SECONDS / 10
	gets = members * board_regions * SECONDS + pushes * 10 + jobs * 20
	lines["S3 requests (freshness boards, status, packs, Actions)"] = (
		puts / 1000 * price("s3.standard.put_copy_post_list") + gets / 1000 * price("s3.standard.get"))

	ci_gb = jobs * t["checkout_mb"] / 1024
	dev_gb = (fetches * a["dev_fetch_mb"] + api * a["api_response_kb"] / 1024) / 1024
	if ci == "internet":
		lines["Data transfer out to the internet (developers and CI)"] = internet_egress(dev_gb + ci_gb)
	else:
		lines["Data transfer out to the internet (developers)"] = internet_egress(dev_gb)
		lines["Same-Region transfer to CI runners (install side)"] = ci_gb * price("dt.inter_az")
	lines["Inter-AZ transfer inside the install"] = (dev_gb + ci_gb) * a["cross_az_share"] * 2 * price("dt.inter_az")
	churn_gb = t["git_gb"] * a["large_repo_share"] * 4.3
	pushed_gb = pushes * a["push_mb"] / 1024
	if topo_name == "paired":
		lines["Inter-Region pack copies"] = (pushed_gb + churn_gb) * (price("dt.inter_region_to_usw2") + price("dt.inter_region_to_use1"))
	if topo["dr"]:
		lines["Inter-Region pack replication to the recovery bucket"] = (pushed_gb + churn_gb) * (
			price("dt.inter_region_to_usw2") + price("s3.replication.rtc"))

	n_regions = len(topo["node_regions"])
	lines["Network Load Balancer"] = n_regions * price("nlb.hour") * HOURS + (dev_gb + ci_gb) * price("nlb.nlcu")
	nats = topo["nats_per_region"] * (n_regions + (1 if topo["dr"] else 0))
	remote_pack_gb = (pushed_gb + churn_gb) * 2 if topo_name == "paired" else 0.0
	lines["NAT gateways and public IPv4"] = (nats * price("vpc.natgw.hour") * HOURS
		+ (50 + remote_pack_gb) * price("vpc.natgw.processed")
		+ (nats + 3 * n_regions) * price("vpc.public_ipv4.hour") * HOURS)
	lines["DSQL interface endpoints"] = n_regions * 3 * price("vpc.interface_endpoint.az_hour") * HOURS + 100 * price(
		"vpc.interface_endpoint.processed")

	keys = 5 + 2 * (len(topo["regions"]) - 1)
	signs = n_regions * SECONDS / 10
	lines["KMS keys and requests"] = keys * price("kms.key.month") + signs / 10_000 * price("kms.requests.asymmetric")
	metrics = n_nodes * 40 + 60
	logs = t["logs_gb_day"] * DAYS
	lines["CloudWatch metrics, alarms, logs"] = (metrics * price("cloudwatch.custom_metric.first10k")
		+ 60 * price("cloudwatch.alarm.standard") + logs * price("cloudwatch.logs.ingest_standard")
		+ logs * price("cloudwatch.logs.storage"))
	lines["Route 53 zone, health checks, queries"] = (price("route53.hosted_zone") + n_regions * (
		price("route53.health_check.aws_basic") + 2 * price("route53.health_check.aws_option"))
		+ (api + jobs + fetches) * 0.1 / 1e6 * price("route53.queries.standard"))
	if topo["dr"]:
		lines["Route 53 ARC routing-control cluster"] = price("arc.routing_control_cluster.hour") * HOURS
	use_search = topo["search"] if search is None else search
	if use_search:
		lines["OpenSearch (code and issue search)"] = (t["search_nodes"] * price(f"opensearch.{t['search_type']}") * HOURS
			+ t["search_gb"] * 2 * price("opensearch.gp3.storage"))
	lines["SES email"] = per_month(a["emails"]) / 1000 * price("ses.outbound")
	return lines


def money(x):
	return f"${x:,.0f}"


def summary():
	print("| Tier | Developers | Topology | AWS per month | Per developer | 1-yr Savings Plan per month | GitHub Enterprise list price |")
	print("| --- | --- | --- | --- | --- | --- | --- |")
	for tier in TIERS:
		devs = TIERS[tier]["devs"]
		for topo in ("evaluation", "regional", "regional_dr", "paired"):
			if topo == "evaluation" and tier != "small":
				continue
			total = sum(model(tier, topo).values())
			sp = sum(model(tier, topo, savings_plan=True).values())
			gh = devs * price("github.enterprise_cloud.user_month")
			print(f"| {tier} | {devs} | {topo} | {money(total)} | {money(total / devs)} | {money(sp)} | {money(gh)} |")


def detail(tier, topo):
	lines = model(tier, topo)
	total = sum(lines.values())
	print(f"| {tier}, {topo}: line item | Per month | Share |")
	print("| --- | --- | --- |")
	for name, cost in sorted(lines.items(), key=lambda kv: -kv[1]):
		print(f"| {name} | {money(cost)} | {cost / total:.0%} |")
	print(f"| Total | {money(total)} | 100% |")


def sensitivity(tier):
	base = sum(model(tier, "paired").values())
	cases = [
		("Paired, CI in Region, 1 s outbox polls, batched lease renewal, 35 retained backups (base)", {}),
		("Backups as history/DESIGN.md states them: hourly full backups kept 35 days (840 copies)", dict(backup="design")),
		("One transaction per outbox lease renewal", dict(lease_renewal="per_lease")),
		("16 outbox partitions per install instead of 256", dict(partitions=16)),
		("CI runners outside AWS (clones leave as internet egress)", dict(ci="internet")),
		("Outbox polled every 100 ms", dict(poll_s=0.1)),
		("Outbox polled every 10 s with in-memory wake-up hints", dict(poll_s=10.0)),
		("Outbox polled every 1 s, one query per consumer across its partitions", dict(batched_polls=True)),
		("One node size smaller", dict(node={"small": "i8g.xlarge", "medium": "i8g.xlarge", "large": "i8g.2xlarge"}[tier])),
		("1-year Compute Savings Plan on nodes", dict(savings_plan=True)),
		("No OpenSearch", dict(search=False)),
	]
	print(f"| {tier}: change from the base | Per month | Change |")
	print("| --- | --- | --- |")
	for name, kw in cases:
		total = sum(model(tier, "paired", **kw).values())
		print(f"| {name} | {money(total)} | {money(total - base) if kw else '-'} |")
	for topo in ("regional", "regional_dr"):
		total = sum(model(tier, topo).values())
		print(f"| Topology {topo} instead of paired | {money(total)} | {money(total - base)} |")


if __name__ == "__main__":
	args = sys.argv[1:]
	if args[:1] == ["--detail"]:
		detail(args[1], args[2])
	elif args[:1] == ["--sensitivity"]:
		sensitivity(args[1])
	else:
		summary()
