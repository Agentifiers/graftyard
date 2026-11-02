# M1-WP6: the edge for git over HTTPS and the clone cache

Run 2026-11-01 in the Milestone 0 account, against the edge deployed as `graft-m1-edge` on workers.dev. The site member does not host the `Repo` slot yet (`workers/site/src/family.rs`, `RepoSlot = Register`). So every row here runs against a stand-in repository group: three real `graftyard_raft::Member<Push>` members on a `Tri` record, inside one Durable Object of the throwaway script `graft-m1-wp6-bed`. The git cache shard is the M1-WP5 container class in that script. The edge calls both through the JSON boundary that `workers/site`'s member already serves (`/commit`, `/outcome`, `/outcome_by_request`, `/read`). Swapping the stand-in for a site member changes the `GROUP` binding and nothing else.

## What is built

| Where | What |
| --- | --- |
| `crates/git/src/pkt.rs` | pkt-line reader and writer: flush, delim, response-end, `0003` refused, length over 65,520 refused, never reads past the buffer. Side-band framing. A fuzz test over random and mutated input |
| `crates/git/src/refs.rs` | `ObjectId`, `RefName` (check-ref-format rules, 900-byte cap), `RefUpdate`, `Atomicity` |
| `crates/git/src/repo.rs` | The repository chain: `Push` as a `graftyard_raft::Command`, `RepoFold`, `RepoEvent::Refs`. `decide` checks the credential (`Principal`, `deps` from the identity folds), then write permission, then the 10,000-ref atomic cap, then duplicate refs, then each ref's `expect`. `refs/pull/` is hidden. `AllOrNothing` refuses every ref when one fails. Formats 32.1 and 32.2 (32.2 adds the client's agent). `encoded_len_without_pack` and `packs_of` |
| `crates/git/src/smart.rs` | Routes (dumb HTTP refused), the v2 advertisement with exactly github.com's capability set, the receive-pack capabilities and advertisement, receive-pack request parsing, `report_status(&Acked<PushOut>)` (the only renderer of `ok`), `Refusal` (atomic cap, updates over `COMMAND_MAX`, unpack failure, a refused command), `cap_refusal` per plan, `parse_v2`, `request_id_header` |
| `crates/git/src/clone.rs` | `CloneKey::of` (only a v2 fetch with `done`, no haves, no shallow state, and known flags), `tags_digest`, `cacheable_response` |
| `crates/git/src/group.rs` | The edge-to-member wire types and `decode_commit`, which maps an unreadable format to `Unsupported` without touching state |
| `crates/git/src/standin.rs` | `StandIn`: three members in one process, deps answered from a seeded identity, persisted as member rows |
| `crates/git/src/store.rs` | `PackStore::place_teed`: the tee to the three buckets (single put, or multipart with the `Upload` journal) concurrent with shard validation, two-of-three in two regions |
| `workers/edge` | The edge Worker. It mints one `RequestId` per client request and sends `X-GitHub-Request-Id` and `X-GitHub-Enterprise-Version` on every response. Requests without a User-Agent get 403. Authentication is a stored token. Receive-pack: answer the 4-byte `probe_rpc`, stream the body up to the edge cap, then stage, validate, place, commit with re-sends, and frame the member's report. Upload-pack: v2 advertisement from the edge, fetch through the clone cache or the shard with the output teed to the client and R2 |
| `tests/m1/wp6/bed` | `graft-m1-wp6-bed`: `Group` (the stand-in, persisted in SQLite, puts `Committed` markers after an ok outcome) and `GitShard` (the M1-WP5 container) |
| `containers/git` | The M1-WP5 image, with three changes. `/validate` reads the tips from the body. Validated packs are held aside until `/ingest` adopts them, so an uncommitted push leaves nothing a later push could lean on. Every git command runs with a fixed identity, and HEAD is written only when it changes (below) |
| `tests/m1/wp6/*.sh` | `clients.sh` (29 git client rows), `bf16.sh`, `bf17.sh`, `cap.sh`, `seed.sh`, plus `mktok.py` and the Dockerfiles of the two older clients |

Authentication on `graft-m1-edge` is a stored opaque token (`ghp_` shape, CRC-checked, SHA-256 of the secret) sent as the HTTP Basic password with any username, or as `Authorization: token|Bearer`. The edge parses the token with `graftyard_identity::OpaqueToken` and reads its row (account, revoked) from the stand-in's seeded identity. Real token rows come from the identity groups once the site hosts them. Four tokens were seeded: owner (account 100), writer with a `Write` grant (101), read-only (102), and revoked (103).

## Git clients

The clients were unmodified. git 2.56.0 is the orb's own. git 2.51.0 was built from the kernel.org tarball, checked against its published SHA-256 (`tests/m1/wp6/git251.Dockerfile`). git 2.39.5 is Debian bookworm's package (`tests/m1/wp6/git239.Dockerfile`). SHARED's `git-http` row names 2.39 and 2.51.

```
docker build -t graft-git239 -f tests/m1/wp6/git239.Dockerfile tests/m1/wp6
docker build -t graft-git251 -f tests/m1/wp6/git251.Dockerfile tests/m1/wp6
tests/m1/wp6/clients.sh graft-m1-edge.<sub>.workers.dev bed/main <writer> <read-only> <revoked>
docker run --rm --network host -e HTTPS_PROXY -v $PWD/tests/m1/wp6:/wp6:ro graft-git239 bash /wp6/clients.sh <same arguments>
```

