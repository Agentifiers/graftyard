"""Merging a member's `Stats::trace` rows from several saves (`tests/nr3/tsave.mjs` snapshots, an end-of-run dump).

`/trace` returns each ring entry once, so two rows equal in every column inside one save are two events: two
read-index asks from one learner in one millisecond, whose batch numbers above 2^53 the save's JSON rounded alike;
two `suspect` reports with one verdict; two Work rows of one turn. A set of tuples keeps one of them. On mrB's leader
it kept 4,898 of 9,770 `IndexTaken` rows (`tests/am1/results/trace-dedup.md`). So a row is kept as many times as the
save that holds it most. tsave's saves never overlap (each asks `since` the last `at` it kept), and a dump that repeats
them adds nothing. Columns become Python ints, so no value is rounded again here; the capture's rounding of batch
numbers above 2^53 is in the raws and cannot be undone.
"""
import collections
import json
import lzma


def merge(saves):
    """`saves`: (member, rows) pairs in save order. Returns {member: [row tuple, ...]}, each member's rows in save
    order and, within a save, ring order."""
    out, kept = collections.defaultdict(list), collections.defaultdict(collections.Counter)
    for who, rows in saves:
        here, k = collections.Counter(), kept[who]
        for r in rows:
            t = tuple(int(x) for x in r)
            here[t] += 1
            if here[t] > k[t]:
                k[t] += 1
                out[who].append(t)
    return out


def saves(path, learners=False):
    """(member, rows) from a tsave file (`.jsonl` or `.jsonl.xz`): voters by site number when `learners` is False,
    learners by address when True, both when None (learners as ('l', address))."""
    with (lzma.open if path.endswith('.xz') else open)(path, 'rt') as f:
        for line in f:
            try:
                d = json.loads(line)
            except ValueError:
                continue
            if 'rows' not in d:
                continue
            if 'learner' in d:
                if learners is False:
                    continue
                yield (d['learner'] if learners else ('l', d['learner'])), d['rows']
            elif learners is not True:
                yield d.get('site'), d['rows']


def load(path, learners=False, keep=None):
    """`merge(saves(path, learners))`, keeping only rows for which `keep(row)` holds when given."""
    return merge((w, [r for r in rs if keep(r)] if keep else rs) for w, rs in saves(path, learners))


if __name__ == '__main__':
    # The rule on known answers: equal rows inside one save all stay, a save that repeats an earlier one adds nothing,
    # a later save with more copies adds only the extra ones, and a value above 2^53 keeps every digit.
    a, b = [1, 5, 192, 2 ** 62 + 1, 0, 0], [2, 28, 0, 3, 4, 4]
    m = merge([(0, [a, a, b]), (0, [a, b]), (1, [a]), (0, [b, b, b])])
    assert m[0] == [tuple(a), tuple(a), tuple(b), tuple(b), tuple(b)], m[0]
    assert m[1] == [tuple(a)], m[1]
    assert m[0][0][3] == 2 ** 62 + 1
    print('ok')
