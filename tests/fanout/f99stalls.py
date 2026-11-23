"""F99's count ($0, 2026-11-24): leader storage-confirm stalls per hour of kept trace, by row family.

    python3 tests/fanout/f99stalls.py [--list]

F99's method (`decisions/OPEN-QUESTIONS.md`, F99, 2026-11-19), kept so the counts compare:
  - every `tests/fanout/raw/tsave-*.jsonl.xz`; voter lines only (no `learner`); kind 10 rows (`Trace::Confirm`: `at` is
    when the confirm ended, `ms` its wall);
  - a row is a leader's when its poll line's `leader` equals its `site` (the leader at read time, an inference: a stall
    begun on the leader and read after it lost the lead counts as a follower's, as m3fhg's did);
  - per site, confirms of 500 ms or more grouped into events: a confirm joins the event when it ends within 10 s of the
    event's first, and the event takes its largest wall; buckets 0.5-2 s, 2-10 s, and 10 s or more;
  - a trace's hours are its confirm rows' span, first to last, setup included.
What differs from 2026-11-19:
  - rows are parsed by `json` as exact integers, never through float, and deduplicated only on the exact tuple
    (site, at, kind, peer, batch, index, ms), since a poll re-reads the ring (LM-1's caution on heavyturns' `trace()`);
  - the family comes from the trace's name where it carries one (`-b0-31`, `-b0-32`; the B0-35 rows m58 to m65), else
    from its own status file (`rows-TAG.status` names b0-29 or b0-33), instead of name patterns alone;
  - 10 s-or-more events on every voter, and 5 s-or-more leader events, are listed with syncs.py's kind 31 verdict when
    the trace has `Trace::Sync` rows, and with the leader changes `terms-TAG.jsonl` shows from the stall's start to 20 s
    after its end.
The second table prices candidate (b), an earlier pre-vote: a follower's silences from its leader (no `AppendStepped`
17 or `ResStepped` 3 row from the leader's site), counted only where the follower's own trace has no hole over 1 s,
and of those, the ones that overlap one of that leader's confirm events of 2 s or more (within 1 s): the silences a
stall caused, against the rest, where the leader sent nothing for another reason.
"""
import bisect
import collections, glob, json, lzma, os, re, sys
from datetime import datetime

RAW = 'tests/fanout/raw'


def family(tag):
    if tag.endswith('-b0-31') or tag.endswith('-b0-32'):
        return tag[-5:].upper()
    if re.fullmatch(r'm(5[89]|6[0-5])', tag):
        return 'B0-35'
    st = f'{RAW}/rows-{tag}.status'
    if os.path.exists(st):
        c = collections.Counter(m.lower() for m in re.findall(r'[bB]0-(29|32|33)\b', open(st).read()))
        if c:
            return 'B0-' + c.most_common(1)[0][0]
    return None


def iso_ms(at):
    return int(datetime.fromisoformat(at.replace('Z', '+00:00')).timestamp() * 1000)


def verdict(r):
    """syncs.py's `verdict` on a kind 31 row (its module reads argv at import, so the rule is restated here)."""
    b, wall = int(r[3]), int(r[5])
    wakes, inputs, lag0, lag1 = int(r[2]), b & 0xfffff, (b >> 20) & 0xffff, b >> 36
    if lag0 * 2 >= wall:
        return 'isolate late'
    if wakes == 0 and inputs == 0 and lag1 * 2 >= wall:
        return 'gate held'
    return 'storage' if wakes or inputs else 'unclear'


def events(rows):
    out = []
    for t, ms in sorted(rows):
        if ms < 500:
            continue
        if out and t - out[-1][0] < 10000:
            out[-1] = (out[-1][0], max(out[-1][1], ms))
        else:
            out.append((t, ms))
    return out


def bucket(ms):
    return 0 if ms < 2000 else 1 if ms < 10000 else 2