| Row | 2.39.5 | 2.51.0 | 2.56.0 |
| --- | --- | --- | --- |
| `GIT_TRACE_PACKET=1 git ls-remote`: `version 2`, `ls-refs=unborn`, `fetch=shallow wait-for-done filter`, `server-option`, `object-format=sha1`, nothing else, no `bundle-uri` or `packfile-uris` (7 rows) | pass | pass | pass |
| `git push --atomic` of three new branches, and `ls-remote` shows them | pass | pass | pass |
| Atomic push with a stale `--force-with-lease`: refused whole | pass | pass | pass |
| The same push without `--atomic`: main moves, b refused | pass | pass | pass |
| Atomic push raced by another writer from a `pre-push` hook, so the old value git sends is stale: the edge refuses both refs and `main` stays | pass | pass | pass |
| Non-fast-forward refused | pass | pass | pass |
| Push over `http.postBuffer` (`probe_rpc`, then chunked) | pass | pass | pass |
| Delete a branch | pass | pass | pass |
| `git clone`, `fsck --strict`, at the pushed tip; full clone lists the branches | pass | pass | pass |
| `git clone --filter=blob:none` (7 blobs missing), lazy fetch on checkout, `git fetch --filter=blob:none` sees a newer push | pass | pass | pass |
| Incremental fetch, `clone --depth 1`, `protocol.version=0` clone | pass | pass | pass |
| Dumb `info/refs` 403; a read-only token's push 403; revoked and anonymous refused; no refused push wrote a ref | pass | pass | pass |
| Total | 29 / 29 | 29 / 29 | 29 / 29 |

Raw output is in `raw/m1-wp6-clients-graft-git239.txt`, `raw/m1-wp6-clients-graft-git251.txt`, and `raw/m1-wp6-clients-host-git256.txt`.

git's own `stale info` refusal happens on the client, before anything is sent. The server's `expect` check is therefore proven by the `pre-push` race row and by `crates/git/tests/push.rs`.

Not run: `GCM_TRACE=1 git clone` with no provider override (the other half of the `git-http` oracle). Git Credential Manager is not installed in this orb.

Clone cache: six clones in a row of `bed/main` each answered `x-graft-clone-cache: hit` in 0.50 to 0.56 s. The first fetch after a push misses, and its response is teed to R2 under `clone/<network>/<key>/0` in `graft-m1-wp6-a`.

### The `git conformance suite` (BM-1)

Neither SHARED nor the plan says what this is. BM-1 is "The `git` conformance suite and `git clone` / `fetch` / `push --atomic` from unmodified git 2.4x pass with one site's member class failing mid-run", and the only other mention is DESIGN's "A conformance clone, fetch, and push gate each step" for `ContainerDay`. My reading [inference] is that it means git's own smart-HTTP tests, `t/t55xx` (t5539 to t5581, run with `GIT_TEST_HTTPD`). Those tests start their own Apache with `git-http-backend` as the server under test. Pointing them at a remote host would need a patched harness, which no longer counts as unmodified git. What can be run is above: SHARED's oracle rows with three unmodified clients. BM-1's "with one site's member class failing mid-run" needs the real site members and is M1-WP8.

## BF-16 on Cloudflare (stand-in group)

`bash tests/m1/wp6/bf16.sh graft-m1-edge.<sub>.workers.dev bed/bf16 6000003 <writer>` (raw: `raw/m1-wp6-bf16.txt`). The edge build is release N+1: it knows Push format 32.2. The stand-in members are release N: their `reads` is `[32.1]`. `FORMATS_ENABLED=32.1` stands for the `Release` group's `FormatsEnable`.

| Step | Result |
| --- | --- |
| Three pushes from the N+1 edge, 32.2 not enabled | The members' receive counters went from `{32.1: 1}` to `{32.1: 4}`, with no `32.2` and no `unsupported`. All three committed. Nothing was sent early |
| Redeployed with `FORCE_FORMAT=32.9` (a format no release reads), one push | HTTP 503, `Retry-After: 5`. git printed `error: RPC failed; HTTP 503`, and no ref was reported ok. No entry carrying commands, refs unchanged. The counter `recv:unsupported` went 0 → 1 (`decode_commit` could not read the bytes) |
| Redeployed with `FORCE_FORMAT=32.2` (the format N+1 knows), one push | HTTP 503, `Retry-After: 5`, no entry, refs unchanged. `recv:32.2` 0 → 1: the members read the tag, refuse it as `Unsupported`, and touch nothing |
| Redeployed without the fault | A push commits |

Pass. The edge is left on the build without the fault.

## BF-17 on Cloudflare (stand-in group)

`bash tests/m1/wp6/bf17.sh graft-m1-edge.<sub>.workers.dev bed/big 6000002 <writer>` with git 2.56.0 (raw: `raw/m1-wp6-bf17.txt`).

| Push | Result |
| --- | --- |
| `--atomic`, 10,000 new tags, a 1 MiB random blob (pack 1,049,107 bytes) | 10,000 refs `ok` in 126 s end to end. One entry (index 37) with 10,000 ref updates, 540,210 bytes (≤ 1.5 MB), naming a stored pack (over `INLINE_MAX`, so not inline). Pack in all three buckets, `Committed` markers in two. `ls-remote` lists 10,000 tags, and a clone has 10,000 tags and fscks clean |
| `--atomic`, 10,001 tags | All 10,001 refs `!`, with `remote: Push refused: an --atomic push may update at most 10000 refs, and this one updates 10001. Split it into smaller pushes.` No new entry, ref count unchanged |
| 12,000 creates with 120-byte names, not atomic (updates encode to 1,740,092 bytes) | All 12,000 refs `!` (`push has too many ref updates, split it into smaller pushes`), with `remote: Push refused: its 12000 ref updates take 1740092 bytes, over this host's 1200000-byte limit for one push.` No new entry, refs unchanged |
| 10,001 tags without `--atomic` | All 10,001 `ok`: the cap is for atomic pushes only |

