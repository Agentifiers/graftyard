#!/usr/bin/env python3
"""Print one row of the linearizability table in tests/RESULTS.md from a lin run's report.

Usage: python3 scripts/results_lin.py tests/m1/lin/raw/<run>.report.json [--gate BM-1] [--source "m1-wp8.md, ..."]

The report is the file `tests/m1/lin/run.ts finish` writes. The row has the columns of the table in
tests/RESULTS.md, "Linearizability and chaos runs on Cloudflare": run, gate, where and when, duration, operations,
faults, checks, verdict, and source. If the run's `<run>.events.jsonl` sits beside the report, the row also says how
many transfer attempts the leader accepted and how many the target refused. The script does not decide the verdict,
because the pass rule also needs the lost-write audit of the members' logs, which the report does not hold. It prints
"fill in" there.
"""

import argparse
import json
from datetime import datetime
from pathlib import Path


def n(x):
	return f"{x:,}"


def when(iso):
	return datetime.fromisoformat(iso.replace("Z", "+00:00"))


def porcupine(by_key):
	"""Summarize one Porcupine mode over every key: Ok on all, or the keys that were not Ok."""
	bad = [k for k, v in sorted(by_key.items()) if v["result"] != "Ok"]
	if not bad:
		return f"Ok on all {len(by_key)} keys"
	results = sorted({by_key[k]["result"] for k in bad})
	return f"{'/'.join(results)} on {', '.join(bad)}"


def transfer_outcomes(events):
	"""Count the leader's answers to the transfer steps the report counts (`^transfer \\d`, as `run.ts` does)."""
	accepted = refused = 0
	for line in events.read_text().splitlines():
		e = json.loads(line) if line else {}
		if not e.get("what", "").startswith("transfer ") or not e["what"][9:10].isdigit():
			continue
		result = e.get("result", "")
		accepted += result.startswith('{"Ok')
		refused += "Target(" in result
	return f", {n(accepted)} accepted by the leader, {n(refused)} refused by the target"


def faults(r, events):
	totals = r.get("faultTotals", {})
	by = {}
	for key, v in totals.items():
		site, kind = key.split(" ", 1)
		kind = kind.split(">", 1)[0]
		by[kind] = by.get(kind, 0) + v
	c = r["chaos"]
	outcomes = transfer_outcomes(events) if events.exists() else ""
	parts = [
		f"{n(c['resets'])} resets ({n(c['leaderResets'])} of a leader)",
		f"{n(c['transfers'])} transfer attempts ({n(c['abandoned'])} planned to be abandoned{outcomes})",
		f"{n(by.get('fail_call', 0))} failed calls",
		f"{n(by.get('drop_append_reply', 0))} dropped AppendEntries replies",
		f"counters from {r['isolates']} isolates",
	]
	return ", ".join(parts)


def checks(r):
	c = r["check"]
	widen = r["clock"]["widenMs"]
	parts = [
		f"Porcupine `-seg 2000` per key, recorded clocks {porcupine(c['recorded'])}",
		f"Widened 100 ms {porcupine(c['widened100'])}",
		f"Widened {widen} ms (the measured bound, halved) {porcupine(c['widenedMeasured'])}",
	]
	ack = c.get("ackEvidence")
	if ack:
		parts.append(f"Ack check {ack['result']} on {n(ack['withEvidence'])} of {n(ack['acked'])} acknowledged writes, "
			f"{n(ack['violated'])} violations")
	else:
		parts.append("No member-side ack evidence")
	return ". ".join(parts)


def main():
	p = argparse.ArgumentParser()
	p.add_argument("report")
	p.add_argument("--gate", default="BM-1")
	p.add_argument("--source", default="")
	a = p.parse_args()
	r = json.load(open(a.report))
	start, end = when(r["start"]), when(r["end"])
	hours = (end - start).total_seconds() / 3600
	where = f"Cloudflare, `{r['chain']}`, {r['start']} to {r['end']}"
	ops = f"{n(r['operations'])}, {n(r['noReply'])} with no reply; p50 {n(r['latency']['p50'])} ms, p99 {n(r['latency']['p99'])} ms"
	source = a.source or f"`{a.report}`"
	events = Path(a.report).with_name(f"{r['run']}.events.jsonl")
	cells = [f"`{r['run']}`", a.gate, where, f"{hours:g} h", ops, faults(r, events), checks(r), "fill in", source]
	print("| " + " | ".join(cells) + " |")


if __name__ == "__main__":
	main()
