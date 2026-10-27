#!/usr/bin/env python3
"""Cheapest reliable monthly AWS bill for a 50-developer install of the round-4 design.

Usage:
	python3 scripts/cost_small.py                    # the ladder of configurations
	python3 scripts/cost_small.py --detail lean-dr   # line items for one configuration

Starts from arena/round4/candidate-3/cost.py's line items for the "small" tier and changes only the
lines each lever touches. Workload and prices come from scripts/cost_model.py and research/prices.json.
Node sizing for 50 developers is a [guess] that Milestone 0 and a load test must confirm.
"""

import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import cost_model as base  # noqa: E402

_spec = importlib.util.spec_from_file_location("c3cost", ROOT / "arena" / "round4" / "candidate-3" / "cost.py")
c3 = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(c3)

price, HOURS, SECONDS, DAYS = base.price, base.HOURS, base.SECONDS, base.DAYS
TIER = "small"

DESIGN = dict(topo="regional", node="i8g.xlarge", rate="ondemand", root_gb=50, cache_gb=0, nats=3,
	search="design", metrics_per_node=40, service_metrics=60, alarms=60, dr=None, standby=0,
	per_region=None, azs=3, streams_endpoint=True, stream_poll_factor=1.0, logs_to_s3=False, nlb=True)
# Compute Savings Plan rates scale with instance size inside a family [inference]; t4g.medium's 3-year rate is
# t4g.large's verified 3-year rate halved. Every other rate is verified in research/prices.json.
INFERRED = {"ec2.t4g.medium.csp_3yr_noupfront": 0.0334 / 2}
LEAN_NODES = dict(node="m8g.large", root_gb=20, cache_gb=100)
ONE_NAT = dict(nats=1)
SMALL_SEARCH = dict(search=("m7g.medium.search", 1, 20))
LEAN_METRICS = dict(metrics_per_node=15, service_metrics=30, alarms=40)
LEAN = {**LEAN_NODES, **ONE_NAT, **SMALL_SEARCH, **LEAN_METRICS}
SCRAPPY_NET = dict(nats=0, streams_endpoint=False)
NO_SEARCH = dict(search=None)
LOGS_S3 = dict(logs_to_s3=True, metrics_per_node=5, service_metrics=10, alarms=12)
T4G = dict(node="t4g.medium", root_gb=20, cache_gb=60)
SCRAPPY = {**LEAN, **SCRAPPY_NET, **NO_SEARCH, **LOGS_S3, **T4G, "dr": "cold", "stream_poll_factor": 0.5}

