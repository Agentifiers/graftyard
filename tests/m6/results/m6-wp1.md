# M6-WP1, region loss and re-protection

This file records M6-WP1's work (`build/BUILD-PLAN.md`, M6-WP1; BF-50). Status: **the pure half is built** on branch `m6-wp1-pure`. The hosted glue and BF-50 on graft-m6a are not started. The operator approved graft-m6a for after the pure half and the glue merge (3 sites plus a wnam spare from genesis, test-faults builds, $20 a UTC day and $40 in total).

## The pure half (2026-11-24)

| Part | Where | Tests |
| --- | --- | --- |
| `graft-ops` signs `ReplaceMember` (executor key) on `SiteLost`, or `ZERO_ACKS` of drain with both other sites silent | `crates/release/src/ops.rs`, `auto_replace`; `gate::loss` | `tests/group/replace.rs` |
| A `SiteLost` site nobody drained is drained first | `ops::auto_drain` | `a_site_lost_that_was_never_drained_is_drained_first` |
| The spare on the lost site's continent, with a page when none exists or it cannot read every enabled format | `SiteTable::spare_for`; `auto_replace` | `a_lost_site_with_no_spare_on_its_continent_pages`, `a_site_lost_replaces_at_once_but_never_onto_a_spare_that_cannot_read_every_enabled_format` |
| One `Move` step per intent: start the coordinator, wait, then done, or a page on failure | `ops::Step::Move`, `Exec::{move_run, start_move}`; `workers/ops/src/exec.rs` (`WORKFLOWS` binding) | `a_failed_*`, the `ZERO_ACKS` test |
| Undrain only on recovered (`RECOVER_AFTER`); `RestorePlacement` refused while drained, then moving back and clearing the loss | `ops::auto_drain`; `group.rs`, `replaced` | `a_replaced_site_undrains_only_once_recovered_*` |
| One group's replace steps | `crates/identity/src/group_move.rs`, `replace_step`, `holds` | `crates/identity/tests/group_move.rs`, on real members |
| The groups to move | `crates/types/src/registry.rs`, `replace_candidates` | `every_chain_on_a_surviving_site_is_a_candidate_once_at_its_highest_generation` |
| The spare's bucket fill | `crates/git/src/sweep.rs`, `refill` | `a_refill_copies_single_copy_packs_first_from_surviving_buckets_only` |

**On real members** (`crates/identity/tests/group_move.rs`):
- **Tri.** The leader's site 0 is blackholed, site 1 is elected, and the steps are `Swap { add: 3, drop: 0 }`, `Leave`, `Home`, `Transfer`. The spare leads at generation 2, and every acknowledged write is on 1, 2 and 3.
- **`*2w`, follower Full member lost.** The steps are `Degrade`, then `Restore`. The spare is Full in slot 1, and every write is kept on both Full members.
- **`*2w`, witness lost.** The steps are `Swap` and `Leave`, with no `Degraded`.
- **`*2w`, leader's Full site lost.** The successor cannot commit its `Noop`, so `reconfigure` answers `Busy`. That is OPEN-QUESTIONS R48, which the operator kept out of the hosted row.

**Plants.** All 11 mutants are caught:
- `tests/m0carried/plant115.py`, 3 mutants: a restore while drained, an undrain on calm, and a `ZERO_ACKS` replace without silence.
- `plant116.py`, 2 mutants: a spare that can't read a format.
- `plant117.py`, 3 mutants: the refill order and its sources.
- `plant118.py`, 3 mutants: a group left out.

Main's `gate-pool` took 114 before this branch, so these are 115 to 118.

**Not built, and where it goes:**
- **The hosted glue.**
  - The Workflows coordinator (plain JS, after a C3 check that its deploy creates no site-script version).
  - member.rs's replace on `/move` beside `replace_on` (LM-1 cleared it; a `Degraded` arm reads the log through `logread::log_after`).
  - The drained-site probe.
  - 24 h per-minute `SiteHealth` rollups, which `Live::health` needs before it can call `gate::loss`. Until then `silent` and `recovered` stay false on the host, so only `SiteLost` replaces and an operator undrains.
  - `deleteAll()` on removed members after 35 days.
- **Deferred by ruling.** The `from_record` swap for creates during a loss (R49), and `*2w` hosted (R48).

## BF-50 (planned)

On graft-m6a, from merged main, after the glue: site A lost with every account API token revoked, drained 24 h with zero acks, `ReplaceMember` by the executor, then recovery and `RestorePlacement`. The recovery window is lowered under test-faults only, which is a deviation to record here. The row is not run yet.
