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

## Not built: the hosted half

- **Site classes.** `Inbox` and `SearchShard` Durable Object classes in `workers/site`: the Inbox's SQLite rows, its snapshot to two buckets, and the rebuild; `SearchShard` running `DDL` and created with `idFromName` under `Domain::jurisdiction()`. `workers/site/deploy.mjs` needs a migration for both. The M4 Actions thread is adding `HookSequencer` as migration v3 there, so these go after it.
- **The consumers' Queue wiring** for `Counts`, `Notify`, and `Search`.
- **Edge routes.** `/stargazers`, `/subscribers`, `/subscription`, `/user/starred`, `/notifications` and its threads, and `/search/issues`, all behind `FORMATS_ENABLED` 32.19 and 0x14.3, so a deployment that has not enabled them keeps its current replies.
- **The site day** that enables 32.19 and 0x14.3.
- **Out of scope here:** feeds, `/issues`, and `/user/repos` from the Inbox (BUILD-PLAN's `Inbox` row lists them, and no M4-WP2 gate measures them), `Follow` rendering, and the generation counter's home (the tests keep it in memory; the hosted half stores it on the `Release` group, or derives it from the newest live object).

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
