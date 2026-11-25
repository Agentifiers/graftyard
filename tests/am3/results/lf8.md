# LF-8 hosted

`build/BUILD-PLAN.md`, AM3-WP1, LF-8: "`release/entries` with an entry signed for another install id, a `Keys` entry not
signed by the anchored customer key, and an entry out of index order." Pass: "All refused by the agents view, 100 of
100." Fail: "Any accepted."

Plan A, as the lead cleared it on 2026-11-24. No install is touched. A forger, `graft-lf8-feed`, serves a synthetic
signed history and one forgery at a time as `GET /api/graftyard/v1/release/entries`. The front under test is main's
`graft-agents-front`, built from `workers/agents-front` with no changes; only its vars change (`GRAFTYARD_HOST` is the
forger, the anchor is the synthetic install A, and the App key is a throwaway RSA key the forger never checks).
**Stand-in:** the runner account stands in as the agents account (`FRAME.md`, "Operator decision, 2026-11-24, AM1
closed and the runner account stands in as the agents account"). The front, a plain `graft-controller` and their
throwaway secrets were named `graft-agents-lf8-`. The forger ran in the install account as `graft-lf8-feed`, because a
runner-account Worker that fetches a same-account workers.dev Worker gets 404, error 1042, unless the front carries
`global_fetch_strictly_public`, and giving it that flag would change its config (`tests/am3/raw/lf8/reach.log`). Stop:
$1.

Run r1 from lf8-hosted `d4ed774a` (main `9ff8c59c` plus the tools). The driver is `tests/am3/hosted/lf8-day.sh`, and the
logs are in `tests/am3/raw/lf8-r1/`. Start sweep 00:06:53Z; **done line 00:20:07Z** (end sweep). **Pass.**

## Verdict

| Line | Result |
| --- | --- |
| Entry signed for another install id | **100 of 100 refused** `WrongInstall`. Each refusal named its own install B_k [measured] |
| `Keys` not signed by the anchored customer key | **100 of 100 refused**: 20 each signed by vendor, executor, agents_executor or the new customer alone (`WrongSigner`), and 20 by a stranger (`UnknownKey`, naming that key's id) [measured] |
| Out of index order, gap (k+2 to k+8) | **50 of 50 refused** `BadState: a release/entries page skips an index` [measured] |
| Out of index order, replay (k-1 25, k 25, each with bytes other than the held entry's) | **50 of 50 moved nothing**: index stayed 10 and the pin stayed release 3's controller. The front did not refuse them; it skipped them as held (see "Replays") [measured] |
| Any accepted | **none**. After all 300, the valid control at index 11 was taken with the pin unchanged [measured] |
| Cold front, install B's genesis | `graft-agents-lf8-cold` never read a view, and it refused `WrongInstall` at index 0 [measured] |

Front versions under test: `graft-agents-lf8-front` `14588085` and `graft-agents-lf8-cold` `eed7cf13`, deployed by
`deploy.mjs` and then `secret put FRONT_TOKEN`. Each row took p50 1,142 ms and p99 3,965 ms from selecting the forgery to
the finished refresh (n 303), and no row timed out. Cost: $0.00 for `graft-lf8-` in the install account and $0.00 for
`graft-agents-lf8-` in the runner account (`cost.log`).

## How the counts are made to mean something

`lf8forge` (`tests/m3/hosted/tools/src/bin/lf8forge.rs`) draws every key and id from a per-run seed. It builds the
history and the forgeries offline into `~/.lf8/r1` (0700), and teardown shredded that directory. The repository holds
only `manifest.tsv` (class, k, case, sha256, field). Before the tool writes anything, it checks three things for each
forgery:
- It differs from the valid control (`IntentStep` at index 11) in one field only. That is `bound.install`, the signer
  (with the body, for the new customer alone), or the index.
- An in-memory `AgentsReleaseView` holding the history refuses it with its own variant and text.
- That view is unchanged afterwards.

After all 300, the control must still be accepted. Hosted, `tests/am3/hosted/lf8/run.mjs` counts a refused row only
when all of these hold:
- index and pin are unchanged;
- the variant is the class's;
- `fails` rose since the selection;
- `lastError` carries the forgery's own text (B_k's install id, the stranger's key prefix, or "skips an index").

A row refused for another reason fails.

## Replays: the front ignores a conflicting entry at an applied index

The lead asked whether the front logged or counted the 25 replays at k with different bytes. **It ignored them
silently.** Across the 50 replays, `oks` rose from 2 to 51, one per replay, and `fails` stayed at 250. `lastError` kept
the last gap's text, and Workers Logs for the front from 00:15:30 to 00:19:35Z hold only request lines and the version
mismatch line below. `AgentsReleaseView::extend` skips any entry at or below its index without comparing bytes, so a
member, or a forger, that answers a different entry at an applied index reads as a healthy refresh. That entry is
equivocation, not staleness. OPEN-QUESTIONS A55 asks whether the view should alarm on it.

**Since fixed (2026-11-25, DECISIONS "`agents-equivocation`"; native only).** The view now keeps each applied index's
digest, and other bytes at a held index are refused as `Equivocation { index }`. That counts as a failed refresh, raises
`paged` on the front's status, and logs `PAGE graft-agents-front`. The replays in r1 ran before this fix. A later LF-8
row should expect `Equivocation` for each replay, and `run.mjs` and `lf8forge` now check for it.

## Notes

- Every forwarded webhook logged `ALARM graft-agents-front: graft-controller pinned to 03011f8a-… answered from …`,
  1,700 in the 10 minutes queried. This is expected in this bed. The pin is release 3's controller in the synthetic
  history, which does not exist, so the plain controller answered from its own version. The row judged the view
  through `/_agents_front/status` and not through forwards.
- Teardown removed both fronts, the controller, the forger, the two runner-store secrets
  (`graft-agents-lf8-AGENT_APP_KEY` and `-WEBHOOK_SECRET`) and the forge directory. Afterwards the install account had
  no `graft-lf8-` scripts, the runner account held only `graft-runners`, and both sweeps read 0 (`teardown.log`,
  `sweep-2.log`).