LADDER = [
	("design", "Design as written: 3 i8g.xlarge, 3 NAT gateways, 2-node search, full metrics", {}),
	("nodes", "Right-size nodes: 3 m8g.large with 100 GB gp3 cache each", LEAN_NODES),
	("nat", "Plus one NAT gateway instead of three", {**LEAN_NODES, **ONE_NAT}),
	("search", "Plus search on one m7g.medium.search node", {**LEAN_NODES, **ONE_NAT, **SMALL_SEARCH}),
	("lean", "Plus lean CloudWatch metrics (all SLO alarms kept)", LEAN),
	("lean-1yr", "Lean, 1-year Compute Savings Plan, no upfront", {**LEAN, "rate": "csp_1yr_noupfront"}),
	("lean-3yr", "Lean, 3-year Compute Savings Plan, no upfront", {**LEAN, "rate": "csp_3yr_noupfront"}),
	("lean-t4g", "Lean on burstable t4g.large, 3-year plan", {**LEAN, "node": "t4g.large", "rate": "csp_3yr_noupfront"}),
	("lean-dr", "Lean, 3-year plan, plus a cold recovery Region", {**LEAN, "rate": "csp_3yr_noupfront", "dr": "cold"}),
	("lean-dr-warm", "Lean, 3-year plan, plus a recovery Region with one warm node", {**LEAN, "rate": "csp_3yr_noupfront",
		"dr": "cold", "standby": 1}),
	("lean-paired", "Lean, 3-year plan, two Regions plus a witness (Paired)", {**LEAN, "rate": "csp_3yr_noupfront",
		"topo": "paired"}),
	("paired-design", "Paired as written", {"topo": "paired"}),
	("s-start", "Scrappy start: lean-dr above", {**LEAN, "rate": "csp_3yr_noupfront", "dr": "cold"}),
	("s-net", "Plus public subnets, no NAT, no Streams interface endpoint", {**LEAN, "rate": "csp_3yr_noupfront",
		"dr": "cold", **SCRAPPY_NET}),
	("s-search", "Plus no OpenSearch: issue search in SQLite FTS5, code search by git grep", {**LEAN,
		"rate": "csp_3yr_noupfront", "dr": "cold", **SCRAPPY_NET, **NO_SEARCH}),
	("s-obs", "Plus logs to S3, 12 alarms on the SLO metrics", {**LEAN, "rate": "csp_3yr_noupfront", "dr": "cold",
		**SCRAPPY_NET, **NO_SEARCH, **LOGS_S3}),
	("s-t4g", "Plus 3 x t4g.medium (burstable, unlimited), on-demand", {**SCRAPPY, "rate": "ondemand",
		"stream_poll_factor": 1.0}),
	("scrappy", "Plus dispatcher polls each shard once a second", {**SCRAPPY, "rate": "ondemand"}),
	("scrappy-3yr", "Scrappy on a 3-year plan [inferred rate]", {**SCRAPPY, "rate": "csp_3yr_noupfront"}),
	("scrappy-dns", "Scrappy without the NLB: Route 53 health-checked records to node IPs, on-demand", {**SCRAPPY,
		"rate": "ondemand", "nlb": False}),
	("scrappy-paired", "Scrappy, two Regions plus a witness, on-demand", {**SCRAPPY, "rate": "ondemand",
		"topo": "paired", "dr": None}),
	("floor", "Floor: one t4g.medium in one AZ, cold recovery Region, no uptime target", {**SCRAPPY,
		"rate": "ondemand", "per_region": 1, "azs": 1, "nlb": False}),
]


