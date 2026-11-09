# wire-fix: pinned wire tags, and no member reply that grows with a repository's ref count (bf20)

These results are from 2026-11-09, on branch `wire-fix` (worktree `.worktrees/wire-fix`), cut from `main` at `f3fa02b`. `origin/main` `debd398` (docs only) was merged in as `576d876`. Two test installs were used: `graft-m2b` for bf20, and `graft-m1` for the lease groups. Nothing was pushed or merged. The raw logs are in `tests/m3/raw/wirefix/` and `tests/am1/raw/wf-*`; both directories are git-ignored.

## Summary

| Problem | Root cause | Fix | Before | After |
| --- | --- | --- | --- | --- |
| A variant inserted mid-enum renumbered stored events (32.13's `Created`/`Deleted` moved the lease events from 33–36 to 35–38) | Postcard writes a variant as its position in the declaration, and nothing pinned the positions | `crates/git/tests/wire_tags.rs` + `wire_tags.golden` pin the tag, name and sample bytes of 655 variants in 120 enums, built from unchanged `f3fa02b` code | No check | Swapping two variants fails the test, and inserting one fails it (or fails to compile on exhaustive matches). The existing `agent_bytes.golden` and `encoding` tests pass unchanged |
| bf20 (`repo:65536`) leaderless since 7 Nov 14:31Z, with leaders dying in `handle_alloc_error` | A shard catch-up read the leader's whole ref set (`/read {"q":{}}`, 571,872 refs, about 43 MB of JSON) in one reply | Catch-up applies each entry's ref changes; rehydrate and the edge read refs in pages of at most 10,000; `{"q":{}}` past one page answers `{"paged":10000}` | 87 `handle_alloc_error` and 307 "exceeded its memory" on sites a and b over 50 h; the old reply at bf20's size is 43,095,634 bytes | 0 and 0 on all three sites from site day 1 (10:00Z) through 11:50Z, across five cron wakes; every page under 1 MB in the regression test; one leader (site a, term 24) at all 58 one-minute checks from 10:50Z to 11:50Z |
| Two catch-ups of one repository at once | A merge waited for a catch-up in flight, but a second catch-up did not; each rehydrated from nothing | `CatchUps::turn`: catch-ups on one network take turns, then resume from the saved cursor | The roll's catch-up of bf20 and prsync's warm ran together: 762 `/entries` calls for 418 pages, one hung until the runtime ended it at 884 s | One at a time (unit test); site day 2's roll of bf20 on shard 1 answered 200 |
| A member's `/entries` page read every row first | `entries_json` collected up to 2,000 rows (bf20's are 23.5 KB each: about 47 MB) before stopping at the 600 KB page | Read rows one at a time and stop at the page's end | About 160 ms of leader CPU per page during a rehydrate (median of 762 calls) | 10 ms median, 19 ms p90 (238 calls on site a during site day 2's catch-ups) |

## Part 1: pinned wire tags

### What pins them

`crates/git/tests/wire_tags.rs` builds one value of every variant of every enum whose variant index is stored or sent between members, using a sample deserializer that picks the outer variant. It checks four things for each variant:

- postcard's first varint is the variant's position, the pinned tag;
- the value round-trips to the same bytes;
- the name is the pinned name;
- the sample's bytes match `wire_tags.golden`.

The table covers 120 enums and 655 variants:

- `RepoEvent`, `RepoCmd`, `IssueOp<BlobRef>`;
- the Authz, Identity, Names and Tokens events and ops, and `IdsEvent`;
- raft's `EntryBody`, `Message`, `Deny` and `GroupConfig`;
- every enum nested inside those.

The golden file was generated from `f3fa02b`'s code, before any change on this branch, so it records the bytes main writes today. Regenerate it with `GRAFTYARD_PIN=1 cargo test -p graftyard-git --test wire_tags -- --nocapture`; a diff in it is a stored-format change. The test register's `RegEvent` (test installs only) is pinned the same way in `workers/site/src/register.rs` (`--features test-faults`). Each top-level enum's doc comment says "append only".

No derive or serde attribute changed, so no byte changed. `serde-reflection` was tried and dropped, because it cannot build values of the validated text types.

Proof that it catches the mistake:

- Swapping `RepoEvent::LeaseRelease` and `PathSet` fails the test. The message reads: "git::repo::RepoEvent: tag 37 was `37 LeaseRelease 25000002` and is now `37 PathSet 2500…`: a variant was inserted, removed, reordered, or renamed, or a field changed. Stored rows and other releases decode by these tags; put a new variant at the end of its enum". It reports tag 38 the same way.
- Inserting a variant mid-enum fails to compile first, on the exhaustive matches in `repo.rs`. With those matched, the test fails on every later variant's tag.
- `agent_bytes.golden` and the `encoding` tests pass unchanged.

