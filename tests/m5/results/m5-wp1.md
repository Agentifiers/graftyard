# M5-WP1: the `Release` group, genesis, and keys (2026-11-13)

All five rows pass. BF-6 is a replay test in `crates/release`. BF-41 ran on a throwaway product install whose `Release` group was empty. BF-40, BF-42, and BF-45 ran on graft-m4a's real `Release` group after site day 4 (`tests/am1/results/install-m4a.md`). Hosted cost was $0.18.

## What was built

- **`crates/release`:**
  - Every `ReleaseEntry` variant, tags 0 to 23 in the design's order (`src/tests/entry_tags.golden`). `Genesis` (0) and `Keys` (1) are unchanged, and graft-m4a's genesis re-signs byte for byte.
  - `Keys` generations with the `drain`, `executor`, and `agents_executor` keys, plus `SiteMode`, `Mode`, `IntentId`, `SignedIntent` with its pre-signed `undo`, every `Intent` variant, and `IntentState`.
  - `ReleaseFold::decide(anchor, at, bytes)`, with the signer rules and every refusal the build line lists.
  - It is pure Rust with no clock: `at` is the entry's own time (`decisions/DECISIONS.md`, "M5-WP1: the `Release` group's rules, as built").
- **Site member (`workers/site/src/release.rs`):**
  - The `Release` group's `decide` and follower `check` run `ReleaseFold` against the script's anchor and the entry's stamped time, stored in a new `release_at` table.
  - Entries after genesis are format 0x30.2.
  - The `/read` view shows keys, the current intent, drained sites, uploads, enabled formats, and pointers, and pages entries with `{"after": n}`.
- **Edge (`workers/edge/src/release.rs`):** `POST /api/graftyard/v1/release/entries` decodes any entry and commits it through the group. The answers are 201, 409 with the current digest, or 422 with the refusal. A genesis still goes through the create path.
- **CLI:**
  - `graftyard release entry <spec.json> --prev <hex> [--keys f]` builds any entry.
  - `graftyard sign <in> --out --key [--prev <hex>]` signs any entry, and with `--prev` rebinds it after a 409.
  - `graftyard release post` sends an entry.

## Gates

| Row | Pass line | Result | Evidence |
| --- | --- | --- | --- |
| BF-6 | 100 of 100 refused | **Pass.** 154 of 154 refused: every committed entry of a history with every entry kind, plus every pre-signed undo of every finished intent (including `McpPointer` and `ControllerPointer`), replayed after `Keys { vendor: None }`. Refusals: `AlreadyInstalled` 1, `NotCurrent` 20, `StalePrev` 133. In a stronger variant, the 20 vendor-signed entries were re-signed over the current digest, and all 20 were refused `UnknownKey` | `bf6_every_historical_entry_is_refused_after_keys_vendor_none` (`crates/release/src/tests/group.rs`) |
| BF-40 | Both refused | **Pass.** An `EdgeRoll` naming a release with no `ReleaseUploaded` was refused 422 `NoUpload`. A `SiteDay` intent committed (201, index 62). A second intent while it was current was refused 422 `IntentCurrent` | graft-m4a, `raw/hosted.log` 12:08:28 |
| BF-41 | Only the matching one commits | **Pass.** Another key's genesis was refused 422 `WrongKey` at the edge. Through a second edge whose anchor names that key, all 3 of 3 members refused it at `/join` (`WrongKey`), and no group started. The matching genesis then committed (201, index 5, `76ec7cec…`). After that, the other genesis met 409 naming the matching genesis's digest, and so did the matching one | throwaway install `01M7ABN08M7193VHPXFB4METX1`, `raw/bf41.log` |
| BF-42 | 409 with the current digest; the re-signed entry commits | **Pass.** A customer `Halt` was signed offline over `d`. The executor key's `IntentStep` then committed (201, index 64, `9298816e…`). The `Halt` met 409 with `prev` = `9298816e…`, the current digest. Re-signed with `sign --prev`, it committed (201, index 66) | graft-m4a, `raw/hosted.log` 12:08:29 to 12:08:30 |
| BF-45 | Only a correctly signed `ReleaseUploaded` commits | **Pass.** All four executor-key forgeries were refused: a `ReleaseUploaded` with a bad record signature (422 `BadRecord`), a `ReplaceMember` of undrained site 1 (422 `NotLost`), an `EdgePointer` pin bound to `prev` (422 `BadBound`), and the same pin as an undo of a non-current intent (422 `NotCurrent`). The correctly signed `ReleaseUploaded` committed (201, index 60) | graft-m4a, `raw/hosted.log` 12:08:26 to 12:08:27 |

