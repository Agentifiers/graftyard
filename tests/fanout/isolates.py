#!/usr/bin/env python3
"""isolates.py TSAVE.jsonl[.xz] [...] -> the isolates a row's members of one group shared (`feed-per-member`).

tsave (`tests/nr3/tsave.mjs` with ISOLATES=1, which `b029.sh` and `b033.sh` set) logs `{at, site | learner, isolate,
since}` when a voter's or learner's test-faults `/isolate` id first shows or changes. The id is random per isolate
(`crate::faults`), so members that report one id ran in one isolate. On B0-29 m3fp3 learner 195 and site 0's voter did,
and isolate-wide state keyed by repository carried the voter's inputs into the learner's polls (220,515 "closure
invoked recursively or after being dropped" on site a, `tests/m0/results/looplag-clock-m3f.md`). A verdict reads this
before it trusts a site's tail: a shared isolate also shares CPU and memory, whatever the code.

Per file: how many members reported an isolate, then each shared isolate with its members, their roles (voter site N,
the leader at the time it first showed; learner N, the tsave's learner address), and when each first and last showed
there. Prints "no shared isolate" when none was. Exits 0 either way; 2 when a file has no isolate rows (ISOLATES off).
"""
import collections
import json
import lzma
import sys


def read(path):
    """Isolate rows in order, and the leader each `{site, leader}` row named, by time."""
    rows, leaders = [], []
    with (lzma.open if path.endswith('.xz') else open)(path, 'rt') as f:
        for line in f:
            if '"isolate"' not in line[:300] and '"leader"' not in line[:300]:
                continue
            d = json.loads(line)
            if 'isolate' in d:
                who = ('voter', d['site']) if 'site' in d else ('learner', d['learner'])
                rows.append((d['at'], who, d['isolate']))
            elif 'leader' in d and 'rows' in d:
                leaders.append((d['at'], d['leader']))
    return rows, leaders


def leader_at(leaders, at):
    named = [l for t, l in leaders if t <= at]
    return named[-1] if named else None


def shared(rows, leaders):
    """Each isolate id two or more members reported: [(id, [(who, role, first, last)])], `last` the time the member
    next showed elsewhere, or None while it stayed."""
    spans = collections.defaultdict(list)
    current = {}
    for at, who, iso in sorted(rows):
        prev = current.get(who)
        if prev is not None:
            spans[prev[0]][prev[1]] = spans[prev[0]][prev[1]][:3] + (at,)
        role = f'voter site {who[1]}' + (' (leader)' if who[0] == 'voter' and leader_at(leaders, at) == who[1] else '')
        if who[0] == 'learner':
            role = f'learner {who[1]}'
        spans[iso].append((who, role, at, None))
        current[who] = (iso, len(spans[iso]) - 1)
    return [(iso, s) for iso, s in spans.items() if len({w for w, _, _, _ in s}) > 1]


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    status = 0
    for path in sys.argv[1:]:
        rows, leaders = read(path)
        name = path.rsplit('/', 1)[-1]
        if not rows:
            print(f'{name}: no isolate rows (tsave without ISOLATES=1)')
            status = 2
            continue
        members = {who for _, who, _ in rows}
        found = shared(rows, leaders)
        print(f'{name}: {len(members)} members, {len({i for _, _, i in rows})} isolates; '
              + (f'{len(found)} shared' if found else 'no shared isolate'))
        for iso, s in found:
            print(f'  isolate {iso}:')
            for _, role, first, last in s:
                print(f'    {role}: from {first}' + (f' to {last}' if last else ''))
    sys.exit(status)


if __name__ == '__main__':
    main()
