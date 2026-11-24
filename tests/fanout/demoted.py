#!/usr/bin/env python3
"""demoted.py A1ROW -> B0-33's user-token line and the Human guard (user-demote), from a1.mjs's row: the leader's
`/stats` `demoted` (the actors DEMOTE_PER_MIN classed Agent, and their writes, by actor key) and `humanActors` (the
fast user's key and the Human writers').

- The fast user, 100 writes a minute on a plain PAT, must be demoted: that is "the user token admitted as Agent". Not
  demoted is a fail of the line. A row whose leader answered no `demoted` (a site before user-demote) cannot read it.
- No Human writer may be demoted: a1 spreads the Human load so each token stays under DEMOTE_PER_MIN, and a demoted one
  means the Human line measured an Agent-classed writer, so the run is invalid for the Human line (not a fail).

A1ROW is `tests/am1/raw/a1-b0-33-<run>.jsonl`; its last line is read.
"""
import json
import sys


def last(path):
    out = None
    with open(path) as f:
        for line in f:
            if line.strip():
                out = line
    return json.loads(out)


def lines(row):
    d = (row.get("leader") or {}).get("demoted")
    who = row.get("humanActors") or {}
    if d is None or not who:
        return ["user token admitted as Agent: not reachable (the leader's /stats has no `demoted`, or the row no "
                "`humanActors`)", "Human writers demoted: not read"]
    by = d.get("by") or {}
    fast = who.get("fastUser")
    n = by.get(fast, 0)
    out = [f"user token admitted as Agent: fast user {fast} demoted {n} writes"
           f" ({d.get('actors', 0)} actors, {d.get('writes', 0)} writes demoted in all): {'met' if n else 'FAIL'}"]
    hs = [(h, by[h]) for h in who.get("humans", []) if by.get(h)]
    out.append("Human writers demoted: " + (", ".join(f"{h} {k}" for h, k in hs) +
                                            ": INVALID for the Human line" if hs else "none"))
    return out


if __name__ == "__main__":
    print("\n".join(lines(last(sys.argv[1]))))
