# fanout

Concurrent validate inside one git shard, and `fanout` (BUILD-PLAN AM1-WP1, "Not yet").

- `bench.py IMAGE [--cpus N] [--mode validate|push]`: validates per second for one network in the shard container at
  1, 2, 4 and 8 concurrent pushes. `run-bench.sh` runs the before (`aa9e097`) and after images as an orb service and
  writes `raw/bench.jsonl`.
- `plant.py`: the plants for this branch's new code; each must turn a test red.
- `workingset.py FROM TO [GIT_BYTES]`: the daily working set (`decisions/OPEN-QUESTIONS.md`, G1) from each running
  shard's daily `shard stats` line in Workers Logs: networks and bytes held, and fetched, pushed, caught up, and used
  in the line's day, per site and in all. Reads logs only; it starts nothing.

## Clocks

Stamp every harness time on Cloudflare's clock, never the orb's. The orb's clock is not synchronised: it ran 7.3 to
7.9 s behind Cloudflare's on 2026-11-18, which made a trace window 7 s early (`tests/am1/results/health-gate-lease.md`,
"The rule saw what the trace shows") and the constant 7 s offset seen in B0-34's timings. Members' trace rows, Workers Logs, and the
`Date` header are on Cloudflare's clock.

- `srvclock.mjs` measures the offset from `Date` headers to within about ±30 ms (`node tests/fanout/srvclock.mjs`;
  test: `node --test tests/fanout/srvclock.test.mjs`). `serverClock()` keeps it fresh for a long-running tracer.
- `terms.mjs` and `tests/nr3/tsave.mjs` stamp `at` with it, and their first line logs the measured offset (`clock`).
- `rows.sh`, `costrow.sh` and `b034.sh` source `srvclock.sh` and stamp status lines with `stamp`. A line that ends
  "(orb clock)" was stamped before the offset could be measured.
- Files written before 2026-11-18's fix are on the orb's clock. Shift them by the offset measured for that run (m50
  +7,310 ms, m51 +7,440 ms), or by the newest trace row's lead over `at` in the run's tsave file.

## B0-33 runs with `arrival=mem` (from 2026-11-18, write-cuts)

`rows.sh` sets test-faults `/stats?arrival=mem` on every voter before B0-33's load (`arrival.mjs`), and skips the
row if any voter does not confirm it. `b033.sh` passes `mem` to `memguard.mjs`, which sends it with every sample, so a
voter that reset is back on it within one sample, and each sample records `arrival`. With `sql`, every `/commit` was a
bare `exec` of its own, outside the entry's transaction, 31 to 44 per leader confirm on m52
(`tests/am1/results/tx-mix.md`, cut 1). B0-33 rows m50 to m52 ran with `sql`, so their confirm times are not
comparable with later rows. Every B0-33 results file states which mode it ran.

## Client colos (from 2026-11-18, B0-33 m53)

`b033.sh` and `b029.sh` end each row's status with its voters' `suspect` reports by edge colo (`colos.py` on the
row's tsave). Record the colos; do not pin them: production clients reach many colos. SUSPECT withholds a promise
only on reports from two colos, so a row whose reports all came from one colo never exercised the withhold, and its
leader-change count says nothing about the leader's health. B0-33 m50 to m52 (and m45) were LAX-only; m53 was the first
with two (LAX and DFW), so leader-change counts before m53 are not comparable with it
(`tests/am1/results/b033-m53.md`). Every B0-33 and B0-29 results file states the row's colos.

## Placement baseline (from 2026-11-19, B0-29's A-B-A)

Every B0-29 and B0-33 results file states the leader's 1-statement storage confirm p50 in the row's setup, before
load, beside its load confirm. The pooled setup p50 is not the baseline: setup confirms are two near-equal groups,
1-statement deferred flushes and unpaired in-step confirms, and the pooled median flips between them on one sample
(`tests/am1/results/b033-m54.md`, "4. Setup baseline"). `b029.sh` and `b033.sh` end each row's status with it (`lsrefsab.py` on the row's tsave). Each row
is a new network on new Durable Objects, and on one build the setup p50 ranged from 39 to 70 ms by placement alone
(`tests/am1/results/b029-m2b.md`, the A-B-A). A load confirm is read against its own network's baseline, not
against another row's.

For F91's within-network control, run `b029.sh` with `A1_EXTRA="--ls-refs-shard-every 60"` and `LS_EVERY=60`. a1.mjs
then sends `x-graft-ls-refs: shard` only in odd minutes of Cloudflare's clock, so one network carries both arms. tsave
reads every member's `/isolate` (`ISOLATES=1`). `lsrefsab.py TSAVE 60` pairs each shard minute with the edge minute
before it and names any members that shared an isolate.

