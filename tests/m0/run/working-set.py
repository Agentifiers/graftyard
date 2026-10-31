"""M0-WP6, B0-14: daily working set of git cache shards, from public GitHub activity as a stand-in for an install.

For each org, the working set of a day is the bytes of the org's repositories that had a git event that day (push,
branch or tag create or delete, pull request), over the bytes of all its repositories. Sizes are GitHub's `size` (KB
on disk) for every non-fork repository today. Events are GH Archive hours. Public data has no fetch events, so a repo
fetched but not pushed that day is missed [inference: CI fetches follow pushes and pull requests, so the miss is
developers' fetches of quiet repositories].

	python3 run/working-set.py sizes [ORG ...]        gh api, into .gen/gha/size-ORG.tsv
	python3 run/working-set.py events DAY [DAY ...]    GH Archive, into .gen/gha/DAY-H.tsv
	python3 run/working-set.py report DAY [DAY ...]    prints the table and writes results/raw/b0-14.json
"""

import collections
import gzip
import json
import statistics
import subprocess
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
GEN = ROOT / ".gen" / "gha"
ORGS = ("apache microsoft google kubernetes mozilla aws hashicorp grafana elastic cloudflare vercel openai huggingface "
	"nodejs rust-lang golang dotnet azure googlecloudplatform").split()
GIT_EVENTS = {"PushEvent", "CreateEvent", "DeleteEvent", "PullRequestEvent"}
TIERS = {"small": 30, "medium": 300, "large": 4000}   # git_gb per tier, scripts/cost_model.py


def sizes(orgs):
	for o in orgs:
		out = subprocess.run(["gh", "api", "--paginate", f"orgs/{o}/repos?per_page=100&type=all", "--jq",
			".[] | [.full_name, .size, .fork, .archived, .pushed_at] | @tsv"], capture_output=True, text=True, check=True).stdout
		(GEN / f"size-{o}.tsv").write_text(out)


def events(days):
	want = set(ORGS)
	for d in days:
		for h in range(24):
			path = GEN / f"{d}-{h}.tsv"
			if path.exists():
				continue
			rows = []
			with urllib.request.urlopen(f"https://data.gharchive.org/{d}-{h}.json.gz") as r, gzip.open(r, "rt", errors="replace") as f:
				for line in f:
					i = line.find('"repo":{"id":')
					j = line.find('"name":"', i) + 8
					if i < 0 or line[j:line.find('"', j)].split("/")[0].lower() not in want:
						continue
					e = json.loads(line)
					rows.append(f'{e["repo"]["name"]}\t{e["type"]}\t{e["created_at"]}\n')
			path.write_text("".join(rows))


def report(days):
	size, total, count = {}, collections.Counter(), collections.Counter()
	for o in ORGS:
		for line in (GEN / f"size-{o}.tsv").read_text().splitlines():
			name, kb, fork, *_ = line.split("\t")
			if fork == "true":
				continue
			size[name.lower()] = (o, int(kb))
			total[o] += int(kb)
			count[o] += 1
	per_day = collections.defaultdict(list)
	events_seen = 0
	for d in days:
		active = collections.defaultdict(set)
		for h in range(24):
			for line in (GEN / f"{d}-{h}.tsv").read_text().splitlines():
				name, kind, _ = line.split("\t")
				events_seen += 1
				if kind in GIT_EVENTS and name.lower() in size:
					active[size[name.lower()][0]].add(name.lower())
		for o in ORGS:
			per_day[o].append((len(active[o]), sum(size[r][1] for r in active[o]) / total[o]))
	orgs = sorted(({"org": o, "gb": total[o] / 1e6, "repos": count[o], "active": statistics.mean(a for a, _ in per_day[o]),
		"share": statistics.mean(s for _, s in per_day[o])} for o in ORGS if total[o]), key=lambda r: r["gb"])
	# Each tier is compared with the orgs within a factor of 3 of its git bytes.
	tiers = {}
	for t, gb in TIERS.items():
		band = [r["share"] for r in orgs if gb / 3 <= r["gb"] <= gb * 3]
		tiers[t] = {"git_gb": gb, "orgs": len(band), "median": statistics.median(band) if band else None,
			"max": max(band) if band else None}
	out = {"days": days, "events": events_seen, "orgs": orgs, "tiers": tiers,
		"bytes_weighted": sum(r["gb"] * r["share"] for r in orgs) / sum(r["gb"] for r in orgs)}
	(ROOT / "results" / "raw" / "b0-14.json").write_text(json.dumps(out, indent=1))
	for r in orgs:
		print(f'{r["org"]:22s} {r["gb"]:7.1f} GB {r["repos"]:5d} repos {r["active"]:6.1f} active/day  {100 * r["share"]:5.1f}%')
	for t, v in tiers.items():
		print(t, v)
	print("bytes-weighted", round(100 * out["bytes_weighted"], 1), "events", events_seen)


if __name__ == "__main__":
	GEN.mkdir(parents=True, exist_ok=True)
	cmd, args = sys.argv[1], sys.argv[2:]
	{"sizes": lambda: sizes(args or ORGS), "events": lambda: events(args), "report": lambda: report(args)}[cmd]()
