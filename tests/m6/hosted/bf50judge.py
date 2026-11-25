#!/usr/bin/env python3
"""BF-50's judge on graft-m6a (`tests/m6/results/m6-wp1.md`, "BF-50"). Reads a row's raw folder and prints one JSON
line per check. Every input is a file the row driver (`bf50-row.sh`) wrote:

  rows-<when>.json      every registry row on sites 0-3, `[{chain, do_name, generation, sites, site}]`
  configs-<when>.json   each chain's committed config, `{chain: {index, cfg} | {error}}`
  entries.json          relview's entries from the SiteLost (or the drain) on, `[{index, kind, sigs, entry}]`
  keys.json             relview's `keys` (customer, drain, executor hex)
  coordinator.json      the coordinator instance's REST record (`result.steps[].name`)
  groups.jsonl          one line per group instance: `{id, status, steps: [names]}`
  load.json, read-<when>.json  the load's last acknowledged value per register chain, and the reads after

  python3 tests/m6/hosted/bf50judge.py <raw folder> <check> [lost] [spare]
checks: line2 (before -> after the replace), home (before -> after the restore), signers, line3, campaign, writes
"""
import json
import sys
from pathlib import Path

raw = Path(sys.argv[1])
check = sys.argv[2]
LOST = int(sys.argv[3]) if len(sys.argv) > 3 else 0
SPARE = int(sys.argv[4]) if len(sys.argv) > 4 else 3


def load(name):
    text = (raw / name).read_text()
    if name.endswith(".jsonl"):
        return [json.loads(line) for line in text.splitlines() if line.strip()]
    return json.loads(text)


def voters(cfg):
    if not cfg:
        return []
    if "normal" in cfg:
        halves = [cfg["normal"]]
    elif "degraded" in cfg:
        halves = [cfg["degraded"]["placement"]]
    elif "joint" in cfg:
        halves = [cfg["joint"]["old"], cfg["joint"]["new"]]
    else:
        halves = []
    return [v for h in halves for v in h.get("voters", [])]


def sites_of(c):
    return sorted({v["site"] for v in voters(c.get("cfg"))})


# Excluded from every group judged: the 18 `Tokens` chains with every member joined and no leader after the precreate
# (`tests/m6/results/m6-wp1.md`, "Idle leaders of half-made chains"). No working group, so none can count as moved.
EXCLUDED = {f"tokens:{n}" for n in (133, 169, 281, 417, 476, 518, 600, 628, 741, 745, 819, 919, 1331, 1435, 1598,
                                    2125, 2310, 2378)}
# And `repo:0`, the install's placement tries: three members of different generations on sites 0, 2 and 3, each at
# commit 0 with no leader, so no group ever formed.
EXCLUDED.add("repo:0")


def unjoined():
    """Chains `before` found with a row member that has not joined, each with its reason, excluded like EXCLUDED."""
    p = raw / "unjoined-before.json"
    return {u["chain"]: u["why"] for u in json.loads(p.read_text())} if p.exists() else {}


def compare(before_name, after_name, held_site, then_on, then_off, home=None):
    before, after = load(before_name), load(after_name)
    # A reread (`<after>-again.json`, chains the first read left unread, read again once their homes woke) fills the
    # gaps: m6aL's home read ran while groups idled and left 60 unread and some stale.
    again = raw / after_name.replace(".json", "-again.json")
    if again.exists():
        a = json.loads(again.read_text())
        # `{chain: cfg}`, or `snapshot`'s `[{key: "chain|do_name", value: cfg}]`.
        items = a.items() if isinstance(a, dict) else ((e["key"].split("|")[0], e["value"]) for e in a)
        for k, v in items:
            if "cfg" in v and v.get("index", -1) >= after.get(k, {}).get("index", -1):
                after[k] = v
    EXCLUDED.update(unjoined())
    excluded = sorted(k for k in before if k in EXCLUDED)
    before = {k: c for k, c in before.items() if k not in EXCLUDED}
    held = {k for k, c in before.items() if "cfg" in c and held_site in sites_of(c)}
    # Controls hold no voter on the site the groups were held on, nor on the one this move leaves: a restore moves
    # every group off the spare, so m6aL's controls on [1, 2, 3] were moved by its restore, correctly.
    other = {k for k, c in before.items() if "cfg" in c and held_site not in sites_of(c) and then_off not in sites_of(c)}
    unread = sorted(k for k, c in before.items() if "cfg" not in c)
    short, kinds = [], {}
    for k in sorted(held):
        a = after.get(k, {})
        s = sites_of(a)
        kind = next(iter(a.get("cfg") or {"none": 0}))
        kinds[kind] = kinds.get(kind, 0) + 1
        ok = "cfg" in a and kind == "normal" and then_on in s and then_off not in s and len(s) == len(sites_of(before[k]))
        # A restore puts each group's home back where it was before the loss: A for A-homed chains, B for the
        # B-homed register chains, which a fixed `home == A` judged short (m6aL: repo:6101 to 6108).
        if ok and home is not None:
            ok = a["cfg"]["normal"]["home"] == before[k]["cfg"]["normal"]["home"]
        if not ok:
            short.append({"chain": k, "before": sites_of(before[k]), "after": s, "kind": kind, "err": a.get("error")})
    changed = [k for k in sorted(other) if after.get(k, {}).get("index") != before[k]["index"]
               or after.get(k, {}).get("cfg") != before[k]["cfg"]]
    print(json.dumps({
        "check": check, "chains": len(before), "held": len(held), "moved": len(held) - len(short),
        "short": len(short), "short_first": short[:10], "after_kinds": kinds,
        "untouched_controls": len(other) - len(changed), "controls": len(other), "controls_changed": changed[:10],
        "unread_before": unread[:10], "excluded": len(excluded), "pass": not short and not changed and not unread,
    }))


