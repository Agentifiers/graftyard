# Working-set residency of graft-m1 network 1048603 against R2

One synthetic network's warm net shards, read from their daily `shard stats` lines (`ShardHost::log_stats`,
`crates/git/src/host.rs`) through `tests/fanout/workingset.py`, against a read-only listing of `pack/1048603/` in
`graft-m1-packs-a`, `-b` and `-c` (R2 API, GET) at 2026-11-21T20:31Z, before the network's purge. No hosted work ran
for this reading. Open question G1 (`decisions/OPEN-QUESTIONS.md`) is what it informs; it does not answer G1.

- **Network:** 1048603 (`m3org/b034-mono-a1`), the monorepo of a B0-34 row held for this reading by the B0-30/B0-34
  thread. The row's load ended 2026-11-20T18:44:23Z; the harness's last client read was about 18:55Z. One catch-up
  (2 entries, not behind) reached each net shard at about 20:57Z by mistake (mt11's first try); nothing else loaded it.
- **Shards:** git#0 (site a) restarted at 18:34:42Z by the row's home-shard kill; git#1 (b) and git#2 (c) started
  18:21:30Z. Pool shards that also hold 1048603 (`git#0#pool:5`, `git#1#pool:5`) are not counted.

These are mechanics only: a test install's traffic is synthetic, and one network fetched whole in a row's last
15 minutes says nothing about how much of a real fleet's git bytes are hot in a day.

## Lines

Each shard's second line came 24.0 to 24.4 h after its first. Times are 2026-11-20 unless marked 11-21.

| Site, shard | First line | Second line | Held (B) | pack_bytes (B) | Day fetched / pushed / caught up / used |
| --- | --- | --- | --- | --- | --- |
| a, git#0 | 17:02:04Z, 41,329,414 B | 11-21 17:04:34Z | 7,321,687,608 | 6,739,945,031 | 7.32 / 7.32 / 7.32 / 7.32 GB |
| b, git#1 | 18:21:48Z, 1,425,345,923 B | 11-21 18:42:54Z | 7,418,282,890 | 6,745,082,966 | 7.42 / 0 / 7.42 / 7.42 GB |
| c, git#2 | 18:21:49Z, 1,820,606,739 B | 11-21 18:32:10Z | 7,228,129,199 | 6,742,349,759 | 7.23 / 7.23 / 7.23 / 7.23 GB |

- Each line holds one network. Disk is 20,000,002,048 B on all three, so held is 36 to 37% of the disk.
- The whole network was fetched in the row's last 15 minutes: fetched at 18:44:37Z (a, after its 18:34Z restart),
  18:42:55Z (b) and 18:32:31Z (c), and caught up at 18:44:57Z, 18:45:09Z and 18:45:09Z. So the day's working set on
  each shard is the full network, held once.
- b's day shows no push: its last push, 18:42:25Z, is before its first line.
- git#0's day began at its 18:34Z restart. Every day includes the row's own 23 minutes of load and the 20:57Z catch-up.

## Against R2

| Bucket | .pack files | .pack bytes | .idx files | .idx bytes | Newest object |
| --- | --- | --- | --- | --- | --- |
| a | 392 | 8,671,128,610 | 61 | 275,199,300 | 2026-11-20T18:45:02Z |
| b | 392 | 8,671,128,610 | 62 | 280,185,912 | 2026-11-20T22:26:40Z |
| c | 392 | 8,671,128,610 | 61 | 279,995,084 | 2026-11-20T22:26:46Z |

| Site | pack_bytes ÷ bucket .pack bytes | held ÷ bucket .pack bytes | disk held ÷ pack_bytes |
| --- | --- | --- | --- |
| a | 0.777 | 0.844 | 1.086 |
| b | 0.778 | 0.856 | 1.100 |
| c | 0.778 | 0.834 | 1.072 |

- The three buckets hold the same packs, so each ratio is the same against any bucket.
- A warm shard holding one fully fetched network keeps about 0.78 of R2's pack bytes, plus about 8% disk over its
  pack bytes (1.07 to 1.10), close to the 1.066 the pool shards showed on 2026-11-20.
- The line carries no pack count, so the comparison is in bytes only.

## Not explained

- R2 lost about 320 packs and 206 MB a bucket after the 2026-11-20T19:20Z baseline (712 to 718 packs, 8,861,948,328
  to 8,877,521,998 B then), and b and c each have an object written at 22:26Z. Not traced; compaction or GC is a guess.
- `used` moved on 11-21 with no load on the network: a at 17:04:05Z, 29 s before its line, and b at 18:03:43Z. c's
  `used` stays at 18:45:09Z on 11-20. Not traced; the alarm path is unverified.