## When the orb cannot reach a workers.dev host

Seen on 2026-11-18 from about 13:20Z (B0-02) and at 14:33Z (B0-33 m52): the orb's resolver answered SERVFAIL for
`graft-m2b-edge.<sub>.workers.dev` ("No Reachable Authority at delegation workers.dev"), and the egress proxy answered
`CONNECT` with 502. Node's `fetch` then fails with `fetch failed` / `ENETUNREACH` (it tries the IPv6 address), and a
driver such as `mkrepos.mjs` creates nothing: `rows.sh` logs "no repo". Check with
`curl -sv -o /dev/null "$GRAFT_EDGE/" 2>&1 | grep -E 'CONNECT|HTTP/'` and `resolvectl query -4 <host>`.

1. Bypass the proxy for workers.dev, in the shell and in every orb service that runs a driver:
   `export NO_PROXY="$NO_PROXY,.workers.dev" no_proxy="$NO_PROXY,.workers.dev"` after sourcing the env file.
   `api.cloudflare.com` and R2 still go through the proxy and keep working.
2. If the resolver still fails, pin the hosts to an address Cloudflare's DNS-over-HTTPS returns, in the orb's own
   `/etc/hosts` only:
   ```
   sudo cp /etc/hosts /tmp/hosts.orig
   ip=$(curl -s -H 'accept: application/dns-json' "https://cloudflare-dns.com/dns-query?name=$(echo $GRAFT_EDGE | cut -d/ -f3)&type=A" | jq -r '.Answer[0].data')
   printf '%s %s\n%s %s\n' "$ip" "$(echo $GRAFT_EDGE | cut -d/ -f3)" "$ip" "$(echo $GRAFT_TOOLS_EDGE | cut -d/ -f3)" | sudo tee -a /etc/hosts
   ```
   DoH itself answered SERVFAIL about one time in three; retry until it gives an address. Restore `/etc/hosts` from
   `/tmp/hosts.orig` when the run's close-out is done.
3. Never commit the pin or the address, and never put either in a driver: the address is Cloudflare's and can change,
   and the pin hides a real resolution failure from the next run. Note in the run's results that the workaround was
   used, as `tests/am1/results/b033-m52.md` does.

## Hosted slot rules (operator, 2026-11-16 and 2026-11-17)

- The only meter is `acctcost.py`, account-wide; read it before and after every hosted step.
- One hosted run at a time across the account, in a slot the operator grants, with an estimate and a stop sent first.
- Every run ends with:
  1. `teardown.py --sweep` (no shard container left running);
  2. `/retire` of every finished test group (`idleretire.mjs`);
  3. **the retired groups' bucket objects deleted** (`purge.py`, which `idleretire.mjs` runs once every member it
     retired answers "not joined"; it checks the group itself and refuses a network with any member joined). A
     retired group's packs left in the buckets make every `*/15` sweep ask its dead group and alarm each pack
     (`decisions/OPEN-QUESTIONS.md`, R25). Only networks the run itself created are purged; the sweeper never
     deletes a group's packs on its own;
  4. `joined.mjs` over every repository the run created, including ones a driver created and then skipped, ending
     "none joined". A test repository left joined is refolded by the site's fold audit once a day (`fold_audit.rs`,
     4 chains each `*/15` run): on `graft-m3f` 40 leftovers from 2026-11-13 to 11-16 wrote 501,211 rows in one
     50-minute stretch of 2026-11-18 (`decisions/OPEN-QUESTIONS.md`, K10). Retiring does not delete a chain's registry
     rows, so the audit's `homed` count stays the same; the joined count is the check. Which tool applies depends on
     the build:
     - a test-faults build (graft-m2b, graft-m3f): `idleretire.mjs` or `retirenets.mjs`,
       then `purge.py`. Both call the test route `/retire`, which a product build does not serve;
     - a product build (graft-m4a, and every customer install): delete the repository through the API, then
       `POST /api/graftyard/v1/admin/repos/{id}/purge` (`workers/edge/src/purge.rs`) as a site admin, with
       `{"early": true}` before `retain_until`, and the second call once `secondPassAt` has passed. It refuses a
       repository that is not tombstoned, so a product install's test repositories are deleted first;
  5. an `acctcost.py` reading 30 minutes later, spanning two `*/15` cron runs, showing the install idle (under $0.10 an
     hour).
- Anything that writes Durable Object rows in a loop is a bug to note and fix.