Pass. The same 10,000-tag atomic push from git 2.39.5 also committed: 10,000 `ok`, entry 49 with 570,210 bytes, a stored pack (raw: `raw/m1-wp6-bf17-git239.txt`).

Where the time goes: on the edge, the COMMAND_MAX refusal used 89 ms of CPU (wrangler tail). The client is slower. Over HTTP, git 2.56 spends 60 s of its own CPU between the advertisement and the first POST for 12,000 refs. A local `git http-backend` takes 39 to 44 s for the same push, and git 2.39.5 and 2.51.0 take 95 to 101 s. This is git's own cost for large pushes, not the edge's.

"Entry" counting: the bed's Group object is evicted during long pushes. On restore its leader resumes `led_term` and appends two `Noop`s in the same term (`crates/raft/src/member/leader.rs`, `become_leader`). The log's last index therefore moves without any push, and "no entry" counts entries with events or `request` rows. An earlier draft of `bf17.sh` compared raw last indexes and failed for that reason.

## The edge cap

`bash tests/m1/wp6/cap.sh graft-m1-edge.<sub>.workers.dev bed/main <writer>` (raw: `raw/m1-wp6-cap.txt`). An 85 MB push with three refs, against an edge cap of 80 MB on Pro with the SSH option off:

```
remote: Push refused: over 80 MB, this install's limit for one push over HTTPS.
remote: Split it into smaller pushes, or ask your admin about the Enterprise plan, which takes larger pushes and SSH.
remote: HTTPS stays the default for every other push.
error: remote unpack failed: push over 80 MB
!	HEAD:refs/heads/…/main	[remote rejected] (push over HTTPS cap, split the push)
```

All three refs were refused and no ref was written. The Enterprise and cloudflared variants are unit tests (`smart::tests`). A push over the 100 MB plan limit gets Cloudflare's 413 before the edge sees it, which M0-WP6 measured, so that row was not rerun.

## Bugs found on Cloudflare and fixed

1. **Every other fetch was 503.** The stand-in's `read` kept a `ReadGrant` across `settle`, which delivers more messages. `Member::fresh` rightly refuses a grant after the member's next step. The fix renders the read's value in the step that grants it. The raft core is right, and the bug was in the stand-in.
2. **Each shard catch-up took 5 s.** `PUT /refs` on the container spent 5,011 ms in `git symbolic-ref HEAD` on Cloudflare and 0 ms locally. With a fixed committer identity on every git command, and HEAD written only when it changes, the same call took 11 ms there. Which of the two changes removed the wait is not isolated.
3. **Large pushes got 400.** git posts a bare `0000` (`probe_rpc`, `remote-curl.c`) before streaming a body over `http.postBuffer`, and the edge parsed it as an empty request. The edge now answers it with an empty 200, and nothing reaches the group. `clients.sh` has a row for it.

## Planted bugs

Each was planted in `crates/git`, run against `cargo test -p graftyard-git`, and then restored byte for byte.

| Plant | Caught by |
| --- | --- |
| `AllOrNothing` no longer refuses the passing refs | `an_atomic_push_with_one_stale_expect_changes_nothing_and_says_ng_on_every_ref` |
| Atomic cap off by one (10,001 allowed) | `ten_thousand_refs_commit_atomically_and_ten_thousand_and_one_are_refused_whole` |
| `expect` check skipped | 4 tests, including the non-atomic, atomic, re-send, and delete/create-order tests |
| pkt-line `0003` read as an empty packet | `pkt::tests::refuses_bad_lengths_without_reading_past_the_buffer` |
| Clone cache keys a fetch without `done` | `clone::tests::a_fetch_that_depends_on_client_state_is_not_cacheable` |
| Clone cache keys a fetch with `have` lines | the same test |

## What the member host's `Repo` slot must provide

`workers/site/src/family.rs`: `pub type RepoSlot = Register` becomes a `Family` for `graftyard_git::repo::Push`, with these associated types:

- `type Ev = RepoEvent`
- `type Fold = RepoFold`
- `type C = Push`

`Push`'s serde form is base64 of `Push::to_wire`, so it satisfies `C: DeserializeOwned`. The trait methods:

- `reads()` returns `[REPO_FORMAT_V1]` until a release enables 32.2.
- `new_fold(ChainId::Repo(r))` returns `RepoFold::new(r, owner)`. The owner comes from the placement the member bootstraps from, `Source::Owner(owner)`.
- `store_fold` and `load_fold` store `RepoFold { repo, owner, refs }`, one row per ref.
- `out_json(&PushOut)` must be `PushReport::of(&acked)`, the outcomes plus base64 `report_status(&acked)`. That needs the `Acked`, not just `Out`. Today the trait hands `out_json` only `&Out`, and `host::outcome_json` holds `a: &Acked`. Either `out_json` takes `&Acked<Out>`, or the Repo family's `outcome_json` is special-cased.
- `err_json(&PushErr)` returns `RefusedWire::of`'s `{err, status}`: 401, 403, or 422.
- `read_json(fold, q)` returns the refs as `[[name, oid]]`.
- `dep` decodes `IdentityDep` folds from the `Tokens` and `Authz` leaders.

The routes the edge calls, with exactly the shapes in `crates/git/src/group.rs`:

- `POST /commit` with `{rid, cmd}`
- `POST /outcome` with `{ticket, wait_ms}`
- `POST /outcome_by_request` with `{rid, cmd}`
- `POST /read` returning `{value: [[name, oid]], index}`

Two more things the member must do:

- **Unreadable `cmd`.** A `cmd` the member cannot decode must answer `{"error": {"unsupported": {family, version}}}` and not a 400. Today the member's `req.json::<CommitReq<F>>()` fails before the command is looked at, so the host needs `decode_commit`'s mapping.
- **Markers.** After an ok outcome that names a `PackRef::Stored`, the leader must call `graftyard_git::mark_committed(buckets, network, pack, chain, acked.index)`, as the bed's `Group::mark` does.

## Design problems found (not edited)

1. DESIGN, Shape: "pub struct CloneKey { pub network: NetworkId, pub wants: Digest, pub depth: Option<u32>, pub filter: Option<FilterSpec>, pub caps: CapSet }" and "Same response bytes for the same key: protocol v2 fetch with no haves is deterministic for a fixed ref state." With `include-tag`, the response also carries annotated tags that point into the pack, so it depends on the tag refs and not only on the wants. `crates/git/src/clone.rs` adds `tags: Option<Digest>`, the digest of `refs/tags/` at the read, which the key does not have.
2. SHARED, "Refs and pushes": "pub enum RejectReason { Stale, NonFastForward, Protected(RuleViolation), TooManyRefs, Hidden }" and "`AllOrNothing` rejects every update when one fails." No reason fits a ref that would have passed but is rejected because another failed. GitHub prints `atomic push failure` for those. The code adds `RejectReason::Atomic`.
3. SHARED, "Refs and pushes": "Admission caps a push at 10,000 refs" conflicts with the table row above it: "At most 10,000 ref updates per `--atomic` push … Non-atomic pushes are not capped." The code follows the table, and BF-17's last row shows a non-atomic push of 10,001 committing. COMMAND_MAX still bounds a non-atomic push.
4. DESIGN, the write path: "`report_status` takes `&Acked`". `Acked` cannot cross a Durable Object binding, but the edge is where git's reply is written. So the member renders `report_status(&acked)` and sends the bytes, and the edge only frames them. The invariant holds, because only the member can build them. But "the edge renders" in journey 1 is really "the member renders, the edge frames". The member host's `Family::out_json` takes `&Out` and not `&Acked`, which is the hook this needs.
5. A command whose every ref is refused, or one that yields no events, still commits an entry with a `request` row (`crates/git/tests/push.rs`, `requests == 1`). That is right for idempotent re-sends. But "no entry" in BF-17's pass column holds only because the edge refuses before `commit`. A refusal inside `decide` does write an entry.
6. SHARED's `git-http` oracle has `GCM_TRACE=1 git clone`, and BM-1 names a "`git` conformance suite" that no document defines (above).

## Checks

| Check | Result |
| --- | --- |
| `cargo test -p graftyard-git -p graftyard-edge -p graft-m1-wp6-bed` | 46 lib, 10 push, and 10 shard tests: all pass |
| `cargo clippy -p graftyard-git --all-targets -- -D warnings` | clean |
| `cargo clippy -p graftyard-git -p graftyard-edge -p graft-m1-wp6-bed --target wasm32-unknown-unknown -- -D warnings` | clean |
| `cargo fmt -p graftyard-git -p graftyard-edge -p graft-m1-wp6-bed --check` | clean |
| `go vet ./...` and `gofmt -l` in `containers/git/server` | clean |
| `python3 scripts/check_design.py`, `check_names.py`, `check_links.py` | OK, OK (553 files), OK (2,272 citations) |

## Deployed and left in place

- Workers:
  - `graft-m1-edge` (workers.dev, final version `45c32c05-1efc-4006-94a3-bec4001d42b9`, secret `BED_TOKEN`, no `FORCE_FORMAT`).
  - `graft-m1-wp6-bed` (no workers.dev route).
- Container image: `graft-m1-wp6-bed-gitshard-git`, last digest `sha256:0d6112bd…8326`.
- R2 buckets: `graft-m1-wp6-a`, `graft-m1-wp6-b`, `graft-m1-wp6-c`. They hold the run's packs, markers, `Upload` journals, and clone-cache objects.
- Stand-in groups for 6000001, 6000002, and 6000003, with their test refs.
- Shard containers: torn down with `/_bed/teardown` for all three networks, alarms deleted.

## 2026-11-01 (later): BM-1's git conformance suite, git's own t/ HTTP tests

The design editor is recording that BM-1's "git conformance suite" means two things: SHARED's `git-http` and `git-protocol-v2` oracle rows (`tests/m1/wp6/clients.sh`, above) on git 2.39 and 2.51, and git's own HTTP transport tests from `t/`, pointed at a remote host. This section covers the second part. The earlier paragraph in "The `git conformance suite` (BM-1)" said that pointing t/ at a remote host needs a patched harness. That turned out to be unnecessary: nothing in `t/` is patched.

### Harness

`tests/m1/wp6/conformance/run.sh <https://edge host> [--git 2.51.0,2.56.0] [--tests "t5551 …"] [--slots bed/main,bed/bf16] [--out DIR] [--jobs N] [--tokens FILE] [--fault-cmd CMD --fault-after S --restore-cmd CMD]`

- It fetches each git version from kernel.org, checks it against the published SHA-256 (2.51.0 `60a7c225…0b62`, 2.56.0 `26c56c29…89d3`), and builds it once under `~/.graft-wp6/src`. It then runs each test script with `--verbose-log`, using `GIT_TEST_HTTPD=true` and `LIB_HTTPD_PATH` pointed at `conformance/httpd`.
- `httpd` is a shim where `lib-httpd.sh` expects Apache. It answers `-v` as Apache 2.4.99 and starts `front` on the test's port for `-k start`. `lib-httpd.sh` itself runs unmodified.
- `front/main.go` is a Go reverse proxy.
  - Before each request, it mirrors the test's own server repository (`httpd/www/<repo>`) into a hosted slot repository with `git push --mirror` through the edge, then forwards the client's smart-HTTP request to the slot.
  - After a receive-pack, it mirrors the slot back into `www/` and holds the response until that is done, so the test's server-side checks read what the host committed.
  - Auth: `/smart/` gets the writer token, or the read-only token unless the repository sets `http.receivepack`. `/auth-push/smart/` maps user@host to the writer and forbidden-user to the read-only token.
  - Every other Apache fixture answers 501 and is logged as `unmapped`: rewrites, CGIs, `/auth/`, `/dumb/`, `one_time_script`, `no_report`, `incomplete_*`, and the rest. So are repositories with hooks, server config outside the host's defaults, shallow server repositories, and SHA-256 repositories.