def build(overrides):
	cfg = {**DESIGN, **overrides}
	topo = cfg["topo"]
	regions = 2 if topo == "paired" else 1
	lines = dict(c3.model(TIER, topo))
	t, a = base.TIERS[TIER], base.ACTIVITY

	per_region = cfg["per_region"] or t["nodes"]
	n_nodes = per_region * regions
	rate_id = f"ec2.{cfg['node']}.{cfg['rate']}"
	hourly = INFERRED[rate_id] if rate_id in INFERRED else price(rate_id)
	del lines["EC2 nodes (same fleet as the baseline)"]
	lines[f"EC2 nodes ({n_nodes} x {cfg['node']}, {cfg['rate']})"] = n_nodes * hourly * HOURS
	del lines["EBS root volumes"]
	lines["EBS gp3 root and cache volumes"] = n_nodes * (cfg["root_gb"] + cfg["cache_gb"]) * price("ebs.gp3.storage")

	nat_hour, ipv4 = price("vpc.natgw.hour") * HOURS, price("vpc.public_ipv4.hour") * HOURS
	c3_nats = 3 * regions
	nat_line = lines.pop("NAT gateways and public IPv4")
	processed = nat_line - c3_nats * nat_hour - (c3_nats + 3 * regions) * ipv4
	nlb_ips = cfg["azs"] * regions if cfg["nlb"] else 0
	if not cfg["nlb"]:
		del lines["Network Load Balancer"]
		lines["Route 53 health checks per node"] = n_nodes * price("route53.health_check.aws_basic")
	if cfg["nats"]:
		nats = cfg["nats"] * regions
		lines[f"NAT gateways ({nats}) and public IPv4"] = nats * nat_hour + processed + (nats + nlb_ips) * ipv4
	else:
		lines[f"Public IPv4 ({n_nodes} nodes, {nlb_ips} NLB)"] = (n_nodes + nlb_ips) * ipv4
	if not cfg["streams_endpoint"]:
		del lines["Interface endpoint for DynamoDB Streams"]
	elif cfg["azs"] != 3:
		lines["Interface endpoint for DynamoDB Streams"] *= cfg["azs"] / 3
	stream_reads = c3.U["stream_shards"] * c3.U["stream_polls_s"] * cfg["stream_poll_factor"] * SECONDS * regions
	lines["DynamoDB Streams reads (dispatcher)"] = max(0.0, stream_reads - 2.5e6) * price("ddb.streams.read") / 1e6

	if cfg["search"] is None:
		del lines["OpenSearch (code and issue search)"]
	elif cfg["search"] != "design":
		del lines["OpenSearch (code and issue search)"]
		kind, count, gb = cfg["search"]
		lines[f"OpenSearch ({count} x {kind})"] = count * price(f"opensearch.{kind}") * HOURS + gb * price("opensearch.gp3.storage")

	logs = t["logs_gb_day"] * DAYS
	log_cost = logs * 2 * price("s3.standard.storage") if cfg["logs_to_s3"] else logs * (
		price("cloudwatch.logs.ingest_standard") + price("cloudwatch.logs.storage"))
	lines["CloudWatch metrics, alarms, logs" + (" (logs in S3)" if cfg["logs_to_s3"] else "")] = (
		(n_nodes * cfg["metrics_per_node"] + cfg["service_metrics"]) * price("cloudwatch.custom_metric.first10k")
		+ cfg["alarms"] * price("cloudwatch.alarm.standard") + log_cost)
	if cfg["logs_to_s3"]:
		del lines["CloudWatch metrics, alarms, logs"]

	key_regions = 3 if topo == "paired" else 1 + (1 if cfg["dr"] else 0)
	lines["KMS keys and requests"] = (6 * key_regions * price("kms.key.month")
		+ regions * SECONDS / 10 / 10_000 * price("kms.requests.asymmetric"))

	if cfg["dr"] == "cold":
		dr = base.PEER
		lines["dr: DynamoDB replica writes"] = lines["DynamoDB writes (chains, consumers, Actions)"]
		lines["dr: DynamoDB replica storage and PITR"] = lines["DynamoDB storage and PITR"]
		lines["dr: S3 replicated packs and snapshots"] = (lines["S3 git packs (copies and noncurrent versions)"]
			+ lines["S3 fold snapshots and chain archive"])
		churn_gb = t["git_gb"] * a["large_repo_share"] * 4.3
		pushed_gb = base.TIERS[TIER]["devs"] * a["pushes"] * DAYS * a["push_mb"] / 1024
		lines["dr: S3 replication transfer"] = (pushed_gb + churn_gb) * price("dt.inter_region_to_usw2")
		lines["dr: no-promotion fence reads"] = n_nodes * 0.1 * SECONDS / 1000 * price("s3.standard.get")
		lines["dr: failover health check and alarm"] = price("route53.health_check.aws_basic") + price("cloudwatch.alarm.standard")
		if cfg["standby"]:
			lines[f"dr: warm standby ({cfg['standby']} x {cfg['node']}, on-demand)"] = cfg["standby"] * price(
				f"ec2.{cfg['node']}.ondemand", dr) * HOURS + cfg["standby"] * (cfg["root_gb"] + cfg["cache_gb"]) * price(
				"ebs.gp3.storage")
	return lines


def money(x):
	return f"${x:,.0f}"


def ladder():
	print("| Config | Configuration | Per month | Per developer |")
	print("| --- | --- | --- | --- |")
	devs = base.TIERS[TIER]["devs"]
	for key, label, ov in LADDER:
		total = sum(build(ov).values())
		print(f"| {key} | {label} | {money(total)} | {money(total / devs)} |")


def detail(key):
	ov = next(o for k, _, o in LADDER if k == key)
	lines = build(ov)
	total = sum(lines.values())
	print(f"| {key}: line item | Per month | Share |")
	print("| --- | --- | --- |")
	for name, cost in sorted(lines.items(), key=lambda kv: -kv[1]):
		print(f"| {name} | {money(cost)} | {cost / total:.0%} |")
	print(f"| Total | {money(total)} | 100% |")


if __name__ == "__main__":
	args = sys.argv[1:]
	if args[:1] == ["--detail"]:
		detail(args[1])
	else:
		ladder()
