#!/usr/bin/env python3
"""B0-07 r2's verdict (`tests/m5/b07r2/row.sh judge`), from the row's raw records alone. No network.

    judge.py <raw dir> <measured intent id>

Reads, all on Cloudflare time:
  entries.jsonl    the Release group's entries from relview ({index, at, kind, sigs, entry}), the group's own times
  elections.jsonl  every poll of every member's election trace ({at, rows: {chain: [[site, at, kind, term, peer,
                   index, extra]]}, missed}); kind 3 is a role change, extra the new role (2 leader)
  ops.jsonl        every bench write ({chain, t, ms, ok}), the scratch Worker's clock
  chains.json      {"c": [...], "ab": [...]}
  answers.txt      every entry this driver posted, one "<label> <answer>" per line
  ops-log.jsonl    graft-ops' Workers Logs lines over the row ({at, msg}), from tests/m5/bf46/logs.py
  status.jsonl     the scratch Worker's /status polls every 30 s; the newest before the fault naming a leader is a
                   chain's baseline
  fault.jsonl      the fault's start ({at}, the scratch Worker's clock), from run.log's GRAY FAULT line

Prints one JSON line per gate line and a summary, and exits 0 only when every line passes. B0-07 (build/BUILD-PLAN.md,
Milestone 0): "Callers on A and B report C, graft-ops undrains B under the 1% lead limit and drains C. Every group led
on C that served a request or committed an entry in the 50 s before the drain moves within 20 s of it, and every other
group led on C moves within 20 s of its next request". Fail: two sites drained at once, or any group on C slower. The
lead's lines for r2: c undrains with no hand Normal, the day reaches IntentDone, and the result names how many c-led
chains were active in the 50 s window; with none, the 20 s line is not exercised and the row does not pass.
"""
import gzip
import json
import os
import sys

ACTIVE_MS = 50_000
MOVE_MS = 20_000
C = 2
ROLE, LEADER = 3, 2
# A graft-ops run's longest stretch from a commit to its summary line [guess: a run reads the view, the deployments,
# the gate and 9 health reads, each bounded by VIEW_CALL, and runs end well inside a minute].
OPS_RUN_MS = 120_000
# A /status poll's own span: 480 member calls at 6 in flight.
POLL_MS = 30_000


def lines(path):
    """A JSON-lines file, or its committed `.gz`."""
    if os.path.exists(path):
        f = open(path)
    elif os.path.exists(path + ".gz"):
        f = gzip.open(path + ".gz", "rt")
    else:
        return []
    with f:
        return [json.loads(x) for x in f if x.strip()]


def site_mode(e):
    """(site, mode) of a SiteMode entry from relview's Debug text, else None."""
    if e["kind"] != "SiteMode":
        return None
    t = e["entry"]
    for s in range(3):
        if f"SiteId({s})" in t:
            return s, ("Drain" if "Drain" in t else "Normal")
    return None


def drains(entries):
    """Every SiteMode in order, and the most sites drained at once."""
    held, most, out = set(), 0, []
    for e in entries:
        m = site_mode(e)
        if not m:
            continue
        out.append({"index": e["index"], "at": e["at"], "site": m[0], "mode": m[1], "sigs": e["sigs"]})
        (held.add if m[1] == "Drain" else held.discard)(m[0])
        most = max(most, len(held))
    return out, most


def role_rows(polls):
    """chain -> sorted, deduplicated [(at, site, term, new role)] of role changes."""
    seen = {}
    for p in polls:
        for chain, rows in p.get("rows", {}).items():
            for site, at, kind, term, _peer, _index, extra in rows:
                if kind == ROLE:
                    seen.setdefault(chain, set()).add((at, site, term, extra))
    return {c: sorted(v) for c, v in seen.items()}


def leader_at(base, rows, t):
    """The site leading at time t: the baseline status poll's leader (site, term) moved by every role change after
    it, up to t. A leader that steps down leaves no leader until a member of an equal or later term wins."""
    best = base
    for at, site, term, role in rows:
        if at > t:
            break
        if role == LEADER and (best is None or term >= best[1]):
            best = (site, term)
        elif best and site == best[0] and term >= best[1] and role != LEADER:
            best = None
    return best[0] if best else None


def baseline(status, before):
    """chain -> (leader site, term, poll time): the newest status poll before `before` (the fault) that names a leader
    for the chain. Idle groups restored from storage know no leader until a request wakes them (F128), so the first
    poll may name none; the bench wakes them."""
    out = {}
    for p in status:
        if p["at"] >= before:
            break
        for chain, ms in p["groups"].items():
            lead = [m for m in ms if len(m) > 2 and m[1] == "leader"]
            if lead:
                out[chain] = (lead[0][0], lead[0][2], p["at"])
    return out


def moved_at(rows, td):
    """The first time after td that a member off C became leader, else None."""
    for at, site, _term, role in rows:
        if at > td and role == LEADER and site != C:
            return at
    return None


