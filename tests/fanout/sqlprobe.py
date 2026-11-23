#!/usr/bin/env python3
"""Join sqlprobe.mjs probe lines with their Durable Object Workers Logs CPU and fit SQLite costs.

Usage, from the repository root:
  python3 tests/fanout/sqlprobe.py PROBE.jsonl WORKERS_LOGS.json

PROBE.jsonl is stdout from sqlprobe.mjs. WORKERS_LOGS.json is the list written by
`tests/fanout/wlobj.py fetch` for the same member object. Arms a and b fit CPU as intercept
+ statements + rows; arm c fits CPU per transaction as intercept + KB. Pure standard library.
"""
import json
import sys
from datetime import datetime


def ms(iso):
    return int(datetime.fromisoformat(iso.replace("Z", "+00:00")).timestamp() * 1000)


def solve(rows, ys):
    """Least squares through normal equations and pivoted Gaussian elimination."""
    k = len(rows[0])
    a = [[sum(r[i] * r[j] for r in rows) for j in range(k)] + [sum(r[i] * y for r, y in zip(rows, ys))]
         for i in range(k)]
    for c in range(k):
        p = max(range(c, k), key=lambda r: abs(a[r][c]))
        a[c], a[p] = a[p], a[c]
        if abs(a[c][c]) < 1e-12:
            return None
        for r in range(k):
            if r != c:
                f = a[r][c] / a[c][c]
                a[r] = [x - f * y for x, y in zip(a[r], a[c])]
    return [a[i][k] / a[i][i] for i in range(k)]


def probes(path):
    with open(path) as f:
        for line in f:
            try:
                line = json.loads(line)
            except json.JSONDecodeError:
                continue  # sqlprobe.mjs's human-readable start line.
            answer = line.get("answer")
            if isinstance(answer, dict) and answer.get("arm") in ("a", "b", "c"):
                yield line


def join(probe_rows, events):
    events = [e for e in events if e.get("path") == "/bench_seed"]
    out = []
    for p in probe_rows:
        lo, hi = ms(p["at"]) - 1500, ms(p["end"]) + 1500
        candidates = [e for e in events if lo <= e["ts"] <= hi]
        if not candidates:
            print(f'no Workers Logs invocation for rep {p["rep"]} {p["ask"]}', file=sys.stderr)
            continue
        event = max(candidates, key=lambda e: e.get("cpu") or 0)
        out.append((p, event))
    return out


def ratio(total, part):
    return total / part if part else None


def number(v):
    return f"{v:.6f}" if v is not None else "-"


def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: sqlprobe.py PROBE.jsonl WORKERS_LOGS.json")
    with open(sys.argv[2]) as f:
        events = json.load(f)
    joined = join(probes(sys.argv[1]), events)
    if not joined:
        raise SystemExit("no probe lines matched a /bench_seed invocation")

    print("arm  k/kb   rep  statements  rows  bytes  cpu ms  ms/statement  ms/row  ms/transaction")
    for p, event in joined:
        a, ask = p["answer"], p["ask"]
        size = f'k={a["k"]}' if a["arm"] in ("a", "b") else f'kb={a["kb"]}'
        cpu = event.get("cpu") or 0
        per_statement = ratio(cpu, a["statements"])
        per_row = ratio(cpu, a["rows"]) if a["arm"] in ("a", "b") else None
        per_tx = ratio(cpu, a["n"]) if a["arm"] == "c" else None
        print(f'{a["arm"]:3}  {size:6}  {p["rep"]:3}  {a["statements"]:10}  {a["rows"]:4}  {a["bytes"]:5}  '
              f'{cpu:6.3f}  {number(per_statement):>13}  {number(per_row):>6}  {number(per_tx):>15}')

    ab = [(p["answer"], event.get("cpu") or 0) for p, event in joined if p["answer"]["arm"] in ("a", "b")]
    fit = solve([[1, a["statements"], a["rows"]] for a, _ in ab], [cpu for _, cpu in ab])
    if fit is None:
        raise SystemExit("arms a/b are collinear: run both arms with at least two k values")
    intercept, call, row = fit
    print(f"\narms a/b fit over {len(ab)} calls: intercept {intercept:.6f} ms")
    print(f"ms per call: {call:.6f}")
    print(f"ms per row: {row:.6f}")
    print(f"one-row statement per-call share: {100 * call / (call + row):.2f}%")

    c = [(p["answer"], event.get("cpu") or 0) for p, event in joined if p["answer"]["arm"] == "c"]
    fit = solve([[1, a["kb"]] for a, _ in c], [cpu / a["n"] for a, cpu in c])
    if fit is None:
        raise SystemExit("arm c is collinear: run at least two kb values")
    tx, per_kb = fit
    print(f"\narm c fit over {len(c)} calls (cpu/n on kb):")
    print(f"ms per transaction: {tx:.6f}")
    print(f"ms per KB: {per_kb:.6f}")
    print(f"128 KB per-transaction share: {100 * tx / (tx + 128 * per_kb):.2f}%")


if __name__ == "__main__":
    main()
