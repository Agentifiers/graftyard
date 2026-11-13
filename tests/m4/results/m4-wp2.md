# M4-WP2: user records, the watcher index, the `Inbox`, and search, the pure half

BUILD-PLAN M4-WP2 asks for `UserRecord` as `IdentityEvent::Record`, the `Counts` consumer and the watcher index it writes (`watcher` and `thread_sub` rows), the `Notify` consumer, `Inbox { account, site }` with its daily snapshot, `sources` table, and rebuild rule, and `SearchShard` and `Domain` (FTS5, 16 shards per residency domain, under the domain's jurisdiction), with the `rest:activity` surface at T1. This round answers C15 on Cloudflare and builds the pure, in-memory half. BF-5, BF-67, and BF-68 pass on real raft members and real SQLite FTS5. Nothing was deployed except the C15 probe, which was then deleted.

Branch `m4-wp2`, cut from `origin/main` at `a2687fe`.

## C15 first: FTS5 works in Durable Object SQLite

A throwaway Worker ran contentless FTS5 with `contentless_delete=1`, porter, unicode61, and trigram, bm25, snippets, prefix and phrase queries, `optimize`, `integrity-check`, rollback, and persistence across an object reset. It passed both with no jurisdiction and under `eu` (`tests/m4/c15/README.md`) [measured]. The platform refuses `sqlite_version()` and `PRAGMA compile_options`. Cost was under $0.01. The PlanetScale option was not needed.

## What was built

- **`UserRecord`** (`crates/identity/src/identity.rs`, format 0x14.3 `IDENTITY_FORMAT_V3`).
  - New events: `IdentityEvent::Record(UserRecord)` (wire tag 6, encoding tag 4, which the existing comment reserved for it) and `IdentityEvent::CountsCursor(Index)` (7).
  - New ops: `IdentityOp::SetRecord` (8), which only the account itself may send; a repeat emits nothing, so a record keeps its first entry's time and index. `IdentityOp::CountsCursor { through }` (9), which only the operator may send and which moves only forward.
  - New types: `ThreadId { repo, number }`, whose REST `id` is `repo << 24 | number` [inference]; `UserRecord` with DESIGN's five variants; and `UserRows` (stars, watches, follows, subscriptions, thread marks, the Counts cursor), each row stamped with its entry's `at` and `index`.
  - `UserRows` sits in `IdentityMore`'s trailing part (`tail_serde`), so a fold with no record keeps 0x14.2's bytes.
- **The watcher index** (`crates/git/src/watch.rs`, repository format 32.19 `REPO_FORMAT_WATCH`).
  - Metadata rows: `watcher(account)`, `stargazer(starred_at, account)`, `subscriber(account)`, `thread_sub(n, account)`, and `watch_count`.
  - `RepoCmd::Counts(CountsCmd)` (JSON `counts`) commits `RepoEvent::Watchers` (tag 42). `CountsCmd::decide` keeps only updates newer than their row's `src`, so a redelivered or reordered batch commits nothing.
  - `counts_plan` sends each changed (account, repository) pair's whole state as the account's fold holds it, with `src` set to the fold's applied index, never a delta.
  - `COUNTS_EVERY_MS` (1 s) and `COUNTS_BATCH_MAX` (500).
  - `RepoCmd::ConsumerCursor(ConsumerCursorCmd)` (JSON `consumerCursor`): the `Notify` and `Search` cursors on SHARED's `consumer_cursor` row, forward only, committed after the work.
- **Notify and the Inbox** (`crates/git/src/inbox.rs`).
  - `notices` turns one committed repository entry into notices by account. It reads the watcher index, the thread's author, assignees, commenters, and mentions, and review requests.
  - Rules: the actor never hears of its own action; an ignored repository notifies nobody, a mention included [inference]; an explicit thread unsubscription stops everything but a mention.
  - `Inbox` keeps one row per thread, and the newest notice wins. `sources` holds the last index applied from each chain, so a redelivered or replayed entry changes nothing.
  - `view` merges read and done from the account's `UserRows`: done hides a thread until a newer notice, and read clears `unread` until one.
  - `snapshot_due` fires once on a new day after a change. `Snapshot::rebuild_from` is DESIGN's rule: each chain in `sources` from its index, and each watched or subscribed repository from `INBOX_WINDOW` (2 days [guess]) before the snapshot.
- **Search** (`crates/git/src/search.rs`).
  - `Domain::of` maps a placement name to a domain, and `jurisdiction()` gives `Eu` and `Us` their DO jurisdiction. `Apac` has none, so placement alone covers it. `admits` says which sites may hold a domain's shards.
  - `SearchShard { domain, shard, generation }` with `do_name` and `site()`: the domain's admitted sites minus lost ones, one site further on for each generation.
  - `shard_of` is the first byte of SHA-256 of the repository id, mod 16.
  - `DDL`: a `doc` table plus contentless FTS5 `doc_fts(title, body, comments)`. `docs_for` and `all_docs` turn entries into document ops.
  - `parse` handles words, phrases, `is:`, `type:`, `state:`, `in:`, `repo:`, `org:`, `user:`, and `author:`, and flags anything else, which gets 422. `fts_match` quotes every term, so query text is never FTS5 syntax.
  - `query_sql` filters to public documents or the viewer's reach. bm25 is computed in an inner query because FTS5 refuses it beside a window function, which a test found.
  - `merge` sets `incomplete` when any shard missed `SHARD_DEADLINE` (5 s [guess]). `shards_for` covers the `Default` domain plus the viewer's orgs' domains.
- **REST** (`crates/api/src/activity.rs`).
  - Routes: `/stargazers`, which returns `stargazer` objects under `application/vnd.github.star+json` and `simple-user` otherwise, `/subscribers`, `/repos/{o}/{r}/subscription`, the notification `thread` object and page, `thread-subscription`, `issue-search-result-item`, the search envelope with `search_type: "lexical"`, and a 422 for an unsupported qualifier.
  - `full_repository` now renders `stargazers_count`, `watchers`, `watchers_count`, and `subscribers_count` from `watch_count`. These are 0 when nobody starred or watched, the value it always rendered.

## Byte identity

No reply changes for a client that does not opt in.

- Every new event, op, and enum is appended. `wire_tags.golden` gained 24 lines and lost none. `agent_bytes` and `wire_tags` pass.
- No earlier command takes 32.19 or 0x14.3, and a fold with no record keeps its old bytes. `user_records_…` checks this by clearing the records and comparing to the bytes before them.
- The only change to an existing renderer is `full_repository`'s four counts, which read 0 from an empty `watch_count`. Every existing `surfaces.rs`, `narrow.rs`, and `repos` test passes unchanged, and `same_bytes_without_opt_in` holds.
- The new routes are not wired into the edge, so the edge still answers them 404 (below).

## The gates

`crates/api/tests/activity_gates.rs` runs on three real `graftyard_raft::Member`s per `Identity` chain and per repository chain. `Counts` reads each account's committed log after its cursor. `Notify` and `Search` read each repository's committed log on a follower after their cursor. Each `SearchShard` is a real SQLite database (bundled through `rusqlite` 0.37) with `search::DDL`. Time is simulated: a commit round of 200 ms [guess, B0-12's enam to apac median], and a consumer poll of 1 s [guess]. Run `cargo test -p graftyard-api --test activity_gates -- --nocapture`.

### BF-5. Pass.

Bob watches the repository. Alice stars it and comments on Bob's issue, mentioning him, after Bob's day snapshot was written.

| Measure | Pass | Result [measured, simulated time] |
| --- | --- | --- |
| Star in `stargazers_count` | within 10 s | 1,400 ms |
| Comment in Bob's `/notifications` | within 10 s | 1,400 ms |
| Rebuilt `Inbox` (from the snapshot before the comment) | read state kept | same threads, same `unread`, `last_read_at`, and reason |

### BF-67. Pass.

Bob watches r1 and r5, subscribes to one thread of r2, and is mentioned on r3. On day one he reads two r1 threads and marks one done. On day two more notices arrive on every `sources` chain, including a new mention on r3, on the done thread, and from r5's first notice (watched, but never in a snapshot). r4 sends a first mention from a chain no snapshot named.

| Measure | Pass | Result [measured] |
| --- | --- | --- |
| Rebuild from day one's snapshot | read and done kept, nothing from `sources` lost | every thread but r4's: read, done, and unread state kept; r5 replayed through `INBOX_WINDOW`; every `sources` chain replayed from its index. r4's first mention is missing, as DESIGN states ("missing until that chain's next event") |
| Rebuild from day two's snapshot | the same | identical to the live Inbox |
| 10,000 stars through `Counts` | `/stargazers` lists all 10,000 | 10,000 over 20 `Counts` entries, oldest first; a redelivery of every batch applies 0 |
| A refold of the repository | the same | metadata digest equal to the live fold's; `/stargazers` lists the same 10,000 |
| Then one unstar, one watch, one watch-then-ignore, and the first star batch again | the newer state wins | 9,999 stargazers without the unstarred one; the watcher subscribed, the ignorer not; `subscribers_count` equals the list |

### BF-68. Pass.

Owners: one on `Tri` (the `Default` domain) and an EU org on `Eu2w` with one public and one private repository. Five sites, two of them in the `eu` jurisdiction.

| Measure | Pass | Result [measured] |
| --- | --- | --- |
| Where EU documents are | only `eu`-jurisdiction shards | every EU document is in an `eu` shard created under `Jurisdiction::Eu` on an `eu` site; no `Default` shard holds one, before or after the rebuild |
| Search with the shard killed | `incomplete_results` | `search#eu#12#1` killed with its site: `incomplete_results: true` and 7 of 13 hits, exactly the documents of the other shards; the `Search` cursor holds for the lost shard's chains |
| After the rebuild | full results | generation 2 on the other `eu` site, rebuilt from the metadata tiers: 14 of 14 (one written during the outage), `incomplete_results: false` |
| Other checks | | a private document only with reach; a comment's words find its issue; `"kumquat" OR NEAR(` matches nothing |

### Other checks

- `notify_skips_the_actor_ignorers_and_unsubscribers_but_not_a_mention_and_cursors_only_move_forward`: on one comment, exactly the author (`author`) and a mentioned, thread-unsubscribed watcher (`mention`) are notified. The commenter, a mentioned ignorer, and an unsubscribed watcher are not. An older thread-subscription batch applies 0. `ConsumerCursorCmd` moves only forward and only for `Notify` and `Search`.
- `an_inbox_keeps_the_newest_notice_and_ignores_an_entry_at_or_below_its_sources`.
- `activity_and_search_replies_render_ghes_objects`: writes 8 REST replies (both `/stargazers` media types, `/subscribers`, `/subscription`, a notification list, a `thread-subscription`, a search reply, and the 422). `crates/api/gh/validate.py` checks them against the pinned GHES 3.22 description: 8 checked, 0 failed. Its first run caught a missing `search_type`, which 3.22 requires.
- `crates/identity/tests/wp3.rs::user_records_are_the_accounts_own_kept_with_their_entry_and_the_counts_cursor_only_moves_forward`.
- Full checks: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; wasm32 clippy for `graftyard-site` and `graftyard-edge`, each with and without `test-faults`: all clean. `cargo test --workspace`: 782 passed, 0 failed. The three design checks pass.

## Planted bugs

`tests/m4/wp2/plant.py` plants 30 mutants one at a time and restores each file byte for byte. The first runs missed 14 of the mutants then planted:
- Notify's actor, ignore, and unsubscription rules, and the Inbox's `sources` and newest-wins rules, which no gate exercised one by one.
- The rebuild's two halves. BF-67 had no watched repository outside `sources` and no new event on a `sources` chain that only the rebuild could replay.
- A rebuild landing on the lost site.
- A stale thread-subscription batch.
- The cursors moving backward.
- Three Identity guards whose mutants another check already covered. Those mutants were replaced with ones that change behavior.

Each now has a test that observes the behavior, and the final run catches all 30 (table below).

The plants also found code that could never matter, and both pieces were removed:
- `write_meta` re-checked `src` after `decide` had already checked it, so its guard could never fire.
- The fold's `max` on `CountsCursor` repeated `decide`'s forward-only check.

| # | Mutant | Verdict | Caught by |
| --- | --- | --- | --- |
| 0 | decide keeps stale updates | CAUGHT | `bf67_a_rebuilt_inbox_keeps_read_done_and_every_sources_event_and_10_000_stars_survive_a_refold` |
| 1 | decide keeps stale thread subscriptions | CAUGHT | `notify_skips_the_actor_ignorers_and_unsubscribers_but_not_a_mention_and_cursors_only_move_forward` |
| 2 | an unstar leaves the stargazer row | CAUGHT | `bf67_a_rebuilt_inbox_keeps_read_done_and_every_sources_event_and_10_000_stars_survive_a_refold` |
| 3 | an ignored repository still counts a subscriber | CAUGHT | `bf67_a_rebuilt_inbox_keeps_read_done_and_every_sources_event_and_10_000_stars_survive_a_refold` |
| 4 | the plan reads only past the cursor up to nothing (applied bound ignored) | CAUGHT | `activity_and_search_replies_render_ghes_objects`, `bf5_a_star_and_a_watched_comment_show_within_10_s_and_a_rebuilt_inbox_keeps_read_state`, `bf67_a_rebuilt_inbox_keeps_read_done_and_every_sources_event_and_10_000_stars_survive_a_refold` |
| 5 | the consumer cursor moves backward | CAUGHT | `notify_skips_the_actor_ignorers_and_unsubscribers_but_not_a_mention_and_cursors_only_move_forward` |
| 6 | the actor is notified of its own comment | CAUGHT | `notify_skips_the_actor_ignorers_and_unsubscribers_but_not_a_mention_and_cursors_only_move_forward` |
| 7 | an ignored repository still notifies | CAUGHT | `notify_skips_the_actor_ignorers_and_unsubscribers_but_not_a_mention_and_cursors_only_move_forward` |
| 8 | a thread unsubscription is ignored | CAUGHT | `notify_skips_the_actor_ignorers_and_unsubscribers_but_not_a_mention_and_cursors_only_move_forward` |
| 9 | watchers are not notified | CAUGHT | `activity_and_search_replies_render_ghes_objects`, `bf5_a_star_and_a_watched_comment_show_within_10_s_and_a_rebuilt_inbox_keeps_read_state`, `bf67_a_rebuilt_inbox_keeps_read_done_and_every_sources_event_and_10_000_stars_survive_a_refold` |
| 10 | a replayed entry applies again (sources ignored) | CAUGHT | `an_inbox_keeps_the_newest_notice_and_ignores_an_entry_at_or_below_its_sources` |
| 11 | an older notice overwrites a newer one | CAUGHT | `an_inbox_keeps_the_newest_notice_and_ignores_an_entry_at_or_below_its_sources` |
| 12 | done never hides a thread | CAUGHT | `bf67_a_rebuilt_inbox_keeps_read_done_and_every_sources_event_and_10_000_stars_survive_a_refold` |
| 13 | read state is ignored | CAUGHT | `activity_and_search_replies_render_ghes_objects`, `bf5_a_star_and_a_watched_comment_show_within_10_s_and_a_rebuilt_inbox_keeps_read_state`, `bf67_a_rebuilt_inbox_keeps_read_done_and_every_sources_event_and_10_000_stars_survive_a_refold` |
| 14 | the rebuild skips watched repositories (no INBOX_WINDOW) | CAUGHT | `bf67_a_rebuilt_inbox_keeps_read_done_and_every_sources_event_and_10_000_stars_survive_a_refold` |
| 15 | the rebuild ignores sources | CAUGHT | `bf67_a_rebuilt_inbox_keeps_read_done_and_every_sources_event_and_10_000_stars_survive_a_refold` |
| 16 | a snapshot is never due | CAUGHT | `bf5_a_star_and_a_watched_comment_show_within_10_s_and_a_rebuilt_inbox_keeps_read_state`, `bf67_a_rebuilt_inbox_keeps_read_done_and_every_sources_event_and_10_000_stars_survive_a_refold` |
| 17 | an EU domain has no jurisdiction | CAUGHT | `bf68_eu_documents_stay_in_eu_shards_a_killed_shard_reports_incomplete_until_its_rebuild` |
| 18 | every domain admits every site | CAUGHT | `bf68_eu_documents_stay_in_eu_shards_a_killed_shard_reports_incomplete_until_its_rebuild` |
| 19 | a rebuild lands on the same site | CAUGHT | `bf68_eu_documents_stay_in_eu_shards_a_killed_shard_reports_incomplete_until_its_rebuild` |
| 20 | query text is FTS5 syntax (terms unquoted) | CAUGHT | `bf68_eu_documents_stay_in_eu_shards_a_killed_shard_reports_incomplete_until_its_rebuild` |
| 21 | a partial result without incomplete_results | CAUGHT | `bf68_eu_documents_stay_in_eu_shards_a_killed_shard_reports_incomplete_until_its_rebuild` |
| 22 | private documents without reach | CAUGHT | `bf68_eu_documents_stay_in_eu_shards_a_killed_shard_reports_incomplete_until_its_rebuild` |
| 23 | comments are not indexed | CAUGHT | `bf68_eu_documents_stay_in_eu_shards_a_killed_shard_reports_incomplete_until_its_rebuild` |
| 24 | a query reaches only the Default domain | CAUGHT | `bf68_eu_documents_stay_in_eu_shards_a_killed_shard_reports_incomplete_until_its_rebuild` |
| 25 | the operator sets a record (SetRecord guard gone) | CAUGHT | `user_records_are_the_accounts_own_kept_with_their_entry_and_the_counts_cursor_only_moves_forward` |
| 26 | a repeat record emits again | CAUGHT | `user_records_are_the_accounts_own_kept_with_their_entry_and_the_counts_cursor_only_moves_forward` |
| 27 | the counts cursor commits a backward entry | CAUGHT | `user_records_are_the_accounts_own_kept_with_their_entry_and_the_counts_cursor_only_moves_forward` |
| 28 | the fold applies a cursor other than the one committed | CAUGHT | `user_records_are_the_accounts_own_kept_with_their_entry_and_the_counts_cursor_only_moves_forward` |
| 29 | stargazers render without the star media type | CAUGHT | `activity_and_search_replies_render_ghes_objects`, `bf67_a_rebuilt_inbox_keeps_read_done_and_every_sources_event_and_10_000_stars_survive_a_refold` |

## Hosted on `graft-m4a`: BF-5, BF-67, and BF-68 pass

Built on `m4-wp2` and deployed through two days logged in `tests/am1/results/install-m4a.md`:

- **Site day 8** (15:58Z, sites `c392af30` / `770a594d` / `bf9bb597`): `m4a-wp2-deploy` `0f32337` = `m4a-deploy` `5af2de0` + `m4-wp2` `4e9da43`. It adds migration v4 (`SearchShard`, `Inbox`), the `SEARCH_n` and `INBOX_n` bindings (own-only in phase 1), Repo reads 32.19, Identity reads 0x14.3, and `GRAFTYARD_ACTIVITY=1`. The 15:50Z attempt stopped at wrangler's check before any upload: the `durable_object` macro exports the Rust struct's own name, and `pub use inbox::InboxObject as Inbox` left the class exported as `InboxObject` (fixed in `4e9da43`).
- **Edge day 7** (M4 web, 16:01Z, edge `3d41cbe7`): `FORMATS_ENABLED` plus 32.19 and 20.3.
- M4 Actions' site day 9 (16:20Z) carried both. BF-5, BF-68, and BF-67's Inbox half ran on it.
- **Site day 10** (17:21Z, sites `06f464eb` / `24a3ab1f` / `9200a44f`): `m4a-wp2-deploy` `88652fe` = `m4a-deploy` `ff0e804` + `m4-wp2` `0055d62`, the `SQLITE_LOCKED` fix below. BF-67's stars half ran on it.

What the hosted half is (`workers/site/src/{activity,inbox,search}.rs`, `workers/edge/src/activity.rs`):

- The consumers run in band on their chain's leader alarm, not through Queues (DECISIONS, "M4-WP2's hosted half, as built"). `Counts` runs on each account's `Identity` leader, and `Notify` and `Search` on each repository leader, each from its committed cursor.
- `SearchShard` objects are created under the domain's jurisdiction. m4a has no `eu` site, so the `Eu` domain binds on the weur site under `ns.jurisdiction("eu")`.
- The edge routes answer only once `FORMATS_ENABLED` holds 32.19 and 0x14.3.

Drivers are in `tests/m4/wp2/hosted/`, raw logs in `tests/m4/wp2/raw/`.

### BF-5. Pass.

m3-member watches `m3org/wp2-bf5`. m3-admin unstars, opens an issue (GraphQL `createIssue`, as gh sends it: the install serves no REST issue create), waits 15 s, then stars and comments (`addComment`). Both are timed from the star's `PUT`.

| Measure | Pass | Result [measured] |
| --- | --- | --- |
| Star in `stargazers_count` | within 10 s, fail over 60 s | 3,984 ms (the two writes took 3,601 ms of it) |
| Comment's thread in the watcher's `/notifications`, unread | within 10 s | 5,041 ms |
| Mark read, snapshot, delete, rebuild through the `Identity` leader's `/inbox` | read state kept | `/notifications?all=true` was empty after the delete, and after an 851 ms rebuild it was identical to the live view, including the read thread's `last_read_at` |

An earlier run failed on the driver, not the system: it read `before` while the unstar was still in flight through `Counts`, so `before + 1` never came. The driver now waits for the count to settle first.

### BF-67. Pass.

**Stars.** `seed.mjs` made 10,005 real accounts, the row's 10,000 plus a 5-account trial. Each one has:

- its `Names` rows (`Account`, a claimed login `wp2-s<i>`, `AccountLogin`, and a `Tri` placement),
- its own `Identity` group with `Created`,
- a `SetRecord(Star)` under the account's own web-session credential, which `decide` checks as the account itself, in the same entry as `Created`.

That took 64 workers in each of 4 processes, 11 minutes, about 15 accounts/s. Two accounts failed on a group-creation timeout and passed on a rerun.

| Measure | Pass | Result [measured] |
| --- | --- | --- |
| `/stargazers` (101 pages of 100, star media type) | all 10,000 listed | 10,005 listed, 10,005 distinct, 0 missing, 0 `ghost` (each page reads its own accounts' logins), `stargazers_count` 10,005; slowest page 6.2 s |
| A refold of the repository (the product fold audit, forced, on site day 10's build) | metadata digest equal; the list after | the refold of `repo:65543` through index 3,167 (616 KB, one batch) gives core `3ba604ba…` and metadata `feb8cc4b…`, and every voter's digests at 3,167 equal both, including site 1's replaced voter `#4#f1`. `/stargazers` after: 10,005 listed, 0 missing, 0 `ghost`, count 10,005 |

**Inbox.** The watcher's Inbox held 4 threads on `wp2-bf5`. One was marked read and one done, then the Inbox was snapshotted. After the snapshot, a comment went on every thread, which is a new event on the `sources` chain. Then the Inbox was deleted and rebuilt from the snapshot plus `sources`.

| Measure | Pass | Result [measured] |
| --- | --- | --- |
| Rebuilt `/notifications?all=true` | read and done kept, nothing from `sources` lost | identical to the live view: all 4 threads with the post-snapshot comments' `updated_at`; the read and the done thread keep their `last_read_at` and are unread again under GHES's rule, because a newer notice arrived; 0 threads after the delete; rebuilt in 1,113 ms |

**The bug.** The fold audit failed at its closing `DROP TABLE` of the scratch tables with `SQLITE_LOCKED` on every repository with a fold row read that hit: `wp2-stars`, `wp2-bf5`, and `wp2-probe`, but not `j4`. `TierSql::sql_get` took one row from its cursor and dropped it without reading to the end, so the statement held its table. It is the same class as d6e350d's log cursor. It is not specific to M4-WP2, but the new consumers are the first path that reads point rows on an audited repository. Fixed in `0055d62`: `sql_get` reads to the end. Site day 10 shipped the fix, and the refold row then passed. The first run after it compared site 1's generation-1 object, which a replacement had retired (null digests), so the driver now reads each site's current voter by name (`voterNames`).

### BF-68. Pass.

Org `wp2eu` (7400001) has an `Eu2w` record on sites [1, 0, 2], home 1 (weur), and four public repositories with three issues each, whose titles carry the run's word. Search reached all 12 in the 2.7 s it took to ask.

| Measure | Pass | Result [measured] |
| --- | --- | --- |
| Where the documents are (every shard of both domains, `/stats`) | only `eu`-jurisdiction shards | 3 shards hold all 12: `search#eu#2#1` (6), `#4#1` (3), and `#8#1` (3). Each reports `state.id.jurisdiction = "eu"` and an EU colo (PRG, PRG, MXP). No `default` shard holds any |
| `search#eu#2#1` lost | `incomplete_results` | `total_count` 6 (exactly the other two shards' documents), `incomplete_results: true` |
| The rebuild (`{rebuild}`, generation 2) | full results | 39 s to scan every site's registry shards and rewrite 2 repositories' 6 documents into `search#eu#2#2`; then 12 of 12, `incomplete_results: false` |
| The same census after | only `eu` | `search#eu#2#2` (6, eu, WAW), `#4#1` (3, PRG), `#8#1` (3, MXP); 0 outside |

Out of BF-68's scope, not a deviation: the org's repository chains still run on m4a's three sites (enam, weur, apac). Holding them on EU sites is Milestone 5's EU sites (the parent's ruling).

### Teardown

- **Deleted through the product route** (17:38Z, all 204): `m3org/wp2-stars`, `wp2-bf5`, and `wp2-probe`, and `wp2eu/e0` to `e3`. `DeleteRepo` tombstones each chain and holds its name for 90 days, as SHARED's retention says. The `Search` consumer removes their documents from the shards (`DocOp::Repo` on the tombstone).
- **Left in place, because no product operation removes them:**
  - the 10,005 seeded accounts `wp2-s0` to `wp2-s10004` (7300000 to 7310004), with their `Names` rows and `Identity` groups;
  - the `wp2eu` org (7400001) and its `Authz` group;
  - m3-member's Inbox (`inbox:7100002`) and its two R2 snapshots under `inbox/7100002/`;
  - the `search#eu#*` and `search#default#*` objects and the `search_map` rows.
  
  Idle groups and objects bill storage only, and the whole install goes with `install-m4a.md`'s teardown.
- Nothing else was created: no new script, bucket, queue, or secret. The `tests/m0/.m0-token` link was removed.

### GHES 3.22 shapes

`replies.mjs` captured 11 live replies: both `/stargazers` media types, `/subscribers`, `/subscription`, `GET /user/starred/{o}/{r}`, `/notifications`, a thread, its subscription, `/search/issues` and its 422, and `full-repository` with the counts. `crates/api/gh/validate.py` against the pinned descriptions (both sha256s match the README): 11 checked, 0 failed.

## Hosted cost estimate for BF-5, BF-67, and BF-68 on `graft-m4a`

Estimated at `research/prices.json` overage rates with allowances ignored. The anchor is the measured `graft-m3c` LF-13 window: $2.55 for a build, four site days, and 33 runs (`tests/am1/results/lf13-m3c.md`, "Cost").

| Row | What costs | Estimate |
| --- | --- | --- |
| BF-5 | 3 `Identity` chains, ~30 entries, one Inbox, one snapshot | under $0.05 |
| BF-67 | 10,000 stargazer accounts, each a new `Tri` `Identity` group (3 members) with `Created`, `Star`, and `CountsCursor`: about 4 M rows written, 0.36 M DO requests, and about 2.2 M DO active-seconds while the new groups heartbeat [guess, 70 s each] | about $7.50 |
| BF-68 | 32 shard objects in 2 domains, ~15 documents, a kill and a generation-2 rebuild | under $0.10 |
| Site and edge days | one site day (classes and migration) and one edge day | under $0.20 |
| Containers | none needed (no git); worst case one stray `standard-4` hour | up to $0.15 |
| **Total** | | **about $8, with a $12 stop** |

BF-67's 10,000 accounts drive the cost. Seeding them through 10,000 real signups is what the row says. Seeding the stars through 100 accounts with 100 repositories each would test something else, so the estimate keeps the row as written.

### Measured

`INSTALL=graft-m4a python3 tests/fanout/m2bcost.py`, per window, at the same rates. The install is shared, so each window also holds the other threads' work, mostly the git shard containers that M4 Actions' runs keep up. From 15:00Z to 16:12Z, before any row of this work ran, they cost $1.37.

| Window (2026-11-13) | What ran | Durable Objects and Workers | Containers (every thread's) |
| --- | --- | --- | --- |
| 15:00Z to 16:12Z | site day 8, edge day 7 (M4 web), the tools edge | $0.30 | $1.37 |
| 16:12Z to 16:22Z | the 5-account seed trial; site day 9 (M4 Actions) | $0.02 | $0.19 |
| 16:22Z to 16:34:50Z | 200 accounts, BF-5 | $0.31 | $0.29 |
| 16:34:50Z to 16:48Z | 9,800 accounts, BF-68 | $4.81 (3.1 M rows written, 0.81 M DO active-seconds, 2.7 M DO requests) | $0.37 |
| 16:48Z to 17:11Z | BF-67's listing, the Inbox half, the refold attempts, the GHES capture | $0.36 | $0.69 |
| 17:11Z to 17:37Z | site day 10, BF-67's stars half (two 101-page listings and the refold) | $0.81 | $0.80 |
| **15:00Z to 17:37Z** | | **$6.62** | **$3.72** |

The install's whole window was **$10.34**, against the $12 stop. Site day 10 resets every member, and each `/stargazers` page reads its accounts' logins on up to 100 `Names` groups. Together these kept about 400 objects active per minute after 17:20Z, so hosted work stopped at the last row. The seed alone was about $5.1, against the $9 trip for seeding. The estimate was $7.50 for BF-67, and its guess of 2.2 M DO active-seconds came in at 0.85 M.
