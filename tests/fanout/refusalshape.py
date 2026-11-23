#!/usr/bin/env python3
"""refusalshape.py A1ROW [STATUS] -> B0-33's refusal-shape line as ruled (FRAME, "Operator decision, 2026-11-23,
B0-33's merge-queue and refusal-shape lines"): every Agent-class refusal in the git `ng` or REST 429 shape, and every
Human or Bot 503 with `Retry-After`. Then the 5xx and 429s by class, route, read or write, rule, and `Retry-After`.

A1ROW is a1.mjs's row (`tests/am1/raw/a1-b0-33-<run>.jsonl`, the last line is read). A row with `refusals` (a1.mjs
since b033-shape) is read as it is: a1 counts every non-2xx answer in the load window by its token's class, so
`Retry-After` is measured. An older row has none: then STATUS (`tests/fanout/raw/rows-<tag>.status`) must hold
fivexxwhy.py's block, the edge's 5xx by route and reason, and each route's 503s are classed by a1's per-kind codes, as
`tests/am1/results/edge-refusal.md`, "(b) erDA's 5,228 edge 503s", read them: a route whose a1 kinds are all one class
takes the edge's count; a mixed route takes a1's counts per class and the rest of the edge's is `unknown`. Such a table
has no `Retry-After`, and the line is not judged from it.

An Agent push refused `full` is a 200 whose report says `ng ... agent admission full` (a1's `push:agent` result
`full`), counted as `ng`. A 4xx other than 429 is an answer, not a refusal, and is left out.
"""
import collections
import json
import re
import sys

SHED, WARM = "every member asked shed the read", "warming"


def rule(status, why):
    if status == 429:
        return "429 (secondary rate limit)"
    if SHED in why or "overloaded" in why:
        return "F65 member shed"
    if WARM in why:
        return "git cache warming"
    return "other"


# An older row's per-kind codes: each kind's route (the edge's `route_kind`), class (its token: the admin's and the fast
# user's PATs are Human, the App's is Agent), and which codes are that route's 503s.
KINDS = {
    "side:comment": ("graphql", "Agent", "gql 503"), "side:draftPr": ("graphql", "Agent", "gql 503"),
    "side:human": ("graphql", "Human", "gql 503"), "fast-user": ("graphql", "Human", "503"),
    "side:label": ("rest-repo-write", "Agent", "503"), "side:taskEdit": ("task", "Agent", "503"),
    "side:standIn": ("task", "Agent", "503"), "side:leaseRenew": ("renew", "Agent", "503"),
    "side:lease": ("lease", "Agent", "503"), "merge:only": ("merge", "Human", "503"),
    "push:human": ("receive-pack", "Human", "http 503"), "read:ref": ("upload-pack", "Human", "503"),
    "read:cloneTtfb": ("upload-pack", "Human", "503"),
}
WRITES = {"graphql", "rest-repo-write", "task", "renew", "lease", "merge", "receive-pack"}


def last(path):
    with open(path) as f:
        out = None
        for line in f:
            if line.strip():
                out = line
    return json.loads(out)


def edge_block(status_path):
    """fivexxwhy.py's `by reason:` lines: [(status, route, reason, n)], and its `edge5xx` total."""
    rows, total, on = [], None, False
    for line in open(status_path):
        if line.startswith('{"logged"'):
            total, rows, on = json.loads(line)["edge5xx"], [], False
        elif line.startswith("by reason:"):
            on = True
        elif on:
            m = re.match(r"\s*(\d+) (\d{3}) (\S+)\s+(.*)$", line)
            if not m:
                on = False
                continue
            rows.append((int(m[2]), m[3], m[4].strip(), int(m[1])))
    if total is None:
        sys.exit(f"{status_path}: no fivexxwhy.py block")
    return rows, total


def legacy(row, status_path):
    """[(cls, route, write, status, retryAfter, rule, n)] for an older row, and the edge's 5xx total."""
    by_kind = row["byKind"]
    edge, total = edge_block(status_path)
    a1 = collections.defaultdict(collections.Counter)  # route -> class -> a1's 503s
    for k, (route, cls, code) in KINDS.items():
        codes = by_kind.get(k, {}).get("codes", {})
        n = sum(v for c, v in codes.items() if c == code or c.startswith(code + " "))
        if k == "push:human":
            a1["info-refs"][cls] += codes.get("adv 503", 0)
        if n:
            a1[route][cls] += n
    reasons = collections.Counter(route for _, route, _, _ in edge)
    out = []
    for status, route, why, n in edge:
        classes = a1.get(route, {})
        if len(classes) == 1:
            split = {next(iter(classes)): n}
        elif classes and reasons[route] == 1:
            # A route two classes reach: a1's count for each, and what the edge answered past them is unknown.
            split, left = {}, n
            for c, m in classes.most_common():
                split[c] = min(m, left)
                left -= split[c]
            split["unknown"] = left
        else:
            split = {"unknown": n}
        out += [(c, route, route in WRITES, status, None, rule(status, why), m) for c, m in split.items() if m]
    return out, total


def measured(row):
    return [(r["cls"], r["route"], r["write"], r["status"], r["retryAfter"], rule(r["status"], r["why"]), r["n"])
            for r in row["refusals"] if r["status"] == 429 or r["status"] >= 500]


def main():
    row = last(sys.argv[1])
    status = sys.argv[2] if len(sys.argv) > 2 else None
    new = "refusals" in row
    if new:
        rows, total = measured(row), (edge_block(status)[1] if status else None)
    elif status:
        rows, total = legacy(row, status)
    else:
        sys.exit("an older row (no `refusals`) needs STATUS, with fivexxwhy.py's block")
    ng = row["byKind"].get("push:agent", {}).get("codes", {}).get("full", 0)
    count = lambda f: sum(r[6] for r in rows if f(*r[:6]))
    by = collections.Counter()
    for cls, _, _, st, _, _, n in rows:
        by[f"{cls} {st}"] += n
    print(json.dumps({"source": "a1 refusals" if new else "edge fivexx joined to a1 kinds", "edge5xx": total,
                      "rows5xx": count(lambda c, r, w, st, ra, rl: st >= 500), "agentNg": ng,
                      "byClass": dict(sorted(by.items()))}))
    print(f"{'n':>6} | class | route | write | status | retry-after | rule")
    for cls, route, write, st, ra, rl, n in sorted(rows, key=lambda r: (r[0], -r[6])):
        shown = (ra or "none") if new else "not recorded"
        print(f"{n:6d} | {cls} | {route} | {'write' if write else 'read'} | {st} | {shown} | {rl}")
    if not new:
        print("refusal shape: not judged (this row records no Retry-After)")
        return
    agent_ok = count(lambda c, r, w, st, ra, rl: c == "Agent" and st == 429 and ra) + ng
    agent_bad = count(lambda c, r, w, st, ra, rl: c == "Agent" and not (st == 429 and ra))
    human_503 = count(lambda c, r, w, st, ra, rl: c in ("Human", "Bot") and st == 503)
    human_bad = count(lambda c, r, w, st, ra, rl: c in ("Human", "Bot") and st == 503 and not ra)
    verdict = "met" if agent_bad == 0 and human_bad == 0 else "not met"
    print(f"refusal shape: Agent refusals {agent_ok + agent_bad}, ng or 429 with Retry-After {agent_ok} (ng {ng}), "
          f"other {agent_bad}; Human or Bot 503 {human_503}, without Retry-After {human_bad}: {verdict}")


if __name__ == "__main__":
    main()