- `report.py` joins git's TAP output with the front's events (`events.tsv`, by test number) and assigns each test one category:
  - pass.
  - fail.
  - n/a: not ok, and during the test the front logged an `unmapped`, `server-setting` or `mirror-error` event.
  - skip: git skipped it for a missing prerequisite.
- `--fault-cmd`, `--fault-after` and `--restore-cmd` run a command partway through and another at the end, with `CONFORMANCE_HOST` set. M1-WP8 wires these to its tool for failing one site's member class. They are not wired yet, and this run used none.
- Default tests: t5539, t5541, t5542, t5545, t5548, t5549, t5550, t5551, t5557, t5581, t5601, t5616, t5700, t5702, t5703, t5812. t5550 stays in to show that dumb HTTP is refused. The other HTTP and protocol scripts in git 2.56.0's `t/` were left out:
  - t5559 reruns t5551 over HTTP/2 through Apache's mod_http2.
  - t5555 is t5551's shared include, not a test.
  - t5560, t5561 and t5562 test `git-http-backend` itself, the server program.
  - t5540 is WebDAV push, which is dumb.
  - t5563 (auth schemes), t5564 (a proxy), t5584 (429 retry) and t5732 (bundle-uri over HTTP) need Apache's auth, proxy, status-code or bundle-serving configuration, which the front can't stage on a hosted service.
  - t5704 drives a local upload-pack.
  - t5730 and t5731 use `file://` and `git://`.
  - t5810, t5811, t5813, t5814 and t5815 use other transports.

  A run of t5563, t5564, t5584 or t5732 would come out almost entirely n/a; they can be passed with `--tests`.

### Run

The command was `tests/m1/wp6/conformance/run.sh https://graft-m1-edge.<workers-subdomain>.workers.dev --git 2.56.0,2.51.0 --tokens ~/.graft-wp6/tokens.txt --out ~/.graft-wp6/conf-run3`, run as an orb service from 17:14 to 17:30Z.

- Clients: git 2.51.0 and 2.56.0, built from source by the harness.
- Host: `graft-m1-edge` version `45c32c05`, backed by the stand-in group and shard in `graft-m1-wp6-bed` version `762c77d4` (image `sha256:b7bbc1f6…b43e`, git 2.56.0). The front mirrors with the orb's `/usr/bin/git` 2.56.0.
- Slot repositories: bed/main (6000001) and bed/bf16 (6000003).
- Before the run, the old shard containers had to be killed with `/_bed/kill`. An earlier attempt (`conf-run2`) ran against containers still on the previous image, so every mirror push failed with `/repos: 404`. That run is discarded.
- The report, per-test results and events (tokens redacted) are in `raw/conformance-20261101/`.

| Script | git | run | pass | fail | n/a (hosted) | skip |
| --- | --- | --- | --- | --- | --- | --- |
| t5539-fetch-http-shallow | 2.51.0 / 2.56.0 | 8 / 8 | 6 / 6 | 1 / 1 | 1 / 1 | 0 / 0 |
| t5541-http-push-smart | 2.51.0 / 2.56.0 | 39 / 39 | 29 / 29 | 4 / 4 | 6 / 6 | 3 / 3 |
| t5542-push-http-shallow | 2.51.0 / 2.56.0 | 3 / 3 | 2 / 2 | 0 / 0 | 1 / 1 | 0 / 0 |
| t5545-push-options | 2.51.0 / 2.56.0 | 13 / 13 | 10 / 10 | 0 / 0 | 3 / 3 | 0 / 0 |
| t5548-push-porcelain | 2.51.0 / 2.56.0 | 25 / 25 | 24 / 24 | 0 / 0 | 1 / 1 | 0 / 0 |
| t5549-fetch-push-http | 2.51.0 / 2.56.0 | 3 / 3 | 0 / 1 | 0 / 0 | 3 / 2 | 0 / 0 |
| t5550-http-fetch-dumb | 2.51.0 / 2.56.0 | 55 / 63 | 17 / 18 | 8 / 8 | 30 / 37 | 0 / 0 |
| t5551-http-fetch-smart | 2.51.0 / 2.56.0 | 54 / 55 | 26 / 27 | 2 / 2 | 26 / 26 | 2 / 2 |
| t5557-http-get | 2.51.0 / 2.56.0 | 2 / 2 | 1 / 1 | 0 / 0 | 1 / 1 | 0 / 0 |
| t5581-http-curl-verbose | 2.51.0 / 2.56.0 | 2 / 2 | 1 / 1 | 0 / 0 | 1 / 1 | 0 / 0 |
| t5601-clone | 2.51.0 / 2.56.0 | 112 / 112 | 108 / 108 | 0 / 0 | 4 / 4 | 3 / 3 |
| t5616-partial-clone | 2.51.0 / 2.56.0 | 47 / 48 | 44 / 45 | 0 / 0 | 3 / 3 | 0 / 0 |
| t5700-protocol-v1 | 2.51.0 / 2.56.0 | 24 / 24 | 23 / 23 | 0 / 0 | 1 / 1 | 0 / 0 |
| t5702-protocol-v2 | 2.51.0 / 2.56.0 | 85 / 72 | 73 / 68 | 0 / 0 | 12 / 4 | 0 / 0 |
| t5703-upload-pack-ref-in-want | 2.51.0 / 2.56.0 | 26 / 26 | 21 / 21 | 0 / 0 | 5 / 5 | 0 / 0 |
| t5812-proto-disable-http | 2.51.0 / 2.56.0 | 29 / 29 | 28 / 28 | 0 / 0 | 1 / 1 | 0 / 0 |
| **total** | both | 1,051 | 825 | 30 | 196 | 16 |

