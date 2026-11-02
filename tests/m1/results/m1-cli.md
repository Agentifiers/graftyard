# M1 operator CLI and genesis acceptance (`cli/`, `crates/release`)

Run 2026-11-01. This covers the CLI half of M1-WP1 (`graftyard init`, `sites add`, `sites add-spare`, `placement set`) and the genesis half of M1-WP7 (`graftyard genesis`, `sign`, `install --genesis`, and the `Release` group's acceptance rule). `crates/release` is pure Rust and builds for `wasm32-unknown-unknown`, so the member host runs the same acceptance code the CLI runs. Neither has a gate row of its own. The CLI writes the placement records that BF-10 and BF-11 route through.

## What was built

- **`crates/release`** (`graftyard-release`). It holds `InstallId`, `KeyId` (an Ed25519 public key; the design gives `KeyId` no other form), `Signature`, `IntentId`, `Keys`, `Genesis` (with `RestoreFrom` and `RestoreSource`), `Bound`, and `ReleaseEntry`. Only the `Genesis` and `Keys` variants are built, with tags 0 and 1 in the design's declaration order. It also holds `SignedEntry`, `Anchor`, `Refusal`, `accept_genesis`, `check_genesis`, and `ReleaseFold` with `Accepted`. Ed25519 comes from `ed25519-dalek` 2.2 with `default-features = false`, which builds for wasm32 with no `getrandom`. Verification uses `verify_strict`.
- **`cli/`** (binary `graftyard`). It holds `init`, `sites add`, `sites add-spare`, `placement set`, `genesis`, `sign`, and `install --genesis`. Every Cloudflare call goes through the API with `CLOUDFLARE_API_TOKEN`. The Tools paragraph allows "Wrangler ... with the customer's deploy token". The CLI calls the same upload API Wrangler uses, so it needs no Node. `GRAFTYARD_CF_API` overrides the base URL so a stub can stand in.

## Canonical bytes of a signed entry

```text
b"graftyard/release/1"
Bound:        install u128 | group str | prev Option<[u8;32]> | parent Option<u128>
ReleaseEntry: tag u8 | fields                       (Genesis = 0, Keys = 1)
sigs:         count u32 | (key [u8;32], sig [u8;64])*
```

The encoding follows `crates/raft`'s `Enc` rule: big-endian fixed-width integers, `u32`-length-prefixed strings and sequences, an `Option` as 0 or 1 and then the value, a `bool` as 0 or 1, and an enum as a `u8` tag. A `PlacementRecord` encodes to the same bytes `crates/identity` writes. Each signature covers every byte before the signature count. An entry's digest, which `Bound.prev` and the 201 name, is SHA-256 of the whole stored form, signatures included. `decode` is strict and refuses each of these:

- an unknown tag;
- a `bool` or `Option` byte other than 0 or 1;
- a count past its bound;
- a short read;
- invalid UTF-8;
- trailing bytes.

Because of that, decoding and then re-encoding gives back the same bytes.

## API for the member host (`crates/raft`, `workers/site` owner)

```rust
use graftyard_release::{Anchor, ReleaseFold, Accepted, Refusal, Genesis, accept_genesis};

// The anchor, from the script's own version (the variables `graftyard sites add` sets on every site script).
let anchor = Anchor::from_vars(&env.var("GRAFTYARD_INSTALL")?, &env.var("GRAFTYARD_CUSTOMER_KEY")?)?;

// Pure, anchor-only: what an edge checks before `commit`, and what `graftyard install` checks locally.
let g: Result<Genesis, Refusal> = accept_genesis(&anchor, &bytes);

// The Release group's fold. `decide` adds the one stateful rule: once a genesis is held, any genesis
// (a replay or a second one) is `AlreadyInstalled(digest)`, and anything else is `NotGenesis` until M5.
let ev: Accepted = fold.decide(&anchor, &bytes)?;   // ev.bytes: canonical stored bytes; ev.digest()
fold.check(&anchor, &ev)?;                          // a follower's verify, before apply
fold.apply(&ev);                                    // sets genesis, keys (generation 0), and last digest
fold.genesis.as_ref().unwrap().placement(SystemChain::Names)   // the system chains' records
```

`Accepted` is the one event. Its `Record::encode` should be `bytes` itself, so every member digests the same bytes. `Refusal::variant()` gives the 422 `refusal` names: `Malformed`, `NotGenesis`, `AlreadyInstalled`, `WrongGroup`, `WrongInstall`, `BadBound`, `WrongKey`, `BadSignature`, `KeysMismatch`, `KeysGeneration`, and `BadGenesis`.

`check_genesis` refuses, in this order:

1. A body that is not a genesis.
2. A `Bound.group` other than `release`.
3. A `Bound.install` that is not the anchor's.
4. A `Genesis.install` that differs from the bound's.
5. Any `prev` or `parent`.
6. Anything other than exactly one signature by the anchor's customer key, or a signature that fails `verify_strict`.
7. `Keys.customer` other than the anchor's key, or `Keys.generation` other than 0.
8. A body that fails these checks:
   - every key is a valid point and holds one role;
   - there is no restore genesis (M7);
   - site ids are below 64, and ids, scripts, and buckets are unique;
   - there is at least one active site;
   - each of `Release`, `Names`, `Tokens`, and `Ids` has exactly one record;
   - no record has `moving_from`;
   - every record builds with `Placement::from_record`;
   - voters are on known, non-spare sites in distinct regions;
   - a record has two Full voters, or is `Solo` under `evaluation`.

`Placement::new`'s round-trip rule is not re-run here, because a member has no `Rtt` rows for a genesis. `graftyard genesis` runs it with the recorded rows before it writes the file.

## What the edge's release endpoint must accept

```http
POST /api/graftyard/v1/release/entries HTTP/1.1
Host: <zone>
Content-Type: application/octet-stream
Accept: application/json
Content-Length: <n>

<the stored bytes above, exactly as `graftyard sign` wrote them>
```

There is no `Authorization` header, because the signatures are the credential. The edge checks the bytes with `accept_genesis`, or with `check_genesis` against its cached view for later entries, and then commits. Its answers:

- **201** `{"index": n, "digest": "<64 hex>"}` after `Acked`. The CLI checks that the digest equals its own SHA-256 of the bytes.
- **409** `{"message": "stale prev", "prev": "<hex>", "index": n}` when the group already holds a genesis. Here `prev` is the digest of the genesis it holds. The CLI treats `prev ==` its own digest as "already installed", which makes a retry after a lost 201 safe. Any other `prev` is "another genesis is held".
- **422** `{"message": "...", "refusal": "<Refusal::variant()>"}` when the edge or `decide` refuses.

Tests: `genesis::tests::the_entry_is_posted_as_its_stored_bytes_with_no_token` reads the method, path, headers, and body off a localhost socket. `genesis_sign_install_then_replay_and_a_second_genesis` runs `install` against a stub edge built on `ReleaseFold`. The design defines 409 only for a stale `prev`. Using it for a genesis into an installed group is this work's reading, flagged below.

## Commands and real output (test account, throwaway `graft-m1-cli-*` names)

```text
$ graftyard init --account $CLOUDFLARE_ACCOUNT_ID --zone git.m1cli.example --customer-key customer.pub
install 01M6BATGS9NK98BGF7WAZ6HMA7: wrote install.toml (customer key 858d674e…01f2); created nothing
$ graftyard init ...           # again
graftyard: install.toml exists: the install id is minted once, and the anchor never changes

$ graftyard sites add --script graft-m1-cli-a --region enam
  bucket graft-m1-cli-a-packs: location ENAM
  phase 1: deployed graft-m1-cli-a binding [graft-m1-cli-a]
  phase 2: graft-m1-cli-a unchanged, no new version
  latency check: fewer than three sites, so no pair is measured yet
recorded site 0 graft-m1-cli-a (enam) bucket graft-m1-cli-a-packs
$ graftyard sites add --script graft-m1-cli-b --region weur
  bucket graft-m1-cli-b-packs: location WEUR
  phase 1: deployed graft-m1-cli-b binding [graft-m1-cli-b]
  phase 2: deployed graft-m1-cli-b binding [graft-m1-cli-a, graft-m1-cli-b]
  latency check: fewer than three sites, so no pair is measured yet
  phase 2: deployed graft-m1-cli-a binding [graft-m1-cli-a, graft-m1-cli-b]
$ graftyard sites add --script graft-m1-cli-c --region apac          # 29 s
  bucket graft-m1-cli-c-packs: location APAC
  phase 1: deployed graft-m1-cli-c binding [graft-m1-cli-c]
  phase 2: deployed graft-m1-cli-c binding [graft-m1-cli-a, graft-m1-cli-b, graft-m1-cli-c]
  rtt 2 -> 0: 226 ms
  rtt 2 -> 1: 241 ms
  phase 2: deployed graft-m1-cli-a binding [graft-m1-cli-a, graft-m1-cli-b, graft-m1-cli-c]
  phase 2: deployed graft-m1-cli-b binding [graft-m1-cli-a, graft-m1-cli-b, graft-m1-cli-c]
  rtt 0 -> 1: 112 ms
  rtt 0 -> 2: 172 ms
$ graftyard sites add-spare --regions eeur,wnam --prefix graft-m1-cli-spare      # 32 s
  bucket graft-m1-cli-spare-eeur-packs: location EEUR
  bucket graft-m1-cli-spare-wnam-packs: location WNAM
  phase 1: … phase 2: … (each spare, then a, b, c, binding all five)
  rtt 3 -> 0: 116 ms    rtt 3 -> 2: 241 ms    rtt 4 -> 1: 168 ms    rtt 4 -> 2: 136 ms
```

- **The deployed bindings.** `graft-m1-cli-a`'s settings read back `MEMBER_0` and `REGISTRY_0` with no `script_name`, `BUCKET_0`, and the plain-text `SITE_ID`, `GRAFTYARD_SITES`, `GRAFTYARD_INSTALL`, `GRAFTYARD_CUSTOMER_KEY`, and `GRAFTYARD_BUNDLE`. Its `workers.dev` route is `{"enabled":false}`.
- **The latency check.** The CLI uploads a throwaway probe Worker, `<script>-probe`, bound to every site's `Member` class. The probe sends one `/join` to a member object `repo:0#<generation>#f0`, created with the measuring site's hint. That member measures 20 binding round trips to each peer with the site script's own code (`workers/site/src/member.rs`). The CLI then deletes the probe. Chain `repo:0` is used because id 0 is never assigned.
- **Which pairs are measured.** A spare is measured against the active sites of other continents only (eeur against enam and apac, but not weur), because it stands in for a lost site of its own continent.
- **The retry.** A pair under 30 ms is measured once more from a new member generation and then refuses the site. Tests `sites::tests::a_pair_under_min_pair_rtt_is_measured_again_with_a_new_generation_then_refused` and `a_retry_that_reads_far_enough_passes` cover this path with a fake member. The live run had no misplaced member to show it on.

**Idempotence.** I reran `sites add` for a, b, and c. Every script reported "unchanged, no new version". The script list's `modified_on` values and the version counts (a 3, b 3, c 2) were the same before and after, and `install.toml` was byte-identical (`diff` printed nothing).

**Misplaced bucket.** `sites add --script graft-m1-cli-d --region oc --jurisdiction eu` printed `bucket graft-m1-cli-d-packs: location EEUR, wanted OC; deleting it while empty` and then `refused: … inside a jurisdiction the location hint is not honoured`. `install.toml` was unchanged and no `graft-m1-cli-d*` script or bucket remained. On the first try R2 answered the delete with HTTP 500 (`10001 internal error`). The CLI now retries 5xx answers on deletes, and that bucket was deleted by hand right away.

**Placement set.** These ran against the real `install.toml` and its measured rows:

```text
$ graftyard placement set default tri --sites A,B,C
placement default Tri on sites [0, 1, 2] home 0 passed Placement::new's checks
no hosted Names group yet: wrote the request for the Names leader on names:24 to placement-default.request.json
$ graftyard placement set default tri --sites A,B,A        → refused: site SiteId(0) named twice
$ graftyard placement set 42 solo --site A                 → refused: a Solo placement needs the install's signed evaluation flag
$ graftyard placement set 42 tri --sites A,B,D             → "D" is spare site 3 (graft-m1-cli-spare-eeur): a spare holds no owner
$ graftyard placement set eval-org solo --site A           → owner "eval-org": … resolving a login needs the hosted Names group (R14)
(copy of install.toml with site C set to enam) placement set default tri --sites A,B,C → refused: sites SiteId(0) and SiteId(2) are in one region
```

The request file holds `chain`, `op: InitPlacement`, `owner`, `record`, and `command_hex`, which is the `NamesCmd` bytes through `crates/identity`'s `Record::encode`. It also holds a note that no `Names` group is hosted yet. The task asked for a signed request. The file is not signed, because `Names` commands carry a `CredentialShard` and no signature, and the design names no operator credential for `placement set`.

**Genesis, sign, install.** `install` ran against a local stub edge (`stub.py`, deleted afterwards), because no edge exists yet.

```text
$ graftyard genesis --out genesis.unsigned --release-key release.pub --vendor-key vendor.pub
  graftyard-drain-0: public c00c6c50… written to Secrets Store 519408f7…; no copy kept
  graftyard-executor-0: public 0ca0efae… written to Secrets Store 519408f7…; no copy kept
wrote unsigned genesis genesis.unsigned (859 bytes): install 01M6BATGS9NK98BGF7WAZ6HMA7, 5 sites, system chains Tri on [0, 1, 2]
not created: graft-front, the edge, graft-ops, and the Workflows script (no M1 bundle exists for them); …
$ graftyard sign genesis.unsigned --out genesis.signed --key wrong.key
graftyard: wrong.key is key e37a9881…, but the genesis names customer key 858d674e…
$ graftyard sign genesis.unsigned --out genesis.signed --key customer.key
(prints install, account, every key, every site, every system-chain record)
signed: wrote genesis.signed (955 bytes), digest 69eb5109…72cd
$ graftyard install --genesis tampered.signed --edge http://127.0.0.1:18788    # byte 100 bit 0 flipped
graftyard: refused locally (BadSignature): the customer key's signature does not verify
$ graftyard install --genesis genesis.unsigned --edge …     → refused locally (WrongKey)
$ graftyard install --genesis genesis.signed --edge http://127.0.0.1:18789  → Connection refused (nothing written to install.toml)
$ graftyard install --genesis genesis.signed --edge http://127.0.0.1:18788
installed: the Release group committed genesis 69eb5109…72cd at index 3
$ graftyard install --genesis genesis.signed --edge …       (replay)
already installed: the Release group holds this genesis (69eb5109…72cd) and is at index 3
stub log: POST /api/graftyard/v1/release/entries content-type=application/octet-stream authorization=None bytes=955 sha256=69eb5109…72cd   (twice)
$ graftyard sites add --script graft-m1-cli-e --region oc   → the install has a genesis: … only a site day may ship (AGENTS.md invariants) …
$ graftyard genesis --out g2 …                              → this install already has a genesis
```

The Secrets Store held `graftyard-drain-0` and `graftyard-executor-0`, both scoped to `workers`.

## Tests

`cargo test -p graftyard -p graftyard-release` runs 31 tests, all passing.

`crates/release` (13):

- every byte of a signed genesis, with one bit flipped (byte i flips bit i % 8), is refused;
- truncated, extended, JSON, and huge-count bytes are `Malformed`;
- signatures that fail: another key's signature, another key's self-consistent genesis, a second extra signature, and a signature lifted from another entry (`BadSignature`);
- an anchor-signed genesis that names another customer key (`KeysMismatch`);
- another install, in the bound or in the body (`WrongInstall`);
- a wrong group, a `prev`, or a `parent`;
- an unsigned entry, or a `Keys` body sent to an empty group;
- the body rules, including `Solo` without and with `evaluation`;
- `ReleaseFold`: one genesis accepted, then the same bytes and a second valid genesis both refused with `AlreadyInstalled`, and the follower's `check` refuses the event again after `apply`;
- signing is deterministic.

`cli` (18):

- install.toml round-trips;
- key files parse in hex and in OpenSSL PEM, against the RFC 8032 test-1 vector re-encoded by `openssl pkey`;
- `placement set` refuses two sites in one region, a repeated site, a pair under `MIN_PAIR_RTT`, an unmeasured pair, `Solo` without the flag (and accepts it with the flag), a spare, an unknown site, and a Witness home;
- the latency-check retry and refusal;
- spare pair selection;
- `plan` refuses a changed site and plans nothing on a rerun;
- the binding comparison;
- the exact HTTP request;
- the 409 and 422 answers;
- genesis, sign, install, replay, and second genesis end to end against the stub;
- `sign` refuses another key, and the edge and `install` refuse another install's genesis;
- `genesis` refuses system chains it cannot place.

```text
$ cargo test -p graftyard -p graftyard-release
test result: ok. 18 passed; 0 failed …   (graftyard)
test result: ok. 13 passed; 0 failed …   (graftyard-release, 7.25 s)
$ cargo clippy -p graftyard -p graftyard-release --all-targets -- -D warnings
    Finished `dev` profile …
$ cargo fmt -p graftyard -p graftyard-release --check      (clean)
$ cargo build -p graftyard-release --target wasm32-unknown-unknown --release
    Finished `release` profile [optimized] …
```

## Resources created and deleted

All of these are in the M0 test account and all are deleted. A final listing shows no `graft-m1-cli*` script, no such bucket in the default, `eu`, or `us` jurisdictions, and no Secrets Store.

- **Scripts:** `graft-m1-cli-a`, `-b`, `-c`, `-spare-eeur`, and `-spare-wnam`, plus the probes `graft-m1-cli-c-probe` and `graft-m1-cli-spare-eeur-probe`, which the CLI deleted itself.
- **Buckets:** `graft-m1-cli-{a,b,c,spare-eeur,spare-wnam}-packs`, and `graft-m1-cli-d-packs` (`eu`), which the CLI refused and deleted. A permission check created and deleted `graft-m1-cli-permcheck`, and a jurisdiction-hint check created and deleted `graft-m1-cli-j-{weur,eeur}` (`eu`), `graft-m1-cli-j-wnam` (`us`), and `graft-m1-cli-j-oc`.
- **Secrets Store:** `graft-ops-01m6batgs9nk98bgf7waz6hma7`, holding `graftyard-drain-0` and `graftyard-executor-0`, plus the permission-check store `graft-m1-cli-permcheck`.

## Open questions

- **R13** ("how does Milestone 1 create the `Names`, `Tokens`, and `Ids` groups, whose placement records live in the signed `Genesis`"): **partly answered in code.** The signed genesis now carries the four system-chain records, and `ReleaseFold` accepts only a genesis matching the anchor. Still open:
  - hosting `ReleaseFold` in a member group, which belongs to the raft and site owner;
  - the edge endpoint;
  - the step after acceptance that "creates the `Names`, `Tokens`, and `Ids` groups and the `Authz(Genesis.account)` chain".
- **R14** (how the gate's accounts, repositories, and tokens are created): **open.** `placement set` takes `default` or a numeric account id, because a login resolves only through a hosted `Names` group. The design names no operator credential for a `Names` command, so the emitted request is unsigned.
- **R15** (may an owner be `Solo` without the `evaluation` flag): **enforced as "no"**, as `Placement::solo` requires. `graftyard placement set 42 solo` refuses without the flag. The flag comes from `graftyard init --evaluation` and is signed into `Genesis.evaluation`. The Usage line `graftyard placement set eval-org solo --site A` therefore works only on an evaluation install.

## Design problems found (quoted, not edited)

1. **The bucket location hint is ignored inside a jurisdiction.** "Install operations", Placement failures: "A misplaced bucket is deleted while still empty and recreated under a new name". Measured on 2026-11-01: an `eu` bucket hinted `weur` or `oc` came back `EEUR`, and a `us` bucket hinted `wnam` came back `ENAM`. Recreating under a new name cannot fix this. Shape says DO jurisdictions "stay wherever residency allows", but a residency site (D = wnam with `us`, or a weur `eu` site) cannot get its own-region bucket. The design needs a rule here: accept any location inside the jurisdiction, or give up the per-site bucket region under a jurisdiction.
2. **409 for a second genesis.** The endpoint text says "409 returns `{"message": "stale prev", …}` when `prev` is not the current digest". A genesis has `prev: None`, and an installed group's current digest is that genesis's own. This work answers a replay or a second genesis with 409 and `prev` set to the held genesis's digest, so a retry after a lost 201 can tell it succeeded. The design should say this, or name a 422 `AlreadyInstalled`.
3. **`KeyId` has no definition.** `Keys` names keys by `KeyId`, while `LifeboatAdd` and `SiteLink::Https` use `Ed25519Public`. This work takes `KeyId` to be the 32-byte public key itself. A digest-style id would need the key bytes somewhere else in `Keys`.
4. **Genesis step 3 cannot run in M1.** The step reads "`graftyard genesis` creates `graft-front`, the edge, `graft-ops`, and the Workflows script". None of those scripts exist in Milestone 1. `genesis` makes the keys and the unsigned entry, and says it did not create them.
5. **Same-region sites.** "Two voters in one region" is refused, but `sites add-spare --regions eeur,wnam,oc` next to residency cells D = wnam, E = eeur, and F = oc puts a spare in an active site's region. `sites add` allows it, and `Placement::new` and the latency-check pair selection (no pair is measured within one region) keep such sites out of one group. Whether a spare may share a region with an active site is unstated.
6. **Scope cuts.** The `--link https --account` path, which is the account-separated option, is refused: its signed-HTTPS ring transport and `Keys.ring` are not built in M1. `sites add` after genesis is refused, because a new binding on existing scripts must ship as a site day (AGENTS.md invariants).

## Assumptions

- The install account id is `DbId(1)` (cell 0, block 0, offset 1). `Ids` leases from block 1, so no user ever takes it.
- System chains default to the first three active sites, as `Tri` when they are enam, weur, and apac, and as `Na3` otherwise. `--sites` overrides this. On an evaluation install they are `Solo`.
- Each drain and executor private key is stored in hex as Secrets Store slot 0, named `graftyard-drain-0` and `graftyard-executor-0`, in a store named `graft-ops-<install>`. The design names neither the slots nor the store.
- `GRAFTYARD_BUNDLE`, the SHA-256 of the site bundle, is an extra plain-text variable. It is how a rerun tells the deployed code is unchanged.