After the run, graft-m4a's group answered `current: null` and `keys.generation: 1`, with one upload (`af45659e…`), last digest `8b2f4278…`. The group was left with nothing current: an `Abort` and an `IntentDone` closed the test intent. The executor and drain keys of generation 1 are in `~/.graft-m4a/m5`, mode 0600, because m4a's genesis keys were discarded (`--offline-keys`).

## Plants

`tests/m5/plant.py` removes one refusal at a time from `crates/release/src/group.rs`. 17 of 17 plants were caught (`plant.log`). The plants cover:

- unknown keys and wrong generations;
- intents past `not_after`;
- an intent with no upload;
- unreadable formats;
- a second current intent;
- a second drain by the drain key;
- an `EdgeRoll` that drops an undo's version;
- a stale `prev`;
- an undo outside the live list, and an undo replayed;
- the executor key replacing an undrained site, signing intents, or uploading a forged record;
- a vendor intent resumed after `Keys { vendor: None }`;
- `FormatsEnable` without the release half;
- steps out of order.

## Hosted runs

**Site day 4 on graft-m4a** (announced 11:54Z, run 12:05Z to 12:09Z):

- **What was deployed:** `m5-wp1-m4a` `e6377c3`, built with `worker-build --release` (no test-faults) and deployed by `workers/site/deploy.mjs` with site day 3's image, `--vars`, `--ops`, and `GRAFTYARD_HOST`.
- **Versions:** sites a/b/c `8b3debe1` / `06555a76` / `d0d31e7e`. `graft-m4a-edge` was unchanged.
- **Before and after the deploy:** `GET /api/v3/repos/m3org/j4`, `/api/v3/user`, and `j4.git/info/refs` answered 200. After it, the genesis replay still met 409 with `b40c10ed…`.
- **Test edge:** BF-40, BF-42, and BF-45 went through `graft-m4a-m5-edge`, a product edge with m4a's anchor, deployed at 12:07Z and deleted at 12:09Z.

**BF-41's throwaway install** (`tests/m5/bf41.sh run`, 11:45Z to 11:52Z, then `delete`):

- `graftyard init` and `sites add` ×3 built `graft-m5-bf41-{a,b,c}` (enam/weur/apac) from the product bundle, with m4a's git image.
- `genesis --offline-keys` built the genesis.
- Two product edges were deployed: one with the install's anchor, and one with the other key's.
- Everything was deleted afterwards. The final listing showed no `graft-m5-bf41*` script or bucket.

## Checks

- `cargo test --workspace --release`: 795 passed, 0 failed.
- `cargo clippy` is clean for the workspace, and for `workers/site` and `workers/edge` on wasm32 with and without `test-faults`. `cargo fmt --check` is clean.
- `scripts/check_design.py`, `check_names.py`, and `check_links.py`: OK.
- CI runs only on pushes to `main` and on pull requests (`.github/workflows/ci.yml`), so the branch pushes had no checks.

## Cost

$0.18 in total, all of it containers, per `INSTALL=graft-m4a python3 tests/fanout/m2bcost.py 2026-11-13T11:45:00Z 2026-11-13T12:15:00Z`. The figures were 29 Worker requests, 4,426 DO requests, and 1,261 DO active seconds. Most of the container GB-s is m4a's warm `GitShard` instances, which run whether or not a test does. The `graft-m5-bf41` scripts show $0.00 for 11:45Z to 12:00Z.

## Not built here

- The edge's own check of non-genesis entries against a cached `Keys` is M5-WP2's cached `Release` view. Until then, `decide` is the only judge (`design/DESIGN.md`, Shape, the release endpoint).
- Reads through `GET /api/graftyard/v1/release/entries` belong to AM3.
- What `graft-ops` runs for each intent's steps is M5-WP3 onward.