`RepoEvent` keeps 32.13's `Created`/`Deleted` at 33–34 and the lease events at 35–38, as decided.

### The graft-m1 lease groups (j4, lf1-2, lf1-3)

Before this branch's site day, every member of each of these groups failed to restore: j4 at log row 3838 on sites 0–2, lf1-2 at row 26, lf1-3 at row 34 on site 0 and row 32 on sites 1–2, each with "Serde Deserialization Error". Their site-3 objects never joined.

**Path used: recreate.** For each group, every member was cleared with the test route `/retire` (BF-11's lost-member stub: the member's storage is deleted and the object resets). Then one `git ls-remote` through `graft-m1-edge` found no group and created it on first use: `ensure_group` → `graftyard_lookup::create` joins the same generation-1 names with the repository's owner, and the home campaigns. The repository's Authz row, `RepoOwner` row, and its shard's packs were not touched; only the chain's log and fold start empty. This was the least invasive path that exists:

- No operator route repairs or rewrites a stored log row.
- A reset alone re-reads the same rows and fails the same way.
- A `ReplaceMember` needs a live group to commit the config change.

Driver: `tests/m3/wirefix/recreate.mjs`; log in `tests/am1/raw/wf-recreate.log`. A first run, `wf-recreate-1.log`, stopped on a reply-printing bug after retiring j4's site-0 member.

| Group | Before | After `/retire` + create | ls-remote / clone |
| --- | --- | --- | --- |
| j4 `repo:1048576` | sites 0–2 "log row 3838: Serde Deserialization Error" | leader site 0, term 2, commit 2 | ok (18.0 s, empty) / ok |
| lf1-2 `repo:1048579` | sites 0–2 "log row 26" | leader site 0, term 2 | ok (16.3 s) / ok |
| lf1-3 `repo:1048580` | site 0 "log row 34", sites 1–2 "row 32" | leader site 0, term 2 | ok (18.7 s) / ok |

Then `tests/m3/wirefix/lease-check.mjs` (`tests/am1/raw/wf-lease-check.log`) ran the lease path on each group through the edge with the m3-ci App's installation token:

- task 201, lease acquire 201, list 200 (1 lease), release 204;
- each log now holds `config, noop, noop, noop, TaskCreated, LeaseAcquire, LeaseRelease`.

After `/reset` on all nine members, every one restored from its own rows with no `restoreError`, so lease events stored at tags 35 and 37 read back at the same tags. The groups are empty: LM-1's j4 rows (L0-12's refusal counts, B0-36, B0-37) and L0-16's lf1-2 need their seed data again before they rerun, and that was not part of this task.

## Part 2: bf20

### What bf20 is

- `repo:65536`, `m2b/bf20`, holds 571,872 refs, counted on 2026-11-09 from a follower's `/fold` refold of the core tier; that tier holds 571,889 rows.
- The names are `refs/heads/bf20/c300{a,s,t,z}N/k`, plus `f48wake` and `main`.
- Its log has 7,310 entries. Entries 1–16 carry about 300 ref changes each, and entries from about 3,000 on are batches of `Comment` events of about 23.5 KB each.
- The bench ref oids name objects the shard does not hold, because `bench_load` made them up. Only `main` (`03cc126`) names a real commit.

### Root cause of the crash

`prsync::warm` runs on a new leader's alarm and calls the git shard's `/catchup`. The catch-up's `ingest::heads` called `ChainLog::core_refs`, which sent the leader `/read {"q":{}}`. `Repo::read_json` answered with every ref in one JSON value. At bf20's size, `serde_json::to_vec` of that reply ran the leader's isolate out of memory (`handle_alloc_error` under `Response::from_json`), which reset every member in the isolate. The group had no leader again, the next winner warmed again, and the cycle repeated.

The edge's info/refs read, `read_refs`, sent the same `{"q":{}}`. This is the same class of bug as M2-WP1's defect (c), which fixed only the `create` probe.

Measured before the fix, over 50 h of Workers Logs:

| site | `handle_alloc_error` | "exceeded its memory" | `/catchup?repo=65536` calls |
| --- | --- | --- | --- |
| a (enam) | 63 | 216 | 60 |
| b (weur) | 24 | 91 | 62 |
| c (apac) | 0 | 0 | 0 |

In the 3 h just before the site day: site a had 1 and 5, site b 1 and 1, site c 0 and 0. All three members were followers at term 23/24, and commit was stuck at 7300–7302.

