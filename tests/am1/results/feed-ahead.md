# `feed-ahead`: a feed cursor past the member's last entry answers `lagging`

Branch `feed-ahead`, off main `534845ad`. No hosted run yet; L0-12's whole rerun is the hosted check.

## Why

L0-12 m3fr7's Workers Logs show site c's two apac learners answering 6,176 of their 20,670 feed polls in under 20 ms, against 632 of 21,795 on site a's four learners and 173 of 8,688 on site b's two. The driver counted 6,574 `gone` answers [measured, `tests/am1/results/lm1-rows-m3f.md`, "m3fr7 after the run"]. The logs carry no bodies, so tying the fast polls to the `gone` count is an inference.

From the code, two defects together make that loop:

1. **The member.** `graftyard_git::feed::start` answered 410 for any cursor outside `oldest..=newest`, including one past the member's last applied entry. A poller held on a caught-up learner carries that learner's cursor to the next member it asks. A learner behind it answered 410, and the edge's `activity` returned the 410 to the client at once, without trying another member.
2. **The driver.** `tests/am1/rest/a1.mjs` kept its old cursor on `gone`, because a `gone` answer carries `oldest`, not `cursor`. So it asked the same lagging learner again at once.

## The change

- **Design.** `design/AGENT-LAYER.md`, "Activity feed": a cursor past the member's last applied entry, but at or below the highest commit index the member knows, is no 410. The member answers `lagging`, and the edge tries the next member. A cursor past that commit index, such as one from before a whole-install restore, is 410. Before this edit the design limited 410 to a cursor older than the ring or inside an install hole, and said nothing of one ahead.
- **Code.** `feed::start` takes the member's known commit index and returns `Start::Ahead` for a cursor in `newest + 1..=committed`. The host keeps the largest commit index a peer sent it this boot (`Host::heard`: an AppendEntries' `commit`, a read index, an install's `through`), and `feed::route` passes the larger of that and its own commit. `workers/site/src/feed.rs` answers it `lagging` through `Following::behind_cursor`, whatever the member's own lag. The edge's last resort (`lagging_ok`) still holds it, reading after the cursor. `graftyard_api::leases::activity` treats `Ahead` as `After`. 410 is unchanged for another group's cursor, a malformed one, one below the ring's floor, and one inside an install hole.
- **Driver.** `a1.mjs` follows `oldest` on `gone`, and its summary adds `feed.byMember`: every member's answers by kind (`hold`, `lagging`, `gone`, `full`, `notServing`, `tooMany`), keyed `site:learner|voter:member`.
- **Decision.** `decisions/DECISIONS.md`, "`feed-ahead`".

## The bound (review of `41c93e72`)

The first push bounded `Ahead` by the member's last applied entry alone. A whole-install restore keeps the group's `RepoId` and can end its log below a cursor a client holds. Under that rule every member would answer `lagging`, and the edge's last resort would hold the poll on a learner. Once the log grew past the cursor, the client would get events only from there on, skipping those between the restore point and its cursor. Bounded by the commit index the member knows, such a cursor names no entry any member knows committed, and it gets 410.

## Checks

- `a_learner_behind_the_cursor_answers_lagging_not_gone` (`crates/git/tests/lease_gates.rs`) fails on main and passes with the change:
  - A ring that applied one entry gets the cursor of a ring that applied three. On main that is `Gone`; with the change it is `Ahead`.
  - The member answers `lagging` with lag 0, and `route` holds the poll on the caught-up learner.
  - A cursor inside the ring is still `After`, and one below the floor is still `Gone`.
- `a_cursor_past_the_groups_commit_index_is_gone` (`crates/git/tests/lease_gates.rs`) and `a_cursor_past_the_groups_commit_index_is_gone_even_on_the_last_resort` (`workers/site/src/feed.rs`: a `lagging_ok` poll on a learner, cursor 55 against commit 40, answers `gone` at once and holds nothing) fail on `41c93e72` and pass with the bound.
- `tests/m0carried/plant22.py`: 8 of 8 caught. The plants are:
  - a cursor ahead is gone again;
  - a cursor ahead is served as one inside the ring;
  - a cursor at the newest entry is ahead;
  - a cursor below the floor is served;
  - a member behind the cursor still follows;
  - a cursor past the commit index is ahead (`41c93e72`'s rule);
  - the bound is one past the commit index;
  - the member bounds by its own applied entry alone.
- Full checks on the branch, all pass:
  - `cargo fmt --check`;
  - clippy for the workspace and for wasm site and edge, with and without `test-faults`;
  - `cargo test --workspace`: 1,012 passed, 0 failed;
  - Go `containers/git/server` and `tests/m0/porcupine`;
  - `check_design`, `check_names`, `check_links`.
- `Acked` is untouched: the feed path builds no `Acked`, and `outcome` is unchanged.