Under git 2.56.0, t5702 stopped after test 73: `error: bug in the test script: test_grep requires a file to read as the last parameter`. This comes from the `one_time_script` test, which is n/a because its fixture rewrites the server's response. So 14 tests of that script did not run.

The 30 fails, 15 per version and the same 15 in both:

| Test | Count | Cause | Kind |
| --- | --- | --- | --- |
| t5541 #20 `push --atomic fails on server-side errors` | 2 | The client got `ng … fetch first` on both refs instead of `atomic transaction failed`. Detail below | **Real bug in the deployed edge** (wire decode). The member's answer is correct |
| t5551 #5 `clone http repository` | 2 | The test diffs the response headers. Cloudflare adds `alt-svc`, `cf-ray`, `nel` and `report-to`, and the edge adds `x-github-enterprise-version`, `x-github-request-id` and `x-graft-clone-cache`; `pragma` moves | Deviation of a hosted service (headers only; the clone itself works) |
| t5541 #29 `http push gives sane defaults to reflog` | 2 | It reads the reflog of the server's repository. On this host that repository is the front's mirror, so the reflog names the front's fetch (`user@172.28.0.26`), not the push | Harness limit: a hosted service's server-side reflog can't be seen |
| t5541 #35, #36 | 4 | #35 needs the clone that #34 (n/a, `/half-auth-complete/` fixture) makes. #36 then runs in the wrong directory (`ambiguous argument 'HEAD'`) | Follows from an n/a test |
| t5551 #16 | 2 | Needs the `half-auth` repository from #15 (n/a, `/auth-fetch/`) | Follows from an n/a test |
| t5539 #3 | 2 | Needs `../clone` from #2 (n/a, shallow server repository) | Follows from an n/a test |
| t5550: 8 tests, `fetch changes via http`, the submodule credential tests, `redirects are reported` | 16 | Their clones and submodule setups come from `/dumb/` and `/auth/dumb/` tests, which are n/a. Dumb HTTP is refused by design | Follows from an n/a test |

