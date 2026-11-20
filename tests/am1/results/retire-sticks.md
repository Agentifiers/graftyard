# retire-sticks: graft-m1's revived retired networks, 2026-11-20

Read-only investigation of graft-m1 (Workers Logs, the Containers API, and LIST/GET on the R2 buckets), nothing
deployed. Decision record: `decisions/DECISIONS.md`, "`retire-sticks`". Times are UTC on 2026-11-20.

## 1048577 (`m3org/lf1-0`)

| Time | What | Source |
| --- | --- | --- |
| 07:09:25, 07:10:48 | B0-34 retires the group, then purges `graft-m1-packs-{a,b,c}` (14 objects; second pass 0) | B0-34 thread |
| 08:15:17 | Site a's `*/15` sweep starts; it logs `network 1048577: fills 3, done 3` | site a, `scheduled` |
| 08:15:37–39 | `pack/1048577/01M724JD….pack` (32 B, an empty PACK v2) and its `committed/` marker `{"chain":"repo:1048577","index":504,"bytes":32}` written to a, b and c. They are the only changes in all three buckets in 08:14–08:17 | R2 listing |
| (source) | The same pack and marker in `graft-m1-packs-d`, written 2026-11-10 07:18:19, identical ETag. purge.py's `SITES = "abc"` never listed d | R2 listing |
| 10:41:39 | One `POST /m3org/lf1-0.git/git-upload-pack` (trace `204ab325`, LAX, 19.4 s). Member `/read`s answer not joined; registry `GET /row` on all three sites; `ensure_group` → `graftyard_lookup::create` | edge, sites |
| 10:41:48 | `POST /claim` (still generation 1), `PUT /row`, `POST /join` on each bootstrap name: "fold tables made again". The group is back at generation 1, empty | sites |
| 10:43:05, 10:51:02, 11:38:01, 13:51:13 | Site a's archive alarm promotes range (1, N). Each try posts `/promote?warm=start` to every site's shard, starting all three containers; the catch-up probe reads entry 504 of a 4- to 10-entry log and fails "entry 504 of a marker was not answered" | site a, `promotion Some((1, …))` |
| 10:47, 10:59, 13:51, 14:04 | Shards torn down by B0-34, then running again after the next try | edge `/_bed/kill`, Containers API |

## 3473415 (mt9's `b034-second-z`)

| Time | What | Source |
| --- | --- | --- |
| 13:19, 13:26:58 | Retired, then purged from a, b and c | orchestrator |
| 13:45:17 | Site a's sweep starts; logs `network 3473415, kinds {fills: 15}, done 3, failed 12`; in the same run 3473414 done 7 and 3538957 done 6 | site a, `scheduled` at 13:49:39 |
| 13:47:57–13:48:01 | 6 `pack/` (`.pack`, `.idx`) and 3 `committed/pack/` objects in a and b, the same keys and sizes as bucket d's, written there 10:48:08–13 | R2 listing |
| after 13:20 | Its members see only registry `GET /row`s: no `/join`, catch-up or promotion through 14:40 | sites |

The sweep's fill does not revive a group. It leaves a committed marker and pack that a later revive, by any read
through create on first use, then reads as its own (1048577's entry 504). The objects are left in R2 as evidence.

## Pool shards (1048602, 3538965, 3538966)

The `shard stats` lines that show them were logged from 12:29 to 12:40 on `git#{0,1}#pool:{1,2,3,4}` (1048602 at
3.3 to 4.2 GB, 3538965 at 0.28 GB), and each line's own `used` and `caught_up` fall within minutes of it: the
networks were in use then, not 24 h after a purge. Workers Logs show no `/retire` to any of the three from 11:00 to
14:25. Each pool slot was torn down by the ledger's heartbeat at 12:41 to 12:47 ("pool
slot N on site S torn down: no lease holds it"). Every `#pool:` instance on all three sites has been `failed`
(exit 137, stopped) since then, and no pool shard logged after 12:48. A container's disk is ephemeral: the next start
uses a fresh disk from the image (`research/cloudflare.md`, Containers). So the bytes were held only while the
container ran, and no disk is billed or kept after the teardown. Eviction on a running shard is the LRU in
`make_room` (`graftyard_git::evict_plan`): it runs before an ingest and frees space only past `DISK_USE_PCT`. A
retired network on a running pool shard therefore stays until the shard stops or needs the room. That is lazy by
design, and the teardown is its trigger: a lease lapses `LOAD_STALE` (180 s) after its last renewal, and the next
`LEDGER_BEAT` (45 s) tears the slot down. No change was made.
