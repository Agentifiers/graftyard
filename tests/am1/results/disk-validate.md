# disk-validate: a push validates on its shard's disk as it stands

Branch `disk-validate` off main 04577c42. The B0-32 fix the parent asked for after `lease-clock`'s analysis: validate against the shard's current disk, and catch up only when a base is missing (`decisions/DECISIONS.md`, "`disk-validate`"). No hosted spend on this branch.

## Why site a's validates ran p50 5.4 s (m3fr5c)

From `tests/am1/results/lease-clock.md`, "B0-32". k was 4. Each push forwarded to a pool shard waited out the network shard's catch-up, then the pool shard's. `Jobs::wait` waits for the run in progress and then a new one, so each wait covered 1.4 to 1.9 runs. Every shard ingested every burst pack, about 564 packs and 145 MB each, and runs grew to 4 to 9.5 s. The container's own work after a catch-up was p50 0.36 to 0.9 s.

## What the branch changes

- **Shard (`workers/site/src/shard.rs`, `"/validate"`).** Once the object holds a cursor for its running container (`graftyard_git::validate_on`), a push skips the catch-up wait. The network shard forwards at once, and each shard starts a catch-up in the background. The archive's promotion (`derived`), and a container with no cursor, catch up first as before.
- **Container (`containers/git/server/main.go`, `/validate?disk=1`).** A missing base answers 409 `{"missingBase": true, "held": "held-<n>"}`. The container keeps the request, spooled pack and head included, under `/data/held/`, because the body streamed once. Missing base means:
  - `index-pack`'s "unresolved delta" or "did not receive expected object";
  - the connectivity walk's "bad object", "bad tree object", "missing … object" or "Could not read";
  - an update's old tip that is not the disk's ref;
  - a new commit that no disk ref reaches and the pack does not hold (`behind`).

  Every other failure stays a 422: a truncated pack's "early EOF", an fsck error.
- **Retry.** The shard waits for a catch-up that started after the push arrived (`graftyard_git::CatchUps`), not for the run in progress and then another. It then asks `/validate?held=<id>` with no body. That call is checked as a caught-up validate is, so any failure is the pack's 422. A held request is answered once, and one left unclaimed for an hour is dropped.

git 2.56's wording was probed on this orb (`/tmp/mb`, not kept):

| Case | git says |
| --- | --- |
| Thin pack, delta base absent | `fatal: pack has 1 unresolved delta` |
| Tree or parent named, absent (`--strict`) | `fatal: did not receive expected object <oid>` |
| Walk reaches an absent commit | `fatal: bad object <oid>` |
| Walk reaches an absent blob or tree | `fatal: missing blob object '<oid>'`, `fatal: bad tree object <oid>` |
| Truncated pack | `fatal: early EOF` (stays 422) |
| Malformed commit | `fsck error in packed object` (stays 422) |

## Why a push accepted on a disk that is behind is safe

A push is accepted only if every base it names is present on the disk it was checked against:
- The disk holds only committed packs. `/ingest` and catch-ups add them. A validated pack waits under `graft/validated-*`, outside the alternates, until its commit. Nothing prunes: repack is `-d --geometric`.
- The disk's refs name only whole tips, because `applyRefs` skips a ref whose object is absent.
- So `index-pack --fix-thin` resolves a delta only against a committed object, and `rev-list --objects --not --all` either stops at a whole tip or fails on the first absent object, which is a missing base.
- Disks only grow, so both answers hold on any later disk.
- The detail is taken only when each update's old tip is the disk's ref and every new commit is in the pack or reachable from a disk ref.
- `decide` still checks every ref's expected old value at the leader.

A newer ref elsewhere can make the merge base earlier than the leader's. That widens `changed` and `protected`; it never narrows them.

## Replay

`crates/git/tests/disk_validate.rs` replays the m3fr5c burst: arrivals, shard and work times per push, from `b032_m3fr5c.csv`, 651 rows. The model:
- one catch-up at a time per shard, costing 412 ms plus 37 ms per pack, the fit to the run's 182 catch-ups;
- every ack becomes a pack 700 ms later;
- main moves every 6 s [guess].

It is an inference, not a measurement of the fix.

| Order | p50 | p90 | p99 | Within 5 s | Held |
| --- | --- | --- | --- | --- | --- |
| main (catch up first) | 7.7 s | 14.9 s | n/a | 237 of 651 | n/a |
| disk as it stands | 1.0 s | 6.9 s | 17.4 s | 579 of 651 | 98 |

The test asserts disk p50 ≤ 1.2 s and main p50 ≥ 4 s. It also asserts that a held push is never checked again before a catch-up that read its base. On main it does not compile (`CatchUps` and `validate_on` do not exist there), and its main-order replay gives p50 7.7 s.

Not adopted: also releasing a held push when the run in progress turned out to cover its base. It held 174 pushes against 98, at the same p50.

The p90 and p99 are still set by pushes whose base is missing behind a long catch-up. B0-32's line is every push acked within 5 s or answered `admission full`, so this branch may still fail it on p99. The hosted rerun decides.

## Checks

- `go test ./...` in `containers/git/server`: `TestValidateOnTheDiskAsItStands`, `TestMissingBaseWording`.
- `cargo test -p graftyard-git --test disk_validate`: 3 tests.
- `tests/m0carried/plant16.py`: 13 of 13 caught.
- Full checks on the branch (`fmt`, workspace clippy, wasm32 clippy for the site and edge with and without `test-faults` and for ops, `cargo test --workspace`, the three design checks, and the shard's Go tests): every step exit 0, 973 tests passed, 0 failed.
