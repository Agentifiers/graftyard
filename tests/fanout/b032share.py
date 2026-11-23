#!/usr/bin/env python3
"""b032share.py A1_JSONL: what share of a B0-32 row's within-5 s miss each lever could recover, from the burst's own
`server-timing` marks (the a1 file's "burst timing" row), for tests/am1/results/lm1-rows-m3f.md, "B0-32 after m3fhs1".

The line counts a burst push as met when it is `ok` within 5 s on the edge clock (`total`) or answered `full`. Only the
late `ok` pushes can move, so each counterfactual takes their marks and asks how many would end within 5 s:
  - commit cap C: every commit stage held to at most C ms (a leader whose commits no longer wait behind its own CPU);
  - commit scale x: every commit stage times x;
  - stage first (A36): a push that committed twice loses its refused first commit and its `stage` (the place), and pays
    VADD ms more in validate, since a pack staged first places beside validate (`place_teed`);
  - gate cap G: the read gate held to at most G ms.
Each is an upper bound in one way: it moves no push's arrival, so it cannot show m3fgt1's bunching.
"""
import collections, json, re, sys

VADD = 170  # m3fhs1: validate p50 527 ms on staged-first pushes against 361 inline [measured]


def marks(p):
    out = collections.defaultdict(list)
    for m in re.finditer(r'(\w+);dur=(\d+)', p.get('timing') or ''):
        out[m.group(1)].append(int(m.group(2)))
    return out


def moved(late, cap=None, scale=None, stage=False, gate=None, vadd=VADD):
    n = 0
    for s in late:
        t, cs = s['total'][0], list(s['commit'])
        if stage and len(cs) > 1:
            t -= cs[0] + sum(s['stage']) - vadd
            cs = cs[1:]
        for c in cs:
            to = c if cap is None else min(c, cap)
            to = to if scale is None else to * scale
            t -= c - to
        if gate is not None:
            t -= max(0, s['gate'][0] - gate)
        n += t <= 5000
    return n


def main(path):
    rows = [json.loads(l) for l in open(path)]
    pushes = next(r for r in rows if r.get('step') == 'burst timing')['pushes']
    ms = [(p, marks(p)) for p in pushes]
    met = sum(1 for p, s in ms if p['res'].startswith('full') or (p['res'] == 'ok' and s['total'][0] <= 5000))
    late = [s for p, s in ms if p['res'] == 'ok' and s['total'][0] > 5000]
    print(f'{len(pushes)} burst pushes: {met} met, {len(late)} late ok, need {int(0.9 * len(pushes))}')
    print('commit cap ms   alone   with stage first')
    for c in (1000, 1500, 1750, 2000, 2250, 2500, None):
        print(f'{str(c or "none"):>12} {met + moved(late, cap=c):7} {met + moved(late, cap=c, stage=True):10}')
    print('commit scale    alone   with stage first')
    for x in (0.9, 0.8, 0.75, 0.7, 0.6, 0.5):
        print(f'{x:12} {met + moved(late, scale=x):7} {met + moved(late, scale=x, stage=True):10}')
    for g in (300, 600):
        print(f'gate cap {g} ms alone: {met + moved(late, gate=g)}')
    print('stage first alone, validate +0 / +170 / +300 ms:',
          ' / '.join(str(met + moved(late, stage=True, vadd=v)) for v in (0, 170, 300)))


if __name__ == '__main__':
    main(sys.argv[1])