### The fix (`cff5057`, `be085bf`, `3fc2397`, `6eb3433`)

- **Paged ref reads.** `RepoFold::refs_page(prefix, after, limit)` and `group::refs_read`:
  - `{"q":{"refs":{prefix,after,limit}}}` answers one page of at most `REFS_PAGE_MAX` (10,000) refs, plus `next`.
  - `{"q":{}}` answers every ref only while they fit one page. Past that it answers `{"paged":10000}`, so no older caller can ask for the whole set.
- **Catch-up from log entries.** `/entries` now carries each entry's net ref changes (`RepoEvent::ref_changes`, including `TestMerged`'s `refs/pull/n/merge`). `ShardHost::catch_up` applies them with `POST /refs` and never reads the ref set. An entry without `refs` (an older member release) or a cursor below the log's first index means a rehydrate.
- **Paged rehydrate.** `rehydrate` sends the refs page by page with `PUT /refs` (`start=1` first, `done=1` last), then catches up from the first page's index.
- **Container `/refs`.** It uses `update-ref --stdin --batch-updates`, so a ref naming a missing object is skipped and counted rather than failing the set. bf20's made-up oids are skipped this way.
- **Edge info/refs paged.** The receive-pack advertisement streams page by page (`smart::receive_pack_head`/`lines`/`tail`), with bytes identical to the old one (tested). The clone cache's tag digest reads only `refs/tags/`.
- **`/entries` streams its rows** (`be085bf`). `entries_json` collected up to 2,000 log rows before applying its 600 KB page bound. bf20's rows are 23.5 KB, so a call held about 47 MB and cost about 160 ms of CPU. It now reads one row at a time and stops at the bound.
- **Catch-ups take turns** (`3fc2397`, `6eb3433`). In site day 1, the roll's `/_bed/catchup` and prsync's warm rehydrated bf20 at once. That took 762 `/entries` calls for 418 distinct pages, and the later call hung until the runtime cancelled it at 884 s ("Promise will never complete"). `CatchUps::turn` makes a catch-up wait for the one in flight, then go on from its cursor. Unit test: `catch_ups_on_one_network_take_turns`, which fails if the gate does not wait (checked).
- **`--roll-shards` goes on past a slow catchup.** The curl limit is now 600 s, and a timeout reports `fail(28)` instead of aborting the roll. On site day 1 one 300 s timeout on bf20 aborted the roll and left later shards on the old image.

### Regression test

`crates/git/tests/refs_paging.rs` builds a core tier of 571,872 refs named like bf20's and drives the reads a catch-up, a rehydrate and info/refs make:

- every reply is at most 1 MB;
- `{"q":{}}` says it is paged;
- 58 pages reassemble the whole set;
- the streamed advertisement is byte-identical to the whole one.

With the paged guard removed, the old `{"q":{}}` reply is 43,095,634 bytes and the test fails (checked). `containers/git/server/refs_test.go` covers paged `PUT /refs` and the skipped refs.

### Why site c never leads

This is from the code and from Workers Logs; no member state was changed.

- Campaigns are request-driven (`host.rs`: "`tick` starts no campaign; campaigns start only from `/campaign` and `/suspect`").
  - `/campaign` goes only to the record's home: `lookup::create` posts it to `members.iter().find(|m| m.site == record.home)`, and bf20's home is site 0.
  - Over the 24 h before the site day, site a received 204 `/campaign` calls and sites b and c none.
- `/suspect` reaches all three (a 586, b 909, c 957 in those 24 h), and each report lets a follower whose promise has lapsed start a pre-vote. Site c's pre-votes still lose, for two reasons:
  - **Pre-vote's tie-break.** A voter that is itself campaigning denies `Contending` to a candidate with a higher site id unless the candidate's log is newer (`election.rs`, `req.candidate > self.cfg.me && !newer`). The edge's `suspect` and `not_leader` paths fan reports to every voter at once, so all three start pre-votes together. With equal logs, site 2 loses to sites 0 and 1, and site 1 loses to site 0.
  - **Distance.** Site c (apac) is the farthest from the others, so its vote requests arrive last. Votes count on a quorum of two, and a and b reach each other first.
- So the crash loop only ever chose site a or b as leader. Each of them died on its first warm, and site c, which never warms, never crashed.
- This is inference from the code and the call counts. No trace of one election's vote messages was taken.

### Hosted proof on graft-m2b

**Deploys.** Shard image `graft-m1-site-gitshard-git@sha256:b6ddeaf55a4259689dabb8d21e2ed24d411a390d697f1e010e1e43264a8c37bd` (`:wirefix`, built from `576d876`).