fam = collections.defaultdict(lambda: {'traces': 0, 'hours': 0.0, 'lead': [0, 0, 0], 'any': [0, 0, 0], 'rows': 0, 'quiet': [0, 0, 0, 0]})
big, skipped = [], []
for path in sorted(glob.glob(f'{RAW}/tsave-*.jsonl.xz')):
    tag = path.split('tsave-')[1][:-len('.jsonl.xz')]
    k = family(tag)
    if not k:
        skipped.append(tag)
        continue
    seen, lead, every, syncs = set(), collections.defaultdict(list), collections.defaultdict(list), collections.defaultdict(list)
    heard, rowsat = collections.defaultdict(set), collections.defaultdict(set)
    lo = hi = None
    n = 0
    terms, changes = [], []
    tf = f'{RAW}/terms-{tag}.jsonl'
    if os.path.exists(tf):
        for line in open(tf):
            d = json.loads(line)
            if 'members' in d:
                ld = tuple(sorted((m[0], m[2]) for m in d['members'] if m[1] == 'leader'))
                if terms and terms[-1][1] != ld:
                    changes.append(iso_ms(d['at']))
                terms.append((iso_ms(d['at']), ld))
    for line in lzma.open(path, 'rt'):
        d = json.loads(line)
        if 'rows' not in d or d.get('learner') is not None:
            continue
        site, leader = d.get('site'), d.get('leader')
        for r in d['rows']:
            rowsat[site].add(r[0])
            if r[1] in (3, 17) and leader is not None and leader != site and r[2] == leader:
                heard[(site, leader)].add(r[0])
            if r[1] not in (10, 31):
                continue
            key = (site, *r)
            if key in seen:
                continue
            seen.add(key)
            if r[1] == 31:
                syncs[site].append(r)
                continue
            n += 1
            t, ms = r[0], r[5]
            lo, hi = (t, t) if lo is None else (min(lo, t), max(hi, t))
            every[site].append((t, ms))
            if d.get('leader') == site:
                lead[site].append((t, ms))
    if lo is None:
        skipped.append(tag)
        continue
    f = fam[k]
    for (site, ld), ts in heard.items():
        xs, a = sorted(ts), sorted(rowsat[site])
        for x, y in zip(xs, xs[1:]):
            if y - x < 5000:
                continue
            pts = [x] + a[bisect.bisect_right(a, x):bisect.bisect_left(a, y)] + [y]
            if max(q - p for p, q in zip(pts, pts[1:])) > 1000:
                continue
            stalled = any(e - ms - 1000 <= y and x <= e + 1000 for e, ms in events(sorted(set(lead.get(ld, [])))) if ms >= 2000)
            for i, bound in enumerate((5000, 9000)):
                if y - x >= bound:
                    f['quiet'][i] += 1
                    f['quiet'][2 + i] += stalled
    f['traces'] += 1
    f['hours'] += (hi - lo) / 3.6e6
    f['rows'] += n
    for site, v in lead.items():
        for _, ms in events(v):
            f['lead'][bucket(ms)] += 1
    for site, v in every.items():
        for t, ms in events(v):
            f['any'][bucket(ms)] += 1
            led = any(abs(t - x) < 10000 for x, _ in lead.get(site, []))
            if ms >= 10000 or (ms >= 5000 and led):
                sv = [s for s in syncs.get(site, []) if abs(s[0] - t) <= 2000 and s[5] >= 0.8 * ms]
                after = [round((c - t) / 1000, 1) for c in changes if t - ms <= c <= t + 20000]
                big.append((k, tag, site, ms, led, verdict(sv[0]) if sv else 'no kind 31 row', after if terms else 'no terms file'))

print('| Family | Traces | Hours | Leader 0.5-2 s per h | Leader 2-10 s per h | Leader 10 s or more | Any voter 10 s or more |')
print('| --- | --- | --- | --- | --- | --- | --- |')
tot = {'traces': 0, 'hours': 0.0, 'lead': [0, 0, 0], 'any': [0, 0, 0]}
for k in sorted(fam):
    f = fam[k]
    h = f['hours']
    print(f"| {k} | {f['traces']} | {h:.2f} | {f['lead'][0] / h:.1f} ({f['lead'][0]}) | {f['lead'][1] / h:.1f} ({f['lead'][1]}) | "
          f"{f['lead'][2]} | {f['any'][2]} |")
    tot['traces'] += f['traces']
    tot['hours'] += h
    for i in range(3):
        tot['lead'][i] += f['lead'][i]
        tot['any'][i] += f['any'][i]
h = tot['hours']
print(f"| All | {tot['traces']} | {h:.2f} | {tot['lead'][0] / h:.1f} ({tot['lead'][0]}) | {tot['lead'][1] / h:.1f} ({tot['lead'][1]}) | "
      f"{tot['lead'][2]} | {tot['any'][2]} |")
print(f'skipped (no family or no confirm rows): {skipped}')
print()
print('| Family | Hours | Follower silences of 5 s or more, per h | 9 s or more | 5 s or more, over a leader stall | 9 s or more, over a leader stall |')
print('| --- | --- | --- | --- | --- | --- |')
for k in sorted(fam):
    q, h = fam[k]['quiet'], fam[k]['hours']
    print(f'| {k} | {h:.2f} | ' + ' | '.join(f'{x / h:.1f} ({x})' for x in q) + ' |')
q = [sum(fam[k]['quiet'][i] for k in fam) for i in range(4)]
print(f'| All | {tot["hours"]:.2f} | ' + ' | '.join(f'{x / tot["hours"]:.1f} ({x})' for x in q) + ' |')
print('10 s or more on any voter, and 5 s or more on a leader (family, trace, site, wall ms, a leader row within 10 s, '
      'syncs.py verdict, leader changes from its start to 20 s after its end, in s from its end):')
for b in big:
    print(' ', b)
if '--list' in sys.argv:
    for k in sorted(fam):
        print(k, fam[k]['rows'], 'confirm rows')