if check == "line2":
    compare("configs-before.json", "configs-after.json", LOST, SPARE, LOST)
elif check == "home":
    # After the restore: every group that held a voter on the lost site holds one there again, none on the spare,
    # and its home is back on the restored site.
    compare("configs-before.json", "configs-home.json", LOST, LOST, SPARE, home=LOST)
elif check == "signers":
    keys = load("keys.json")
    who = {keys["customer"][:6]: "customer", keys["executor"][:6]: "executor", keys["drain"][:6]: "drain",
           keys["release"][:6]: "release"}
    if keys.get("vendor"):
        who[keys["vendor"][:6]] = "vendor"
    out = [{"index": e["index"], "kind": e["kind"], "by": [who.get(s, s) for s in e["sigs"]], "entry": e["entry"][:90]}
           for e in load("entries.json")]
    # The ReplaceMember path: its intent, then every IntentStep, Halt and IntentDone up to its IntentDone (relview
    # prints those without the intent id; no other intent is current while it runs). A drain-key SiteMode is the
    # drain's own path, listed with the rest but not part of it.
    start = next((i for i, e in enumerate(out) if "ReplaceMember" in e["entry"] and " id " in e["entry"]), None)
    path, rid = [], None
    if start is not None:
        rid = out[start]["entry"].split(" id ")[1].strip()
        for e in out[start:]:
            if e is out[start] or e["kind"] in ("IntentStep", "IntentDone", "Halt"):
                path.append(e)
            if e["kind"] == "IntentDone":
                break
    done = bool(path) and path[-1]["kind"] == "IntentDone"
    print(json.dumps({"check": check, "entries": out, "replace_intent": rid, "path": [e["index"] for e in path],
                      "replace_done": done, "path_signers": sorted({b for e in path for b in e["by"]}),
                      "pass": done and all(e["by"] == ["executor"] for e in path)}))
elif check == "line3":
    names = [s["name"] for s in load("coordinator.json")["result"]["steps"] if s.get("name")]
    rows = [n for n in names if n.startswith("rows ")]
    by = {}
    for n in rows:
        site = n.split()[1].split("/")[0]
        by[site] = by.get(site, 0) + 1
    print(json.dumps({"check": check, "steps": len(names), "rows_by_site": by,
                      "refill": [n for n in names if n.startswith("refill")][:6],
                      "pass": set(by) == {"1", "2"} and by["1"] == 64 and by["2"] == 64}))
elif check == "campaign":
    groups = [json.loads(line) for line in (raw / "groups.jsonl").read_text().splitlines() if line.strip()]
    camp = [g for g in groups if any(n.startswith("campaign ") for n in g["steps"])]
    steps = sum(sum(1 for n in g["steps"] if n.startswith("campaign ")) for g in groups)
    held = {k for k, c in load("configs-before.json").items()
            if "cfg" in c and LOST in sites_of(c) and k not in EXCLUDED}
    status = {}
    for g in groups:
        status[g["status"]] = status.get(g["status"], 0) + 1
    # Each campaign step's output is the member's `/campaign` verdict: `won`, `started` and `leader` elected it (or
    # found it leading), anything else (`promised`, `lost`, `busy`, an HTTP error) did not.
    verdicts = {}
    for g in groups:
        for v in g.get("verdicts", []):
            verdicts[str(v)] = verdicts.get(str(v), 0) + 1
    elected = sum(1 for g in camp if any(str(v) in ("won", "started", "leader", "resumed") for v in g.get("verdicts", [])))
    print(json.dumps({"check": check, "groups": len(groups), "status": status, "campaign_steps": steps,
                      "chains_campaigned": len(camp), "chains_elected": elected, "verdicts": verdicts,
                      "held": len(held),
                      "share_of_held": round(len(camp) / len(held), 3) if held else None}))
elif check == "writes":
    last = load("load.json")["last"]
    got = load(sys.argv[5] if len(sys.argv) > 5 else "read-home.json")
    # A read answer is `{status, by, v: value}` (bf50-edge's `/read` reads the home as leader, after a campaign). A
    # `.jsonl` of `{chain, got}` lines (m6aL's recheck, `read-home-leader.jsonl`) is read the same way.
    if isinstance(got, list):
        got = {g["chain"]: {"v": g["got"]} for g in got}
    miss = [{"chain": k, "want": v, "got": got.get(k)} for k, v in sorted(last.items())
            if (got.get(k) or {}).get("v") != v]
    print(json.dumps({"check": check, "acked_chains": len(last), "present": len(last) - len(miss),
                      "missing": miss[:10], "pass": not miss}))
else:
    sys.exit(f"unknown check {check}")