So of 1,051 tests run, 825 passed. One test shows a real defect (t5541 #20, in both versions) and one shows a header deviation (t5551 #5). The other 26 fails follow from tests the host can't stage or from server-side state it can't expose.

#### t5541 #20: a member reply the edge can't decode becomes a wrong answer

This test is a D/F swap: the server holds `refs/heads/other/conflict`, and the client pushes `atomic` and `other` atomically.

- The test's first run (the earlier partial run) found a real bug in `Push`: the fold accepted a ref whose name is a directory or file of a ref that stays. The shard's reftable refuses that, even within one transaction. The fix in `crates/git/src/repo.rs`:
  - `RejectReason::NameConflict` (`refname conflict`) for such a create. A delete plus a create in one push is allowed.
  - `RejectReason::AtomicTransaction` (`atomic transaction failed`) on every ref of an `AllOrNothing` push that hits it, as upstream receive-pack does.
  - The test `a_ref_that_is_a_directory_or_file_of_another_is_refused_as_a_name_conflict`.
  - The shard's `PUT /refs` now applies deletes and updates as two `update-ref` transactions.
- In this run, the bed carried the fix and the deployed edge (built 13:46) did not. The bed's wasm contains `NameConflict`; the edge's has no occurrence. What happened, step by step:
  - The member replied `{"Rejected":"NameConflict"}` / `AtomicTransaction`.
  - `workers/edge` `post_json` failed to decode the reply and took the lost-reply path: `/outcome_by_request` under the same id.
  - The leader found the request's row and replayed its stored events. `Push::replay` reports every ref those events didn't apply as `Stale`.
  - So git printed `fetch first` on every ref.
- Reproduced by hand on bed/bf16 (2.56.0):
  - Server holds `refs/heads/d/x`; a push of `d` and `fine` printed `fine -> fine` ok and `d -> d (fetch first)`.
  - Against the `StandIn`, the existing test `a_ref_that_is_a_directory_or_file_of_another_is_refused_as_a_name_conflict` gives `NameConflict` for this shape. A temporary unit test, since removed, gave `[Ok]` for a delete of `d/x` followed by a create of `d`.
- What the integration agent needs to do:
  - Rebuild the edge with the current `crates/git`, then rerun t5541 #20.
  - The edge should treat a reply it can't decode as `Unsupported` (503), not as a lost reply. A lost-reply fallback that replays turns any newer member's answer into `fetch first`.

#### Latency seen

Of the run's 575 forwarded requests, two took minutes. Both were `info/refs?service=git-upload-pack` on bed/main: 253 s (2.56.0 t5551 #6, the first request to that network after the shard was killed, so a cold start plus a rehydrate) and 326 s (2.51.0 t5551 #50, cause not found). Every other request took about 0.2 to 2 s. The run waited them out.

### LRU, `sweep_all`, BF-18

See `m1-wp5.md`: "The LRU of bare repositories", "BF-18 rerun after the LRU" (pass, network 5000020), and "The sweeper's cron entry".

## 2026-11-01 (integration): the real repository groups

From this pass `graft-m1-edge` runs against the site script's own members, not the stand-in in `graft-m1-wp6-bed`. Everything below ran on the test install `graft-m1-site-a/b/c` (enam, weur, apac), deployed with `test-faults` (`node workers/site/deploy.mjs tests/m1/wp1/sites.json --genesis tests/m1/wp7/genesis.json --test-faults --image <the bed's digest-pinned image>`), and on `graft-m1-edge` deployed by `tests/m1/integ/deploy-edge.mjs tests/m1/wp1/sites.json graft-m1-edge --test-faults --genesis tests/m1/wp7/genesis.json`. The raw output is in `tests/m1/integ/raw/`.

### What changed

- **The `Repo` family** (`workers/site/src/family.rs`). It does what "What the member host's `Repo` slot must provide" lists:
  - The fold is `RepoFold::new(repo, owner)`, with the owner from the join. Each member checks the owner against the repository's `RepoOwner` row on `Names` before it stores anything (`Source::Repo`).
  - The refs are stored one row per ref in `repo_ref`.
  - `out_json` takes the `Acked` and returns `PushReport::of(&acked)`.
  - `err_json` is `RefusedWire::of`.
  - `read_json` is `[[name, oid]]`.
  - `dep` decodes the identity folds.
  - An unreadable `cmd` answers `{"error":{"unsupported":{family, version}}}` with `decode_commit`'s mapping, and counts `recv:unsupported`.
  - After an ok outcome that names a stored pack, the leader calls `mark_committed` over `BUCKET_<n>`. A failed put counts in `/stats` `markerFailures`; the sweeper repairs it.
  - The four routes have the shapes in `crates/git/src/group.rs`. The leader-only `/entries?from&to` serves the shard's `ChainLog`.
- **The git cache shard** is the `GitShard` class in the site script (`workers/site/src/shard.rs`, the M1-WP5 container image). It reads its repository group through the lookup rule. Each site binds only its own shard class, because Cloudflare attaches a container config to the first binding of its class, and that binding must be the script's own (measured: a version binding another site's `GitShard` first is refused, "You cannot configure a container on a Durable Object that is defined in another Worker"). The edge binds `SHARD_<n>` for every site and uses the owner record's home.
- **The edge finds the repository by the M1-WP1 lookup rule.** `o/r` goes to the `Names` login row (on `name_shard(Login)`), then the owner's `Placement` row (on `account_shard(owner)`), then the `Authz(owner)` group's `{"repo": name}`. The repository group is created on first use from the owner's record, with `{owner}` in each join. A token is read from its `Tokens` shard (`{"token": hex}`) through `fresh`. The seeded rows and the `REPOS` variable are gone.
- **The identity rows** were created through the hosted groups by `tests/m1/integ/seed.ts`, which sends operator commands through the M1-WP7 test edge:
  - `PutAccount` for the owner 7000001 and the users 7000002 to 7000004, then `InitPlacement` of the owner (Tri).
  - `ClaimLogin wp6`, on `names:987`.
  - `Open` (User) and `CreateRepo` on `authz:7000001`. This created `wp6/main` 196608, `wp6/big` 196609, `wp6/bf16` 196610, and `wp6/k0` to `k9` 196611 to 196620.
  - `RepoOwner` for each repository.
  - `SetGrant`: Write for 7000002 and 7000004, Read for 7000003.
  - `Store` of four PATs, then `Revoke` of the fourth.
- **The commit hedge, fixed.** `graftyard_lookup::commit_hedged` follows the design's rule: it finds the leader with `/outcome_by_request`, sends the command to that one member, and on a missed receipt sends only `suspect`. The first BF-12 run on Push showed two faults, and both are fixed:
  - A member that answered `NotLeader` naming no leader sent the edge back to a full lookup (`call_opt`, up to 20 s of polling). Campaigns are request-driven, so no member campaigned in that time. The 10 groups' first kills each took 80 to 85 s and then gave up (`raw/stalled/p1.bf12.jsonl`, 10 rows, not counted).
  - The target was not kept after a send, so a later miss looked the leader up again.

  Now a `NotLeader` naming no leader is reported with `suspect` to every voter, the reported member included. The next send goes to the voter that answered that it leads, or else to the next voter. The member that answered is kept as the target. A kill's re-send then took 4.7 to 8.3 s at p50 and p99 instead of 28 s.

### Git clients on the real groups

`tests/m1/wp6/clients.sh graft-m1-edge.<workers-subdomain>.workers.dev wp6/main <writer> <read-only> <revoked>`, unchanged, on the orb's git and in the two images (`raw/clients-20261101.log`):

| Client | Rows passed |
| --- | --- |
| git 2.39.5 (`graft-git239`) | 29 / 29 |
| git 2.51.0 (`graft-git251`) | 29 / 29 |
| git 2.56.0 (orb) | 29 / 29 |

A run of the 29 rows takes 4 to 5 minutes per client, against about 2 minutes on the stand-in. Every push now crosses the regions: the leader is on enam, and the quorum is enam plus weur.

### BF-16 on the real groups

**Pass.** `tests/m1/integ/bf16.sh <host> wp6/bf16 196610 <writer>` (`raw/bf16-20261101.log`). The script is `tests/m1/wp6/bf16.sh` with one line changed: its redeploys run `wrangler deploy -c tests/m1/integ/.gen/graft-m1-edge.jsonc --var FORCE_FORMAT:<f>`, the config `deploy-edge.mjs` wrote with the site bindings. The bed's `workers/edge/wrangler.jsonc` is deleted.

| Phase | Result |
| --- | --- |
| Three pushes from the N+1 edge, 32.2 not enabled | The members' counters went `recv:32.1` 1 → 4, with no `recv:32.2` and no `recv:unsupported`. Pushes committed |
| `FORCE_FORMAT=32.9` | HTTP 503, `Retry-After: 5`. No entry, refs unchanged, no ref reported ok. `recv:unsupported` 0 → 1 |
| `FORCE_FORMAT=32.2` | HTTP 503, `Retry-After: 5`. No entry, refs unchanged, no ref reported ok. `recv:32.2` 0 → 1: the member read the tag and refused it as `Unsupported` |
| Redeployed without the fault | A push commits |

### BF-17 on the real groups

**Pass**, 12 of 12 rows. `tests/m1/wp6/bf17.sh <host> wp6/big 196609 <writer>`, unchanged (`raw/bf17-20261101.log`).

| Push | Result |
| --- | --- |
| `--atomic`, 10,000 new tags, 1 MiB random blob (pack 1,049,105 bytes) | 10,000 refs `ok` in 49.7 s. One entry (index 3) with 10,000 ref updates, 540,210 bytes (≤ 1.5 MB), naming a stored pack. The pack is in all three buckets. `Committed` markers are in buckets 0 and 2, which are the pack's `sites` (5 = {0, 2}, the two-of-three in two regions), so the marker is in every bucket the pack names. `ls-remote` lists 10,000 tags. A clone has 10,000 tags and passes fsck |
| `--atomic`, 10,001 tags | All 10,001 refused, "an --atomic push may update at most 10000 refs". No entry, refs unchanged |
| 12,000 non-atomic creates, 1,740,092 bytes of updates | All 12,000 `ng` in 185 s, "over this host's 1200000-byte limit for one push". No entry, refs unchanged |
| 10,001 tags without `--atomic` | Commits, 10,001 ok |

### BF-12 on `Push` commands (regression, 1,000 kills)

**Pass.** `node tests/m1/integ/bf12.ts --run p2 --kills 1000` (`raw/p2.bf12.jsonl`, `raw/p2.bf12-report.json`). Ten repositories, `wp6/k0` to `k9`, ran 100 kills each in parallel.

- **The command.** Each kill is one `Push` of a single ref update with no pack. It goes through `/_bed/push`, under the writer's token, through the edge's own commit path (`commit_hedged`). The ref `refs/heads/bf12/p2` moves along a chain of object ids, each push expecting the previous one. So a push applied twice would be refused `Stale` and change the outcome, and a lost push would leave the ref behind.
- **The kill.** It rides the first send only. The modes alternate `after_quorum` and `after_quorum_unsynced`. Every 25th kill also holds the object down for 12 s.

| | Count |
| --- | --- |
| Kills sent / landed (one `fault` row, on the leader) | 1,000 / 1,000 (0 landed twice) |
| Re-send returned the original outcome (same index, same per-ref result) | 1,000 |
| Changed outcome / no outcome | 0 / 0 |
| Per-ref result `Ok` | 1,000 |
| Double applies (request ids at two log indexes, plus `dup` rows), over all 30 members | 0 |
| Members whose request ids equal their `request` rows / whose `applied` equals `commit` | 30 / 30 |
| Refs at the chain's last id | 10 / 10 |

| Mode | Kills | Re-send p50 / p99 | Answered by another leader |
| --- | --- | --- | --- |
| `after_quorum` | 480 | 4,863 / 12,492 ms | 0 |
| `after_quorum_unsynced` | 480 | 4,721 / 8,398 ms | 0 |
| `after_quorum` + 12 s down | 20 | 16,354 / 19,434 ms | 4 |
| `after_quorum_unsynced` + 12 s down | 20 | 16,479 / 17,750 ms | 0 |

### Deployed and left in place

- `graft-m1-site-a/b/c`: this pass's site script with `test-faults`. It has the `GitShard` container class (migration `v2`) and the buckets `graft-m1-packs-a/b/c`, which this pass created.
- `graft-m1-edge`: the git edge on the real groups, with `test-faults`, secret `BED_TOKEN`, and no `FORCE_FORMAT`.
- `graft-m1-wp6-bed` and its buckets `graft-m1-wp6-a/b/c` were not changed. Nothing binds the bed any more. `workers/edge/wrangler.jsonc`, the bed edge's config, is deleted, and `tests/m1/integ/deploy-edge.mjs` deploys the edge.
- After the runs, every shard this pass started was torn down with `/_bed/teardown`, for repositories 196608 to 196620.
- The identity rows and tokens that `seed.ts` wrote are in `~/.graft-integ/` on the orb: `tokens.txt` and `repos.json`. They are not committed.

## 2026-11-02 (M1-WP8): t5541 #20 fixed

The two defects behind t5541 #20 ("t5541 #20: a member reply the edge can't decode becomes a wrong answer") are fixed:

- **The replay.** `Push::replay` reported every ref the request row's events had not applied as `Stale`, because the row stored no record of refused refs. `decide` now also emits `RepoEvent::Refused { push, refs }` (event tag 1), which applies nothing, and replay answers each ref with its stored reason. Test: `a_replay_reports_each_refused_refs_own_reason`, which fails on the old code.
- **The edge.** It is rebuilt on the current `crates/git`. `commit_hedged` now returns `Unsupported` (503 with `Retry-After`) for any 200 member answer it cannot read, instead of sending again under the same id.

On Cloudflare, t5541 on git 2.56.0 with no fault: 30 of 39 pass, and `ok 20 - push --atomic fails on server-side errors`. BM-1's runs under a failing site are in `m1-wp8.md`.

Under BM-1's fault, a later run found that the git cache shard could not rehydrate a slot whose log holds many inline packs. The member's `/entries` answered 4.6 MB in one page, past the lookup's 3.5 s call deadline. `/entries` now pages by stored bytes (`m1-wp8.md`, fix 4).
