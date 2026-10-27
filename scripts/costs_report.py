#!/usr/bin/env python3
"""Write research/costs.md from the cost scripts, so every number in it can be regenerated.

Usage: python3 scripts/costs_report.py
"""

import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.dont_write_bytecode = True  # loading history/ scripts must not write __pycache__ there
sys.path.insert(0, str(ROOT / "scripts"))
import cost_model as dsql  # noqa: E402
_sspec = importlib.util.spec_from_file_location("cost_small", ROOT / "history" / "scripts" / "cost_small.py")
small = importlib.util.module_from_spec(_sspec)
_sspec.loader.exec_module(small)

_spec = importlib.util.spec_from_file_location("cf_cost", ROOT / "scripts" / "cost.py")
cf = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cf)
_aspec = importlib.util.spec_from_file_location("cf_agents", ROOT / "scripts" / "cost_agents.py")
cfa = importlib.util.module_from_spec(_aspec)
_aspec.loader.exec_module(cfa)

c3 = small.c3
TIERS = ("small", "medium", "large")
OUT = ROOT / "research" / "costs.md"


def money(x):
	return f"${x:,.0f}"


def total(lines):
	return sum(lines.values())


def table(header, rows):
	out = ["| " + " | ".join(header) + " |", "| " + " | ".join("---" for _ in header) + " |"]
	out += ["| " + " | ".join(str(c) for c in row) + " |" for row in rows]
	return "\n".join(out)


def detail(lines):
	t = total(lines)
	rows = [(name, money(cost), f"{cost / t:.0%}") for name, cost in sorted(lines.items(), key=lambda kv: -kv[1])]
	return table(("Line item", "Per month", "Share"), rows + [("Total", money(t), "100%")])


def ladder_total(key):
	return total(small.build(next(o for k, _, o in small.LADDER if k == key)))


def by_size():
	rows = []
	for tier in TIERS:
		devs = dsql.TIERS[tier]["devs"]
		rows.append((f"{devs:,}", money(total(c3.model(tier, "regional"))), money(total(c3.model(tier, "paired"))),
			money(total(dsql.model(tier, "regional", partitions=16))), money(total(dsql.model(tier, "paired", partitions=16)))))
	return table(("Developers", "One Region", "Two Regions (Paired)", "DSQL design, one Region",
		"DSQL design, two Regions"), rows)


def workload():
	a, rows = dsql.ACTIVITY, []
	for tier in TIERS:
		t = dsql.TIERS[tier]
		rows.append((f"{t['devs']:,}", f"{t['git_gb']:,} GB", f"{t['actions_gb']:,} GB", t["node"], t["nodes"]))
	per_dev = (f"{a['pushes']} pushes, {a['ci_jobs']} CI jobs with one checkout each, {a['dev_fetches']} fetches, "
		f"{a['api_requests']:,} API requests, {a['metadata_writes']} issue and pull request writes, and {a['emails']} emails")
	return per_dev, table(("Developers", "Git data", "Actions storage", "Node type in the model", "Nodes per Region"), rows)


def ladder():
	devs = dsql.TIERS["small"]["devs"]
	rows = []
	for key, label, ov in small.LADDER:
		t = total(small.build(ov))
		rows.append((f"`{key}`", label, money(t), money(t / devs)))
	return table(("Config", "Configuration", "Per month", "Per developer"), rows)


def dsql_lessons():
	rows = []
	for tier in TIERS:
		devs = dsql.TIERS[tier]["devs"]
		fixed = total(dsql.model(tier, "paired", partitions=16))
		hourly_backups = total(dsql.model(tier, "paired", partitions=16, backup="design")) - fixed
		outbox = total(dsql.model(tier, "paired")) - fixed
		rows.append((f"{devs:,}", money(fixed), money(hourly_backups), money(outbox)))
	return table(("Developers", "DSQL design, two Regions, fixed", "Extra for hourly full backups kept 35 days",
		"Extra for 256 outbox partitions polled each second"), rows)


def cloudflare():
	rows = []
	for tier in TIERS:
		devs = dsql.TIERS[tier]["devs"]
		rows.append((f"{devs:,}", *(money(total(cf.model(tier, v))) for v in ("design", "separate", "solo")),
			money(total(c3.model(tier, "regional"))), money(total(c3.model(tier, "paired")))))
	return table(("Developers", "Cloudflare default (one account, three sites)", "Cloudflare, account-separated sites",
		"Cloudflare `Solo` (evaluation, no SLO)", "AWS design, one Region", "AWS design, Paired"), rows)


def agent_layer():
	rows = []
	for tier in TIERS:
		devs = dsql.TIERS[tier]["devs"]
		base, added = total(cf.model(tier, "design")), total(cfa.lines(tier)[0])
		rows.append((f"{devs:,}", money(base), money(added), money(base + added), f"{added / base:.0%}"))
	return table(("Developers", "Cloudflare default", "Agent layer added", "Default with agents", "Added share"), rows)