| Deploy | Commit | Versions |
| --- | --- | --- |
| Site day 1 (`siteday-m2b-1`) | `e0716f1` | a `8c340dee` / `4c4871c5`, b `cc155f39` / `8dd07400`, c `3ef674c9` / `87ec20cb` (two passes) |
| Edge (`edge-m2b-1`) | `e0716f1` | `graft-m2b-edge` `d560e7d4-7269-4f5f-a31b-27f5fdea088e` |
| Site day 2 (`siteday-m2b-2`) | `3fc2397` | `152dd387`, `9644d355`, `00ed3b79`, `ec56189d`, `82144379`, `b2ee8232` (a, b, c, two passes) |

Site day 1's roll stopped at bf20's catch-up, which took more than 300 s (see the catch-up fix above). A hand catch-up of bf20 on shard 0 then rehydrated 571,872 refs in 58 pages in 140.6 s, with `heads` at 331 ms and no crash. Site day 2 rolled all 68 running shard instances: every one answered `kill 200, catchup 200`, bf20 on shard 1 included.

**Leader.** After site day 1, site a led bf20 again at term 24, resuming its `ledTerm`. That was the first leader since 7 Nov 14:31Z. `tests/m3/wirefix/watch-bf20.sh` then ran a `git ls-remote` and read the three members' status every minute from 10:50Z:

- 58 of 58 checks (10:50Z to 11:50Z): site a leader, term 24, sites b and c followers naming 0;
- the term never moved;
- every ls-remote answered `03cc126d5e8b`, in 3.0–5.6 s.

**Cron wakes.** `*/15` fold-audit wakes since site day 2 (10:45Z), from Workers Logs `$workers.eventType = scheduled`:

| Wake (UTC) | site a | site b | site c |
| --- | --- | --- | --- |
| 10:45 | ok, cpu 164 ms | ok, 197 ms | ok, 195 ms |
| 11:00 | ok, 749 ms | ok, 136 ms | ok, 228 ms |
| 11:15 | ok, 148 ms | ok, 85 ms | ok, 226 ms |
| 11:30 | ok, 141 ms | ok, 126 ms | ok, 239 ms |
| 11:45 | ok, 228 ms | ok, 141 ms | ok, 236 ms |

Over site day 1 (10:00Z) through 11:50Z, there were 0 `handle_alloc_error` and 0 "exceeded its memory" lines on all three sites (`/tmp/wf/cron.py`-style queries of `$metadata.message`). The non-ok events were `canceled` Member alarm invocations (cpu 0–11 ms, a few per quarter hour on site a, before and after the site days alike; not examined further) and site day 1's two cancelled bf20 catch-ups described above.

**git.** `tests/m3/wirefix/git-bf20.sh` at 10:43Z, through `graft-m2b-edge`, as `m2b-writer`, with the token in git's environment:

| Request | Result | Time |
| --- | --- | --- |
| `ls-remote` v0 | rc 0, `HEAD` and `refs/heads/main` at `03cc126` | 4.8 s |
| `ls-remote` v2 | rc 0, the same two lines | 1.7 s |
| `ls-remote` v2 `refs/heads/main` | rc 0 | 1.5 s |
| receive-pack info/refs | 200, 41,954,822 bytes, 571,872 ref lines, streamed | 24.2 s (edge cpu 2,545 ms) |
| `clone --no-checkout` | rc 0, `HEAD` `03cc126`, 3 refs, 1 pack | 3.0 s |

The upload-pack side serves what the shard holds. That is `main` and the refs whose objects exist; the 571,870 bench refs name objects that were never pushed, and the shard skips them, so a clone has 3 refs. The receive-pack advertisement comes from the member's pages and lists all 571,872. Before the fix, neither worked: no leader answered.

## Part 1 hosted: graft-m1

Site day (`tests/am1/hosted/siteday.sh wf-siteday-m1 <image>` from `6eb3433`, sites a–d, `GRAFTYARD_AUDIT=1`, the same `:wirefix` image, `--roll-shards`), 10:45Z to 11:35Z:

| Script | Versions (two passes) |
| --- | --- |
| `graft-m1-site-a` … `-d` | `b0b5a359`, `519a42cf`, `6c6e2328`, `eea78bfe`, `78c73a4d`, `98a96704`, `c8f5b55e`, `066374aa` |
| `graft-m1-edge` | `363bbc00-3c8c-4c40-82c6-e7ded9fb9428` (`tests/am1/hosted/deploy-edge.sh wf-edge-m1`, the same FORMATS as before) |

