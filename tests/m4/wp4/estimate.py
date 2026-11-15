#!/usr/bin/env python3
"""M4-WP4's hosted cost estimate: BF-60 and BM-4 on Sandboxes, and BF-54's 2,500 held polls with unmodified
`actions/runner` listeners. Prices from research/prices.json at overage rates, allowances ignored, as the earlier
estimates do (tests/m4/results/m4-wp2.md). Every input below is labelled; run from the worktree root:

    python3 tests/m4/wp4/estimate.py
"""
import json, math, os

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
P = {e['id']: e['usd'] for e in json.load(open(os.path.join(ROOT, 'research/prices.json')))}
MEM, CPU, DISK = P['cf.containers.memory'], P['cf.containers.vcpu'], P['cf.containers.disk']
# Instance types [doc: developers.cloudflare.com/containers/platform/limits, 2026-11-12].
TYPES = {'standard-3': (2, 8, 16), 'standard-4': (4, 12, 20)}


def instance_hour(kind, cpu_share):
    """One hour of `kind`: memory and disk provisioned, CPU at `cpu_share` of its vCPU active."""
    vcpu, gib, gb = TYPES[kind]
    return 3600 * (gib * MEM + gb * DISK + vcpu * cpu_share * CPU)


def row(name, kind, hours, lo=0.10, hi=1.0):
    a, b = hours * instance_hour(kind, lo), hours * instance_hour(kind, hi)
    print(f'{name:<44} {hours:6.1f} {kind}-h  ${a:6.2f} to ${b:6.2f}')
    return a, b


print(f'{"per hour":<44} standard-3 ${instance_hour("standard-3", 0.1):.3f} (10% CPU) to '
      f'${instance_hour("standard-3", 1):.3f}; standard-4 ${instance_hour("standard-4", 0.25):.3f} (25%) to '
      f'${instance_hour("standard-4", 1):.3f}')
total = [0.0, 0.0]


def add(x):
    total[0] += x[0]
    total[1] += x[1]


# ---- BF-60: 50 `ubuntu-latest` jobs of `sleep` 1 to 8 min (mean 4.5) under the cap of 20.
# Each Sandbox: 1.5 min to start, configure, and exit [guess], so 50 x 6 min; plus at most 10 spare runners that two
# overlapping passes start near the end of the queue, each living up to the next sweep's stale delete, 10 min [guess
# from tests/m4/results/m4-wp4.md: the in-memory run had none past the last job].
bf60_h = (50 * (4.5 + 1.5) + 10 * 10) / 60
add(row('BF-60 (50 jobs, cap 20)', 'standard-3', bf60_h))
# ---- BM-4: a 2 x 4 matrix of 5-minute jobs, a site failed mid-run, so each job waits out the failover [guess 5 min].
bm4_h = 8 * (5 + 1.5 + 5) / 60
add(row('BM-4 (8-job matrix, a site failed)', 'standard-3', bm4_h))

# ---- BF-54: 2,500 listeners on one broker scope, 85 MB idle and 95 MB more while a job's worker runs [measured,
# lead]. standard-4 is the largest instance (12 GiB). Per instance: 512 MiB for the Sandbox server and the OS [guess],
# then listeners, then room for workers.
N, IDLE, WORKER, BASE = 2_500, 85, 95, 512
mib = TYPES['standard-4'][1] * 1024
per = 100
instances = math.ceil(N / per)
room = mib - BASE - per * IDLE
workers = room // WORKER
print(f'BF-54: {per} listeners per standard-4 ({per * IDLE + BASE} of {mib} MiB), {instances} instances, '
      f'{instances * TYPES["standard-4"][0]} vCPU; {workers} concurrent jobs per instance '
      f'({instances * workers} in all) before memory runs out')
# Run length: image pull and start 3 min [guess]; 2,500 registrations plus 2,500 session opens on one Actions shard,
# paced at 10 a second [guess under the measured 14.5 to 25 acked writes/s of one group, tests/m1/results/m1-wp3.md
# and tests/m0/results/m0-wp12.md], 8.3 min; hold 10 min for the 503 and re-poll half; 10 min of queued jobs; 5 min of
# teardown. A rerun margin makes the budget an hour.
reg_min = 2 * N / 10 / 60
run_min = 3 + reg_min + 10 + 10 + 5
print(f'BF-54 run: {run_min:.0f} min ({reg_min:.1f} min of registration and sessions at 10/s); budgeted 60 min')
add(row(f'BF-54 ({instances} x standard-4, 1 h)', 'standard-4', instances * 1.0, lo=0.25))

# ---- The install and the runner account. A site day and an edge day put M4-WP4's routes on graft-m4a; M4-WP3's
# window with one site day and two edge days measured $0.53 (tests/m4/results/m4-wp3.md, "Cost"). BM-4's site kill and
# return are two more site deploys. BF-54's polls: 2,500 runners each holding a 50 s poll, plus the 500 past the cap
# re-polling every 15 to 30 s, for an hour: about 0.25 M front, edge, and broker requests each.
install = 3 * 0.53 + (3 * 0.25) * (P['cf.workers.requests'] + P['cf.do.requests'])
subscription = P['cf.workers.paid.subscription']
print(f'{"install: 3 site or edge days, BF-54 requests":<44} ${install:6.2f}')
print(f'{"runner account: Workers Paid, one month":<44} ${subscription:6.2f}')
lo, hi = total[0] + install + subscription, total[1] + install + subscription
print(f'{"total":<44} ${lo:6.2f} to ${hi:6.2f}')