def main():
	per_dev, sizes = workload()
	scrappy = small.build(next(o for k, _, o in small.LADDER if k == "scrappy"))
	floor_keys = ("Load Balancer", "IPv4", "KMS", "CloudWatch", "Route 53")
	floor = sum(v for n, v in scrappy.items() if any(k in n for k in floor_keys))
	arc = dsql.price("arc.routing_control_cluster.hour") * dsql.HOURS
	text = f"""# Cost reference

`scripts/costs_report.py` writes this file from `scripts/cost_model.py`, `history/arena/round4/candidate-3/cost.py`, `history/scripts/cost_small.py`, `scripts/cost.py`, and `scripts/cost_agents.py`. Do not edit it by hand. To change a number, change a script or a price and run `python3 scripts/costs_report.py`.

Every figure outside the Cloudflare sections is a monthly AWS list price in US dollars for one install, with us-east-2 as the home Region, us-west-2 as the peer, and us-east-1 as the witness. Prices come from `research/prices.json`, read from the AWS Price List files on 2026-10-24. No figure includes the vendor's fee, CI runner compute, an AWS Support plan, or tax. Workload and node sizes are guesses until Milestone 0 and a load test measure them.

## Workload the model assumes

Each developer generates, per day, {per_dev}.

{sizes}

## The design by company size

The design is `history/DESIGN.md` as of round 4. The DSQL design is the round 1 to 3 design, with its two known overcharges fixed (backups kept as hourly, daily, and weekly copies, and 16 outbox partitions).

{by_size()}

## Where a 50-developer bill goes

This is the design as written, in one Region, from `PYTHONPATH=scripts python3 history/scripts/cost_small.py --detail design`. It is {money(ladder_total('design') - total(c3.model('small', 'regional')))} more than the one-Region figure above, because `history/scripts/cost_small.py` prices six KMS keys per Region and `history/arena/round4/candidate-3/cost.py` prices five.

{detail(small.build({}))}

## Cheaper setups for 50 developers

Each row changes the lines its configuration names and keeps the rest. Rows from `s-net` down use the small-install profile in `history/DESIGN.md` ("Install operations"). Run `PYTHONPATH=scripts python3 history/scripts/cost_small.py --detail <config>` for the line items of any row.

{ladder()}

## The small-install profile, line by line

This is the `scrappy` row: three t4g.medium nodes in three AZs, on-demand, with a cold recovery Region.

{detail(scrappy)}

The fixed part of this bill (load balancer, public IPv4, KMS keys, CloudWatch, and Route 53) is {money(floor)} a month. It does not shrink with fewer developers, which is why no reliable setup costs much less than the `scrappy` row.

## Lessons from the DSQL design

The first cost model found three costs that the DSQL design hid.

{dsql_lessons()}

- DSQL backups through AWS Backup are always full copies (https://docs.aws.amazon.com/aws-backup/latest/devguide/backup-aurora.html). Hourly backups kept 35 days hold 840 copies, which the table's middle column prices.
- Polling 256 outbox partitions every second costs the same at 50 developers as at 2,000, which the last column prices.
- A Route 53 Application Recovery Controller cluster costs {money(arc)} a month, more than a whole small install. The current design uses Route 53 failover records instead.

## The Cloudflare design by company size

`design/DESIGN.md` is the design built only on Cloudflare products. Its figures come from `python3 scripts/cost.py`, on the same workload, at Cloudflare list prices (the `cf.*` entries in `research/prices.json`). They exclude an Enterprise contract, which has no list price and which managed cells and pushes over 200 MB need. Per-operation constants in that script are guesses until its Milestone 0.

{cloudflare()}

At 50 developers the cheapest reliable setups are the Cloudflare default at {money(total(cf.model('small', 'design')))} (RPO zero across three continents, on the Pro plan with a 100 MB HTTPS push cap and an unpriced Spectrum add-on), AWS `scrappy` at {money(ladder_total('scrappy'))} (one Region with a cold recovery Region, so a Region loss is not RPO zero), and AWS `scrappy-paired` at {money(ladder_total('scrappy-paired'))} (RPO zero across two Regions).

## Where a 50-developer Cloudflare bill goes

This is the Cloudflare default, from `python3 scripts/cost.py --detail small design`.

{detail(cf.model("small", "design"))}

## The Cloudflare agent layer

`design/AGENT-LAYER.md` adds agents as users: 3 agent tasks per developer per day, each with 5 pushes, 60 REST calls, 2 clones, 1 draft pull request, and 3 CI jobs. Its figures come from `python3 scripts/cost_agents.py`, which reruns `scripts/cost.py` with that workload and prices the new pieces, including the extra warm git cache shards for each size's largest network. Runner and model compute are excluded, and opt-in Code Mode is priced separately in that script.

{agent_layer()}
"""
	OUT.write_text(text)
	print(f"wrote {OUT.relative_to(ROOT)}: design {money(ladder_total('design'))}, scrappy {money(ladder_total('scrappy'))}, "
		f"cloudflare {money(total(cf.model('small', 'design')))}, agents {money(total(cfa.lines('large')[0]))} at 2,000")


if __name__ == "__main__":
	main()