The roll moved 103 running shard instances. 95 answered `catchup 200`. The other 8:

- six are the broken lease groups' shards (j4 on shards 0–3, lf1-2 and lf1-3 on shard 0), which answered 500 because their members did not restore;
- `196622` on shard 0 timed out at 600 s, `fail(28)`. It is a BF repository with 1,001 stored packs (about 1 GB); its catch-up made its `/entries` calls and then waited on the container;
- `196627` on shard 1 answered 500. Its site-1 member had just restarted and had no leader yet.

The roll went on past each of these, which is the `deploy.mjs` change. The lease groups were then recovered as above.

## Checks

At `41b918b`, as `/tmp/am1fix/checks.sh` runs them (all exit 0):

- `cargo fmt --all --check`;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- wasm32 clippy with `-D warnings` for `workers/site` and `workers/edge`, each with and without `--features test-faults`, and for `workers/ops`;
- `cargo test --workspace`: 67 suites, 583 passed, 0 failed, 3 ignored. That includes `wire_tags`, `refs_paging`, `agent_bytes`, `encoding`, and `catch_ups_on_one_network_take_turns`;
- `python3 scripts/check_design.py`, `check_names.py` and `check_links.py`;
- `go test ./...` in `containers/git/server`.

The raft crate did not change, so the long sweep and Porcupine were not rerun.

Token scan: 26 raw log files (`tests/m3/raw/wirefix/`, `tests/am1/raw/wf-*`, and the drivers' scratch output) against 10 known secrets (both installs' bed tokens, the tools token, the m2b and m3h user tokens, and the Cloudflare API token) and the `ghp_`/`ghs_`/JWT shapes: 0 hits. The drivers pass tokens through files, headers, or git's environment, never argv.

## Commits

| Commit | What |
| --- | --- |
| `9c1a5c7` | `wire_tags.rs` and `wire_tags.golden`: every stored or sent enum's postcard tags and sample bytes pinned |
| `80eebf6` | The test register's `RegEvent` tags pinned |
| `cff5057` | F63: paged ref reads, catch-up from entry ref changes, paged rehydrate, container `/refs` paging and skips, edge info/refs streamed; `refs_paging.rs` |
| `576d876` | Merge `origin/main` `debd398` (docs only) |
| `e0716f1` | graft-m2b site day, edge deploy and image drivers (`tests/m3/wirefix/`) |
| `be085bf` | Member `/entries` reads rows one at a time; `git-bf20.sh` |
| `3fc2397` | A catch-up waits for one in flight; `--roll-shards` goes on past a slow catchup |
| `6eb3433` | `CatchUps::turn` and its unit test; `watch-bf20.sh` |
| `2da290a` | `wire_tags.rs` names its golden file |
| `41b918b` | `recreate.mjs`, `lease-check.mjs` |
| this file | Results |

## Not done, or open

- **graft-m2b runs `3fc2397`, not the final `6eb3433`.** Both make a catch-up wait for one in flight. `3fc2397` waits for idle and then starts, so two waiters woken in the same tick could still both start. `6eb3433`'s `turn` takes the slot in the same poll. graft-m1 runs `6eb3433`. A later site day on graft-m2b picks it up.
- **The site-c explanation is inference.** It comes from the code and the per-route call counts, not from a trace of one election's vote messages.
- **A clone of bf20 has 3 refs.** The 571,870 bench refs name objects that were never pushed, so the shard skips them. receive-pack's advertisement lists all 571,872. This is the data, not the fix: before the fix, nothing answered.
- **The receive-pack advertisement for bf20 takes 24 s** and 2.5 s of edge CPU for 42 MB. It no longer holds every ref at once, but it is slow, and a push to bf20 pays it.
- **graft-m1's `196622` catch-up timed out at 600 s** in the roll (1,001 stored packs, about 1 GB), and `196627`'s shard 1 answered 500 while its member restarted. Neither was retried. Both shards start on the new image at their next use.
- **The recreated j4, lf1-2 and lf1-3 hold no history.** LM-1's rows that used them need their seed again.
- **Token prefixes in this session's transcript.** While locating the m2b token file I printed the first 20 characters of three graft-m2b user tokens (`~/.graft-m2b/tokens.txt`) to my own terminal. They are in no file, log, or commit. Rotating those three test tokens would close it.
- **No design edit.** `decisions/DECISIONS.md` already records this decision ("Released wire positions are pinned…") and F63 on main (`debd398`), and nothing here contradicts them. One wording point, without an edit: the decision says the groups "are recreated or left out, and LM-1's rows rerun on other groups". They were recreated, empty, under the same ids.