def judge(raw, intent):
    # The group's whole history is in the file; the row judges only what follows its own intent.
    entries = lines(f"{raw}/entries.jsonl")
    start = next((e["index"] for e in entries if e["kind"] == "Intent" and e["entry"].endswith(f"id {intent}")), None)
    if start is None:
        return [{"line": "the measured intent is in the group", "pass": False, "intent": intent}]
    entries = [e for e in entries if e["index"] >= start]
    polls = lines(f"{raw}/elections.jsonl")
    ops = lines(f"{raw}/ops.jsonl")
    with open(f"{raw}/chains.json") as f:
        chains = json.load(f)
    posted = open(f"{raw}/answers.txt").read() if os.path.exists(f"{raw}/answers.txt") else ""
    ops_log = lines(f"{raw}/ops-log.jsonl")
    out = []

    modes, most = drains(entries)
    drain_c = next((m for m in modes if m["site"] == C and m["mode"] == "Drain"), None)
    normal_c = next((m for m in modes if drain_c and m["site"] == C and m["mode"] == "Normal" and m["index"] > drain_c["index"]), None)
    b_normal = next((m for m in modes if drain_c and m["site"] == 1 and m["mode"] == "Normal" and m["index"] < drain_c["index"]), None)
    out.append({"line": "never two sites drained", "pass": most <= 1, "most_drained": most, "modes": modes})
    out.append({"line": "graft-ops undrains b under the lead limit, then drains c",
                "pass": bool(drain_c and b_normal), "b_normal": b_normal, "c_drain": drain_c})
    # The driver posts no SiteMode, so any in answers.txt is a hand undrain; and graft-ops logs `Committed("SiteMode")`
    # for each one it commits, at the end of the run that committed it (within OPS_RUN_MS of the entry).
    hand = [x for x in posted.splitlines() if "SiteMode" in x or x.startswith("normal")]
    by_ops = normal_c is not None and any(
        'Committed("SiteMode")' in o["msg"] and 0 <= o["at"] - normal_c["at"] <= OPS_RUN_MS for o in ops_log)
    out.append({"line": "c undrains with no hand Normal", "pass": bool(normal_c) and not hand and by_ops,
                "c_normal": normal_c, "graft_ops_committed_it": by_ops, "hand_posts": hand})
    done = next((e for e in entries if e["kind"] == "IntentDone" and intent in e["entry"]), None)
    out.append({"line": "the day reaches IntentDone", "pass": done is not None, "intent_done": done})

    if not drain_c:
        out.append({"line": "every group led on c moves within 20 s", "pass": False, "why": "c was never drained"})
        return out
    td = drain_c["at"]
    fault = next((x["at"] for x in lines(f"{raw}/fault.jsonl")), td)
    base = baseline(lines(f"{raw}/status.jsonl"), fault)
    # A poll reads its chains over up to POLL_MS before its `at`, so role rows from that stretch are applied on top of
    # its leader; term order keeps an older row from undoing a newer leader. With no baseline, every row applies.
    every = role_rows(polls)
    rows = {c: [r for r in every.get(c, []) if c not in base or r[0] > base[c][2] - POLL_MS] for c in chains["c"]}
    base = {c: v[:2] for c, v in base.items()}
    missed = sorted({(c, s) for p in polls for c, s in p.get("missed", []) if c in chains["c"]})
    by_chain = {}
    for o in ops:
        by_chain.setdefault(o["chain"], []).append(o)
    groups = []
    for c in chains["c"]:
        r = rows.get(c, [])
        if leader_at(base.get(c), r, td) != C:
            continue
        mv = moved_at(r, td)
        w = by_chain.get(c, [])
        served = [o for o in w if o["t"] <= td and o["t"] + o["ms"] >= td - ACTIVE_MS]
        committed = [o for o in served if o["ok"] and td - ACTIVE_MS <= o["t"] + o["ms"] <= td]
        nxt = min((o["t"] for o in w if o["t"] >= td), default=None)
        active = bool(served)
        ms = None if mv is None else mv - td
        since_next = None if mv is None or nxt is None else mv - nxt
        ok = ms is not None and (ms <= MOVE_MS or (not active and since_next is not None and since_next <= MOVE_MS))
        g = {"chain": c, "active": active, "committed_in_window": len(committed), "moved_ms": ms,
             "since_next_request_ms": since_next, "pass": ok}
        groups.append(g)
    act = [g for g in groups if g["active"]]
    ms = sorted(g["moved_ms"] for g in groups if g["moved_ms"] is not None)
    # A chain with no baseline leader and no role row before the drain cannot be placed, so it is counted, not judged
    # as off c: the line fails if any c-homed chain is unplaced.
    unplaced = [c for c in chains["c"] if c not in base and not any(r[0] <= td for r in rows.get(c, []))]
    out.append({"line": "every c-homed chain's leader at the drain is known", "pass": not unplaced,
                "unplaced": unplaced, "c_homed": len(chains["c"])})
    out.append({"line": "c-led chains active in the 50 s before the drain", "pass": len(act) > 0,
                "led_on_c_at_drain": len(groups), "active": len(act), "idle": len(groups) - len(act),
                "why": None if act else "no c-led chain was active, so the 20 s line was not exercised"})
    out.append({"line": "every group led on c moves within 20 s (active: of the drain; idle: of its next request)",
                "pass": bool(groups) and all(g["pass"] for g in groups),
                "passed": sum(g["pass"] for g in groups), "of": len(groups),
                "active_max_ms": max((g["moved_ms"] or 10**9 for g in act), default=None),
                "p50_ms": ms[len(ms) // 2] if ms else None, "max_ms": ms[-1] if ms else None,
                "late": [g for g in groups if not g["pass"]], "members_missed": len(missed)})
    return out


if __name__ == "__main__":
    res = judge(sys.argv[1], sys.argv[2])
    for r in res:
        print(json.dumps(r))
    ok = all(r["pass"] for r in res)
    print(json.dumps({"verdict": "PASS" if ok else "FAIL"}))
    sys.exit(0 if ok else 1)
