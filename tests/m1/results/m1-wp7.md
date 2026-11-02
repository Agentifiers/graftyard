# M1-WP7: minimal identity and owner placement records (pure crate)

Run 2026-11-01 on `crates/identity` (`graftyard-identity`). The crate is pure Rust with no Workers dependency, clock, or randomness of its own. Each group is a `graftyard_raft::Command` with its fold. Nothing hosts it yet, so BF-10, BF-11, and BF-15 below ran against real `graftyard_raft::Member`s driven by hand, not on the real stack. The gate rows still need the hosted code (see "What remains for hosting").

This package adds to `crates/types`: `DbId::{pack, cell, block, offset}` and the layout constants, `CredentialShard` and `CredentialKey` (new module `credential.rs`, only the `Token` key for now), `Placement::solo`, and re-exports of `TokenShard` and `NameShard`.

## Public API

- **Ids.**
  - `IdBlock { base, len }`. It is built only by `IdBlock::new` or `IdBlock::whole`, which refuse an empty block, a block over 2^16 ids, a block that runs past its block's last offset, and id 0.
  - `IdCursor` is the part of a group's fold that assigns ids. It has `peek`, `remaining`, `accepts`, `apply_block`, and `apply_assigned`.
  - The `Ids` group: `IdsFold { cell, next_block }`, `IdsEvent::Leased { block: IdBlock, to: ChainId }`, `IdsCmd { to }`, and `IdsErr::Exhausted`.
- **Names.**
  - `Login`, `LoginKey`, `RepoName`, and `RepoNameKey`, each with `parse`, `key`, and `as_str`, plus `RESERVED` and `NameError`.
  - `NameKey { Login, Repo }`, `name_shard`, `account_shard`, and `NAME_SHARDS = 1024`.
  - The fold types: `NamesFold`, `NamesScratch`, `NamesRead`, `LoginRow { Claimed, Former }`, and `AccountRow`.
  - `NamesEvent { LoginClaimed, Account, FormerName, Placement, RepoOwner, IdBlock }`.
  - `NamesCmd { cred, op: NamesOp }` with `NamesOp { ClaimLogin, ReleaseLogin, PutAccount, InitPlacement, MovePlacement, FinishMove, RepoOwner, IdBlock }`, plus `NamesOut` and `NamesErr`.
- **Authz.**
  - `AuthzFold`, `AuthzScratch`, and `AuthzRead`, including `is_admin` and `resolve`.
  - `AuthzEvent { Opened, Member, Grant, RepoName, IdBlock }`.
  - `AuthzCmd { cred, op: AuthzOp }` with `AuthzOp { Open, SetMember, SetGrant, CreateRepo, RenameRepo, DeleteRepo, IdBlock }`, plus `AuthzOut` and `AuthzErr`.
  - The value types `AccountKind`, `OrgRole`, `RepoPermission`, `NameEpoch`, and `RepoEntry { Reserved, Live, Alias, Deleted }`.
- **Tokens.**
  - `TokenKind` and `OpaqueToken`, with `parse`, `new`, `text`, `hash`, and `shard`. `TokenError`.
  - Sharding: `token_shard`, `shard_of_hash`, `Family`, `family_of(Region)`, `SHARDS_PER_FAMILY`, and `draw_secret(rand, family)`, which returns `RedrawExhausted` after 64 draws that all miss.
  - The group: `TokensFold`, `TokensRead`, `TokenRow`, `TokensEvent { Stored, Revoked }`, `TokensCmd { cred, op: TokensOp { Store, Revoke } }`, `TokensErr`, and `ScopeSet`.
- **Credentials.**
  - `Credential::Stored`, `Actor { User, Anonymous }`, `Principal`, and `Principal::credential_shard()`.
  - `UserId`, `OrgId`, `IdentityDep { Names, Tokens, Authz }`, `Own`, `credential_chains`, `check_credential`, and `CredentialRefusal`.
- **Cache.** `IdentityCache<K, V>` (`lookup(now, key, class)`, `insert(read_at, …)`, `forget`), `Lookup { Hit, Fetch, Stale, Overloaded }`, `RequestClass`, `READ_CACHE = 10 s`, and `IDENTITY_BUDGET_PER_ISOLATE = 50`.
- **Placement.** `validate_record(owner, record, evaluation, checks)` is what `graftyard placement set` calls. It runs `Placement::new` for multi-site records and `Placement::solo` for `Solo`.
- **Formats.** `NAMES_FORMAT`, `TOKENS_FORMAT`, `AUTHZ_FORMAT`, and `IDS_FORMAT` use families 0x10 to 0x13, version 1. The family numbers are this crate's choice, because no registry exists.

## How each command decides

Every command except `IdsCmd` carries a `CredentialShard` (`NONE` for `graft-ops`, the CLI, and finishers). `decide` first runs `check_credential`, so 401 comes before any other refusal. The check reads each row as follows:

- The account's `Account` row on `account_shard(account)`. A missing row refuses with `NoAccount` and a suspended row with `Suspended`.
- When the key is `Token { shard, hash }`, the token row:
  - A shard other than `shard_of_hash(hash)` refuses with `WrongShard`.
  - A missing row refuses with `NoToken`, a revoked row with `Revoked`, and a row for another account with `WrongAccount`.
- A row on the deciding group's own chain is read from the scratch view, so a revocation or suspension earlier in the same entry binds the commands after it.
- Other rows come from the `deps` folds, which are passed to `deps_read`. A chain the host did not pass refuses with `NotRead`, so a missing read fails closed.

`deps()` lists those credential chains once each, without the group's own chain. This is the per-entry read the design calls `credential()`, built from the trait's present shape.

**`NamesCmd`.** After the credential check, a key on another shard refuses with `WrongShard`. Then `permitted`: a user may claim a login for itself or sign up an org, release its own login, and change its own `sessions_not_before`. Everything else answers `Forbidden`. The ops:

- `ClaimLogin`:
  - A reserved key refuses with `Reserved`.
  - A key that is `Claimed` refuses with `Taken { by }`, also for the holder. A duplicate goes through the `request` row.
  - With `account: None`, the account takes `IdCursor::peek()`. With no free id, it refuses with `NeedIdBlock`.
  - A `Former` key may be claimed by anyone.
- `ReleaseLogin`: refused with `NotHolder` unless the account holds the key as `Claimed`. It writes `FormerName`.
- `PutAccount`: always writes the row.
- `InitPlacement`: refused with `RecordExists` if the owner has a record, and with `BadRecord` if `Placement::from_record` refuses. It stores the record with `moving_from: None`.
- `MovePlacement`:
  - It needs a record (`NoRecord`) and a change (`Unchanged`).
  - It writes `generation + 1` with `moving_from: Some(previous record)`. A move during a move nests, so `lookup_order` walks every record a group may still sit in.
  - It is refused with `BadRecord` when the shape is wrong.
- `FinishMove { generation }`: refused with `StaleGeneration` unless `generation` is the record's own and `moving_from` is set. It clears `moving_from` and nothing else, so a late Workflow cannot finish a newer move.
- `RepoOwner`: always writes the row.
- `IdBlock`: refused with `BlockRefused` unless the block starts at or after the end of the current block.

A follower's `check` refuses:

- a claim of a reserved key, or of a key another account holds;
- a block that does not follow the current one;
- a `Placement` that does not follow the stored record. "Follows" means one of: a first record with no `moving_from`; generation + 1 with `moving_from` equal to the previous record; or the same record with `moving_from` cleared.

**`AuthzCmd`.**

- `Open` is for operators only (`Forbidden` otherwise) and runs once (`AlreadyOpened`). Every other op needs the chain opened (`NotOpened`).
- A user must administer the owner. For a user account that means being the owner. For an org it means holding the `OrgRole::Admin` grant. Otherwise the command is refused with `Forbidden`. `IdBlock` is for operators only.
- `SetMember` needs an org (`NotAnOrg`). `SetGrant` always writes.
- `CreateRepo`:
  - It is refused with `Taken` if the key holds `Live`, `Reserved`, or `Deleted`. An `Alias` may be overwritten.
  - It takes the next id from the chain's `IdCursor` (`NeedIdBlock` when there is none) and writes `Live` at epoch + 1.
- `RenameRepo` needs a `Live` source (`NoRepo`) and a different key (`SameName`), and refuses a held target with `Taken`. It writes `Live` at the new key and `Alias` at the old, both at epoch + 1.
- `DeleteRepo` needs `Live` (`NoRepo`) and writes `Deleted { retain_until }`.

A follower refuses a second `Opened`, a name entry whose epoch goes backwards, and a `Live` or `Reserved` entry over a key held for another repository.

**`TokensCmd`.**

- After the credential check, a hash on another shard refuses with `WrongShard`.
- A user acts only for its own account (`Forbidden`). Operators act for any account.
- `Store` refuses `Installation` (`NotStored`), and refuses a hash that was ever stored, revoked included, with `Exists`.
- `Revoke` needs a row (`NoToken`) that is not revoked (`AlreadyRevoked`).
- A follower refuses a hash stored twice or stored on the wrong shard.

**`IdsCmd { to }`.** It leases the next whole block of the cell: `Leased { block, to }`, starting from block 1, so cell 0 never assigns id 0. It refuses with `Exhausted` past block 2^29 - 1. A follower refuses a lease that is not the next block, is not whole, or names another cell.

**Token parsing.** `OpaqueToken::parse` accepts `ghp_`, `github_pat_`, `gho_`, `ghu_`, and `ghr_` followed by exactly 36 base62 characters. The last 6 are the CRC32 of the 30 secret characters in base62 (`0-9A-Za-z`), zero padded. It refuses `ghs_` (`Stateless`), any other prefix, any other length or alphabet, and a checksum mismatch. It accepts GitHub's published example `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`. The stored hash is SHA-256 of the 30 secret characters with a domain tag.

## What `crates/raft` needs to change

`crates/raft`'s `EntryBody` has only `Events`, `Config`, and `Noop` (`crates/raft/src/types.rs`). So `IdBlock` is not there, and the crate does not edit it. Until it lands, each assigning fold records a block as an ordinary event: `NamesEvent::IdBlock` and `AuthzEvent::IdBlock`, applied by the same `IdCursor`. This keeps the design's property: the block commits before any assignment from it, and a new leader resumes from the committed cursor. The change needed in `crates/raft`:

1. `pub enum EntryBody<E> { …, IdBlock { base: DbId, len: u32 } }`. Better, carry `graftyard_identity::IdBlock`'s invariants by moving `IdBlock` into `crates/types` next to `DbId`.
2. In `encode_entry`, tag 3: `enc.u64(base.0 as u64); enc.u32(len)`, and use `RAFT_FORMAT`, which every release reads.
3. In `GroupFold::apply`, `EntryBody::IdBlock(b)` must reach the fold. `Fold` needs a hook such as `fn apply_id_block(&mut self, b: IdBlock) {}` with a no-op default, or `GroupFold` keeps an `IdCursor` itself and `Scratch` exposes it to `decide`.
4. In `verify`, refuse an `IdBlock` that does not start at or after the end of the last one, with `requests` empty.
5. A leader API to append one, such as `Member::id_block(now, IdBlock)`, so the host commits a block it leased from `Ids` before assigning. With the event stand-in, that is an ordinary `commit` of `NamesOp::IdBlock` or `AuthzOp::IdBlock`.

The `Command` trait also lacks `credential()`. This crate puts the credential chains in `deps`, which the leader already reads once per entry after the batch freezes. That meets the invariant without a trait change. If `credential()` is added, it should return `graftyard_types::CredentialShard`, which this package put in `crates/types` for that purpose. The leader then adds those chains to its `DepsWanted`.

The simulator harness in `crates/raft/tests/common` is fixed to `KvCmd` (`Disk`, `Slot`, `Sim`, and `Cluster` all name it). It could not be reused without editing `crates/raft`. `crates/identity/tests/common/mod.rs` is a generic copy of `Cluster` (`Group<C: Command>`) driven by hand. To run the identity commands under the seeded fault simulator, make `Sim` and `Cluster` generic over `C: Command` with a client-op generator per command type.

## Tests

`cargo test -p graftyard-identity`: 11 unit tests, 23 in `tests/decide.rs`, and 5 in `tests/raft.rs`, all passing. `tests/decide.rs` decides batches as a leader does (one scratch, each command seeing the earlier commands' events). It then checks every produced event with the follower `check` before applying, and fails if a follower would refuse.

| Requested case | Test | Result |
| --- | --- | --- |
| Login claims racing in one entry, the second refused by the first (BF-13 style) | `decide::two_claims_of_one_login_in_one_entry_the_second_refused_by_the_first`; `raft::a_refusal_resting_on_an_uncommitted_claim_is_never_returned` | Pass. On real members, three claims of one login in different cases batch into one entry. Only the first produces events. The leader is lost before commit, and no ticket resolves as an ack or a refusal, only `NotLeader`. The re-sent claims decide again on the new leader: one claim, one `Taken` |
| A name released and reclaimed | `a_released_login_is_a_former_name_until_someone_claims_it`, `the_original_owner_can_take_its_former_name_back_by_account` | Pass. A reclaim by a new account gets a new id. A release and a claim in one entry succeed |
| Case-folding collisions | `case_folding_collides_…`, `repo_names_are_unique_case_folded_…` | Pass. Non-ASCII never parses |
| Reserved names | `reserved_names_are_refused_in_any_case_and_followers_refuse_them_too` | Pass. `_`-prefixed entries cannot parse as logins |
| `DbId` never reused across a leader change mid-block | `raft::db_ids_are_never_reused_across_a_leader_change_mid_block` | Pass. A 12-id block ends at offset 65,535, and a leader appends 5 claims and dies. The new leader reassigns those never-acknowledged ids, crosses into block 22 after exactly one `NeedIdBlock`, and commits 35 claims with 35 distinct ids. Every member's cursor agrees |
| `IdBlock` boundaries (offset 2^16 - 1) | `ids::tests::cursor_runs_to_offset_2_pow_16_minus_1_then_stops`, `id_block_refuses_…`, `pack_round_trips_and_stays_below_2_pow_53`, `decide::a_repository_id_is_never_reused_across_blocks` | Pass. The top id is 2^53 - 1 |
| Token shard redraw distribution | `tokens::tests::redraw_lands_in_the_family_in_about_three_draws`, `a_source_that_never_lands_is_reported` | Pass. Over 6,000 tokens per family, the mean draw count is 2.8 to 3.2. Chi-square over the family's 1,024 shards is under 1,250. Base62 characters are even |
| A revoked token's commands refused (BF-15) | `raft::bf15_revoked_token_with_1000_commands_queued`, `raft::bf15_suspended_account_with_1000_commands_queued`, plus the `decide` versions | Pass. On real members, 1,000 `CreateRepo` commands queue on an owner's `Authz` leader. The token is revoked, or the account suspended, while the first waits on its dependency read. 1,000 of 1,000 decided afterwards get `Unauthorized`. The host read the credential chains 4 times for the 1,000 commands, once per entry |
| An owner placement change keeping `moving_from` | `an_owner_move_bumps_generation_and_keeps_moving_from_until_finished`, `followers_refuse_a_placement_that_drops_moving_from_or_skips_a_generation` | Pass, including the nested move and the stale `FinishMove` |
| Placement validation, Solo flag | `placement::tests::placement_set_reuses_placement_new_and_solo_needs_the_flag` | Pass |

### Planted bugs

Each bug was applied alone and the suite rerun, then reverted. All 18 were caught:

| Bug | Caught by |
| --- | --- |
| `ClaimLogin` reads the fold, not the scratch (ignores the earlier command) | `two_claims_…`, `a_released_login_…` |
| Reserved check removed | `reserved_names_…` |
| Login key not case folded | 4 tests |
| `apply` does not advance the id cursor (reuse) | 3 tests |
| `IdCursor` accepts a rewinding block | `cursor_runs_to_offset_…` |
| `IdBlock` may run past offset 2^16 - 1 | `id_block_refuses_…` |
| Revoked token not checked | `a_revoked_token_…`, `a_revocation_earlier_…` |
| Own-chain credential not read from scratch | `a_revocation_earlier_in_the_same_entry_…` |
| Suspension not checked | `a_suspended_account_…`, `a_revocation_earlier_…` |
| Move drops `moving_from` | `an_owner_move_…` |
| Move does not bump `generation` | `an_owner_move_…` |
| `FinishMove` ignores `generation` | `an_owner_move_…` |
| Redraw stops after one draw | both redraw tests |
| Checksum comparison inverted | `parses_githubs_published_example_…` |
| `Placement::solo` without the evaluation flag | `placement_set_…` |
| Repository name not case folded | `repo_names_are_unique_…` |
| `Deleted` name not held | `repo_names_are_unique_…` |
| A non-admin may create repositories | `only_admins_create_…` |

Testing also found three bugs in my own first drafts, all now fixed. A shard-count bound in the redraw test was wrong for a Poisson spread. The test hosts sent commands to a deposed leader. And committed events were read from a follower, which learns the last commit index only from the next append.

## Checks

```
cargo test -p graftyard-identity -p graftyard-types          ok: 11 + 23 + 5, and types 11
cargo clippy -p graftyard-identity -p graftyard-types --all-targets -- -D warnings   clean
cargo fmt --check -p graftyard-identity -p graftyard-types   clean
cargo build -p graftyard-identity --target wasm32-unknown-unknown   ok
cargo check --workspace --all-targets                        ok
```

## Design ambiguities found

1. **Families.** The design says "1,024 shards per home family (na, eu, apac)" (DESIGN, Shape, Identity, `TokenShard`). It does not say how regions map to families or how a family's shards are numbered. This crate uses shards f × 1024 to (f + 1) × 1024 - 1, with enam and wnam as na, weur and eeur as eu, and apac and oc as apac [inference].
2. **Redraw.** The redraw rule says "A new token's secret is redrawn until its hash lands in the owner's family, about 3 draws on average". That needs a bound, because an unlucky or broken source never lands. This crate gives up after 64 draws, a (2/3)^64 chance.
3. **How `RepoNameKey` is formed.** SHARED defines `RepoName` and `RepoNameKey` but states the folding only for logins ("LoginKey: ASCII lowercase, the only form compared"). This crate lowercases repository names too [inference].
4. **`RESERVED` is incomplete.** It ends in `/* .. */` and must be "Generated from the router; CI fails on drift". The router does not exist, so the crate holds SHARED's 21 entries. Four of them (`_services`, `_t`, `_codeload`, `_registry`) cannot be logins anyway, because `_` is not a login character.
5. **The trait shape.** "`fn credential(&self) -> CredentialShard;`" is in the design's `Command` trait but not in `crates/raft`'s. See "What `crates/raft` needs to change".
6. **`decide` has no clock.** "A missing, revoked, or suspended row refuses the command with 401" does not mention an expired token. `decide` has no clock, though it could use the entry's `at`, which the trait does not pass. So this crate leaves `expires` to the edge's authentication [inference]. A token that expires while its command is queued is therefore not refused by the leader.
7. **`moving_from` during a move.** "Changing an owner's record bumps `generation` and keeps the old record in `moving_from`" does not say what happens when the record changes again before the Workflow "clears `moving_from` last". This crate nests (the new record's `moving_from` is the whole old record, `moving_from` included) and makes `FinishMove` name a generation.
8. **A user's own placement.** A new owner "copies the install account's record". The design does not say who writes it or when relative to the login claim, which is the signup commit point. This crate leaves it to the signup finisher (`InitPlacement`, operator only). Until then, the owner's chains have no record to find.
9. **`Solo` without the flag.** R15 is still open: "may an owner be placed `Solo` on an install without the `evaluation` flag". `validate_record` follows `Placement::solo` and refuses it.
10. **Format families.** "`LogEntry.format` names" a record family and version, but no table of family numbers exists. This crate picked 0x10 to 0x13.

## What remains for hosting

- **Groups.** These need hosting as member classes in `workers/site`, with the fold stored in member SQLite:
  - one `Ids` group per cell, on `Tri`, from `Genesis.placements`;
  - 1,024 `Names` shards and 3 × 1,024 `Tokens` shards, all from genesis records;
  - one `Authz(owner)` group per owner, from the owner's `Names` `Placement` record.
- **Genesis.** M1-WP7's genesis acceptance (R13) is not built: `graftyard genesis`, `graftyard sign`, `graftyard install --genesis`, and a `Release` group that takes only a `Genesis` matching the trust anchor. No system chain can bootstrap without it.
- **DO classes.** These run on the existing per-site member class, keyed by `ChainId` text:
  - the `MemberRegistry` rows for each new chain;
  - an `IdentityCache` per edge isolate, not a DO.
- **Edge calls.**
  - Parse the token with `OpaqueToken::parse` and check `expires`.
  - Read the token row and the `Account` row: through `IdentityCache` for reads, and on the leader through `fresh` for writes.
  - Build `Principal::credential_shard()` and attach it to every command.
  - Answer `DepsWanted` for `Names` and `Tokens` chains with `IdentityDep` from the leader's `fresh`.
  - Resolve `o/r` through `Names` (`LoginRow`), then `Authz` (`resolve`), and `/repositories/{id}` through `RepoOwner`.
  - Find a chain's group through the owner's `Placement` record and its `lookup_order`.
- **Lease loop.** On `NeedIdBlock`, call `IdsCmd { to }`, commit `IdBlock` on the asking group, and re-send. Better, lease ahead of exhaustion so the refusal is rare.
- **Signup and repository-creation finishers.**
  - Signup: `ClaimLogin`, then `PutAccount`, `InitPlacement`, and `AuthzOp::Open`.
  - Repository creation: `CreateRepo`, then `RepoOwner` and the repository chain's `Genesis`.
- **Owner move Workflow.** `MovePlacement`, `Joint` for each of the owner's groups from the old sites' `MemberRegistry` rows, then `FinishMove`.
- **R14.** How the gate's accounts, repositories, and tokens are created before REST and sign-in exist is still open. The operator-only ops (`CredentialShard::NONE`) are the hook a CLI or bootstrap path would use.
- **Gate rows.** BF-10, BF-11, and BF-15 run on deployed code.

## On Durable Objects (2026-11-01)

Run 2026-11-01 on the deployed test install: graft-m1-site-a/b/c (enam, weur, apac) and two test edges, graft-m1-wp7-edge and graft-m1-wp7-edge-b, under `tests/m1/wp7/`. The raw rows are in `tests/m1/wp7/raw/`: `p1.setup`, `g1.bf10`, `g1.bf10mv`, `g1.bf15`, and `g2.bf11`. `g1.bf11` is the BF-11 run on the code before the `home` fix and is not counted. Short trial runs during development used owners outside these pools, and their rows were deleted.

**The system chains' records come from a test-only config.** Genesis acceptance (R13) is being built separately. Until it lands, `tests/m1/wp7/genesis.json` holds Tri records at generation 1 for `names`, `tokens`, and `ids`. `workers/site/deploy.mjs --genesis` copies them into each site's `GRAFTYARD_GENESIS` variable, and `tests/m1/wp7/deploy-edge.mjs` copies them into each test edge's `GENESIS` variable. The product code reads only the variable, which stands where the accepted `Genesis.placements` will. It has no test-only resolver (`decisions/DECISIONS.md`, "Milestone 1 builds minimal identity").

### `crates/raft` changes for identity

- **`EntryBody::IdBlock(IdBlock)`** has encode tag 3: `u64` base, then `u32` len. `IdBlock` moved to `crates/types` (`ids.rs`, additive). Serde decoding re-checks its invariants (`try_from = IdBlockRaw`). The digest's canonical encoding of the other entry kinds is unchanged, and the pinned digests in `tests/codec.rs` still match.
- **`Fold::apply_id_block` and `Scratch::apply_id_block`** both have no-op defaults. The `Command` trait is unchanged.
- **`GroupFold`** carries the latest committed block, available through `id_block()`. `GroupFold::from_parts` takes it as a new fourth argument (an API change).
- **`verify`** refuses a block that does not follow the latest one, counting unapplied stored entries. It also refuses an `IdBlock` entry that carries requests.
- **`Member::id_block(now, block) -> Ready<Result<(), IdBlockError>>`.** The block takes the in-flight slot ahead of queued commands and resolves as `Resolution::IdBlock { block, result }`. `IdBlockError` is `NotLeader` or `Behind { current }`. Taking a block again while it is queued or in flight is a no-op.
- **`Member::reconfigure(now, GroupConfig) -> Ready<Result<Index, ReconfigError>>`** and **`Member::latest_config()`**, for BF-11.
  - A change may add or remove one voter site and must keep the member on every other site.
  - So a member retried or replaced on its own site goes out through `Degraded { lost }` and comes back as a new member in a second change. Votes and acks are counted by site, so one config cannot name two members on one site.
  - The previous config must have committed, the leader must be ready with nothing in flight, and it must stay a voter.
  - A site that comes back has its match state reset, so nothing the old member acknowledged counts for the new one.
  - `ReconfigError` is `NotLeader`, `Busy`, `NotOneChange`, or `RemovesLeader`.
- **`crates/identity`** uses `EntryBody::IdBlock`, and its event stand-ins are gone: `NamesEvent::IdBlock`, `NamesOp::IdBlock`, `NamesErr::BlockRefused`, and the three `Authz` equivalents. The `Names` and `Authz` folds apply blocks through `IdCursor::apply_block`. Their events, folds, commands, outputs, errors, and rows gained serde derives. `Login`, `LoginKey`, `RepoName`, and `RepoNameKey` serialize as text and are validated on decode.

The simulator's host records an id block on 2‰ of steps, 20% of them stale re-records. `check` asserts that committed blocks never rewind and that the fold and `GroupFold::id_block` both match the latest committed block. Two scenario tests were added. `id_blocks_commit_as_their_own_entry_and_never_rewind` covers the leader and `verify` refusals. `a_member_is_replaced_on_its_own_site_in_two_config_changes` covers `NotOneChange`, `Busy`, `RemovesLeader`, and a new empty member catching up to identical digests.

### The hosting design

**One member class, one family per object.** The `Member` Durable Object reads the chain kind from its stored `/join`, or from the first `/join`, and builds one `Host<F>` for the family that kind runs (`workers/site/src/family.rs`, `Kind::of`):

| Chain | Family | Fold storage | Record source checked at bootstrap |
| --- | --- | --- | --- |
| `names:<shard>` | `NamesCmd` | one codec blob in `fold` | genesis |
| `tokens:<shard>` | `TokensCmd` | one codec blob in `fold` | genesis |
| `ids` | `IdsCmd` (cell 0) | one codec blob in `fold` | genesis |
| `authz:<owner>` | `AuthzCmd` | one codec blob in `fold` | the owner's `Placement` row on `Names` shard `account_shard(owner)`, read through `fresh` |
| `repo:<id>` | `RepoSlot`, the test register for now | `reg` rows | none (the register trusts the record it was sent) |

The other chain kinds (`network`, `identity`, `actions`, `release`, `repo-meta`) have no family yet, and their `/join` answers 400. The `Family` trait covers the per-family parts:

- the formats a family reads;
- how its fold is stored, inside `Write::Apply`'s transaction, and loaded on restore;
- the JSON its outputs and refusals take on the member's routes: 401 for `Unauthorized`, 403 for `Forbidden`, 422 for the rest;
- what a `fresh` read answers;
- how another group's fold, read as a dependency, decodes;
- which refusal asks for an id block;
- where the record is checked at bootstrap.

The storage, persist, alarm, and transport design from M1-WP2 is unchanged (`tests/m1/results/m1-wp2.md`, "On Durable Objects"). The changes to it:

- **Tables.** `fold` holds the identity folds. `id_block` holds the latest committed block. `deps_log` holds every dependency read: the entry it decided, when, and each chain's read index. `group_config` already existed. Restore builds the `GroupFold` with `from_parts` from the fold, `id_block`, `group_config`, and `request` tables, then compares it with the log replayed through `applied`.
- **Dependency reads.** When the leader's core asks for `DepsWanted` after a batch freezes, the host reads each chain once through that chain's leader. It calls `/read` with `{q: {dep: true}}`, which runs `fresh` and answers the whole fold in the crate's codec. The host then hands the folds to `deps_read`. So `decide` sees one read per entry, taken after the batch froze. The calls find the dependency's group by the lookup rule below (`workers/site/src/lookup.rs`).
- **The id-block lease loop.** A `NeedIdBlock` refusal is not answered to the caller.
  1. The host commits `IdsCmd { to: chain }` on `ids` through the lookup rule, under one request id.
  2. It records the leased block with `Member::id_block` and waits for it to commit.
  3. It re-sends every command refused for want of a block, under the command's own request id. The old ticket maps to the new one, so a waiting `/outcome` follows it.
  On DO, the first `CreateRepo` on a new `Authz` chain leased block 2 of cell 0 and got repository id 131,072 in 4.9 s.
- **Member names.** Messages go to the names in the latest config in the log, or to the bootstrap names until the group has a committed config.

**Bootstrap check.** This is `check_source` in `member.rs`. A bootstrap member is one whose generation equals its record's. Before it stores anything, it reads its chain's record from the source: the genesis variable, or the owner's row through `fresh` on `Names`. It joins only if the record it was sent is in the source's `lookup_order`. It then claims the record generation that bootstraps the chain, on the registry of the first site of the oldest record in that order (`POST /claim`, first claim wins), and joins only if its own generation won. Two more join rules:

- A member already joined refuses a `/join` with another identity or record.
- A member whose site's registry row names a later generation of its chain refuses to join, so a retired name cannot bootstrap a second group.

**The lookup rule.** It is in `workers/site/src/lookup.rs` for member-to-group calls and in `tests/m1/wp7/edge.ts` for the edges.

1. The config cached from the group's last `NotLeader`, leader first.
2. The bootstrap names from the chain's record, in record order.
3. The chain's row in the `MemberRegistry` of each record site.
4. Steps 2 and 3 for each `moving_from` record.

A `NotLeader` that names a config teaches the caller the voters, and the caller follows the named leader. A `NotLeader` that names no leader is reported to every voter as `suspect`, once per lookup. The report carries no command, and the edge finds the leader with `/outcome_by_request`, which only a leader answers. A member that is not joined (409), or that holds no config, is skipped. Placement failures retry with a new member generation: `/replace` retires a member whose latency check failed and tries `next_generation()` again, up to three times.

### The `Repo` slot

`workers/site/src/family.rs` defines `pub type RepoSlot = Register;`, and `Hosted::Repo(Rc<Host<family::RepoSlot>>)` hosts it. To hand it over, implement `Family` for `crates/git`'s repository fold and `Push` command, then change the alias. Nothing else in the site script names the register for `repo:` chains. The implementation needs:

```rust
impl Family for RepoFamily {
    type Ev = /* the repository's event type: raft::Record + Clone + Debug + PartialEq + Serialize + DeserializeOwned */;
    type Fold = /* the repository fold: raft::Fold<Event = Self::Ev> + PartialEq */;
    type C = /* Push (or an enum of the repository's commands): Command<Fold = Self::Fold> + DeserializeOwned */;
    fn reads() -> Vec<Format>;                        // the repository record formats this release decodes
    fn new_fold(chain: ChainId) -> Self::Fold;        // the empty fold for repo:<id>
    fn store_fold(fold: &Self::Fold, entries: &[&LogEntry<Self::Ev>], stmts: &mut Vec<Stmt>);
                                                      // SQL statements that store the fold after `entries` applied;
                                                      // they run in Write::Apply's transaction, in the output gate
    fn load_fold(sql: &SqlStorage, chain: ChainId) -> worker::Result<Self::Fold>;   // the inverse, on restore
    fn out_json(o: &Out<Self>) -> Value;              // `done.ok.out` on /commit
    fn err_json(e: &Err<Self>) -> Value;              // {"err": .., "status": 401 | 403 | 422}
    fn read_json(fold: &Self::Fold, q: &Value) -> Value;   // the answer to /read {"q": ..} through `fresh`
    fn fold_bytes(fold: &Self::Fold) -> Vec<u8>;      // only if another group reads this one as a dependency
    fn dep(chain: ChainId, bytes: &[u8]) -> Result<Dep<Self>, String>;
                                                      // decodes a dependency fold (IdentityDep for credentials)
    fn needs_block(e: &Err<Self>) -> bool { false }   // true for a refusal the host answers by leasing an id block
    fn source(chain: ChainId) -> Source;              // Source::Owner(owner) once the edge knows the repository's
                                                      // owner (RepoOwner); Source::Given trusts the sent record
    fn audit(..) -> worker::Result<Value> { Ok(Value::Null) }   // optional extra state for test audits
}
```

The edge sends commands as `/commit {"rid": "<26-character ULID>", "cmd": <serde JSON of C>}` to the leader it found by the lookup rule. A command whose `Command::deps()` names the credential chains gets them read once per entry. Commands that store packs out of band should keep the pack write before the commit, as `crates/git` already plans; the host adds no pack I/O. A repository chain created by the git edge should join its members with `Join { chain, me, record }`, where `record` is the owner's record, and set `source` to `Source::Owner` so the bootstrap check reads it.

### Setup

`node tests/m1/wp7/run.ts setup --run p1` created the `ids` group and eight `Names` shard groups (`names:453, 644, 1003, 414, 792, 10, 761, 304`). It then committed 1,150 owner records, one operator `InitPlacement` each, holding a copy of the Tri record at generation 1 (home enam). The owners are test accounts with ids from 5,000,000 up, chosen so that `account_shard(owner)` lands on those eight shards. They are not assigned by `ClaimLogin`. A by-hand signup on DO also passed: `ClaimLogin` with `account: None` on `names:911` got id 65,536 from the first leased block; then came `PutAccount`, `InitPlacement`, `Open`, a `Store` of a PAT on `tokens:2279`, and a `CreateRepo` under that token, which got repository 131,072 from the `Authz` chain's own lease.

### BF-10: one group per chain, and no read or vote without a config

**Pass.**

**Part 1, 1,000 chains, two edges at once** (`bf10 --run g1 --chains 1000 --conc 20`, 13.9 min). For each owner chain `authz:<owner>`:

1. Both edges ran `/create` at once with cold caches. Each edge read the owner's record from `Names` and joined the three bootstrap members. Each then probed every member that held no committed config with a vote request and a read in one turn (`/probe_vote`). Edge A campaigned on the record's home (enam), and edge B on apac.
2. Both edges then committed `AuthzOp::Open` at once, each under its own request id.
3. `/census` read every member of the chain on all three sites and the chain's registry rows.

| Count | Result |
| --- | --- |
| Chains with one group (every configured member holds the same bootstrap digest) | 1,000 of 1,000 |
| Chains with two groups / no group | 0 / 0 |
| Chains where exactly one `Open` was accepted and the other got `AlreadyOpened` | 1,000 of 1,000 |
| Members per chain holding a committed config | 3 for all 1,000 |
| Probes of unconfigured members: votes granted / reads served | 0 / 0 of 5,021 (every deny `NoConfig`, every read refused with no config) |
| Probes skipped because the member already held a config when the probe came | 979 |
| Creates that answered `ok: false` | 3 of 2,000. All three were edge B's final lookup, run while both edges were campaigning; each chain still had one group and one accepted `Open`. A later `/create` on one of them found it at once (`bootstrap0`, leader apac) |

Each chain took a median of 15.8 s to create from both edges, p99 21.3 s. Most of that is the 20 sequential RTT pings each new member makes to each peer.

**Part 2, 100 chains during an owner record change** (`bf10mv --run g1`). Each round moved the owner's record from generation 1 (home enam) to generation 2 (home weur) with an operator `MovePlacement` while two edges created the chain. Edge A held the generation 1 record from its cache. Edge B read the record from `Names` with `fresh`. There were three orderings, in rotation:

- straddle: A's members joined before the move committed and campaigned after it, and B created after it;
- move-first: the move committed, then A and B created at once;
- together: A created while the move committed, and B created after.

| | Rounds | One group | Two groups | One `Open` accepted | Joins refused |
| --- | --- | --- | --- | --- | --- |
| With the generation claim | 100 (34 straddle, 33 move-first, 33 together) | 100 | 0 | 100 | 3 per round. Generation 2's members were refused (`claimedBy: 1`), so every chain booted from generation 1 and edge B found that group through the registry rows (`registry0`) |
| Control: the design's sentence alone, no claim (`no_claim`: a member joins if its record is the source's current generation) | 20 (straddle) | 0 | 20 | 20 | 0 |

The control is the design problem described in "Design problems found" below. Each control chain has six configured members, holding two different bootstrap digests: three for generation 1 and three for generation 2. The `Open` race still accepted one per chain only because edge A's `Open` and edge B's `Open` both found the same one of the two groups first. A later edge that finds the other group sees a second, empty `Authz` chain.

### BF-15: commands queued behind a revocation or suspension are refused with 401

**Pass.** `bf15 --run g1 --queued 1000`. For each case:

1. An owner chain was created. Its account was marked unsuspended (`PutAccount`) and the chain opened (`Open`).
2. A PAT was stored on `tokens:2279` (`Store`).
3. One `SetGrant` under the token committed, to show the credential worked.
4. One `/commit_many` sent 1,000 `SetGrant` commands under the token to the `Authz` leader, each with its own request id. The call told the leader to hold its first dependency read for 8 s, so the first entry waited on it and the other 999 queued behind it.
5. 2 s into the hold, edge B committed the change: `TokensOp::Revoke` on `tokens:2279`, or `PutAccount { suspended: true }` on `account_shard(owner)`.

| Case | Change committed at | Decided after it | 401 | Accepted | Entries | Dependency reads (incl. step 3's) |
| --- | --- | --- | --- | --- | --- | --- |
| Revoke the stored token | `tokens:2279` index 16, 2.46 s in | 1,000 of 1,000 | 1,000 (`Unauthorized(Revoked)`) | 0 | 2 | 3 |
| Suspend the account | `names:453` index 142, 2.49 s in | 1,000 of 1,000 | 1,000 (`Unauthorized(Suspended)`) | 0 | 2 | 3 |

The member's `deps_log` shows the reads, one row per entry:

- Index 4 is step 3's `SetGrant`. It read `tokens:2279` at index 15 and the account's `Names` shard before the change, and was accepted.
- Index 5 is the batch's first entry, holding the first command alone because one entry is in flight. Its read was held 8 s and taken after the change: `tokens:2279` index 16 in the revoke run, `names:453` index 142 in the suspend run.
- Index 6 holds the other 999 commands. It read both chains once more.

So the 1,000 queued commands were decided by 2 entries, each with one read of each credential chain, not one read per command. The table's "Dependency reads" column counts all three rows, step 3's included. The 401 is the edge's status, from `Family::err_json`. The commands were `SetGrant`, which needs no id, so no lease ran.

### BF-11: edges with cold caches find the group after a member retry, `ReplaceMember`, and an owner move

**Pass, 1,500 of 1,500 requests.** `bf11 --run g2 --from g1 --skip 30 --groups 30 --per 10` ran on 30 chains that BF-10 created (owners 31 to 60 of its list). For each chain, each phase sent 10 requests that alternated between the two edges: two fresh reads, then two operator `SetGrant` commits, and so on. Every request started with `cold: true`, which drops the isolate's cached configs, leaders, and owner records. Every third request also skipped the first record's bootstrap names. Those requests had to find the group through the next step of the rule: the registry, or `moving_from`. The phases:

1. **before**: no change.
2. **retry**, the member retry: site apac's member is replaced. The first replacement, `#2#f2`, has its latency check fail (injected), so `/replace` retires it and joins `#3#f2`, the next generation. This took two config changes, `Degraded { lost: apac }` and then `Normal` naming `#3#f2`. If apac held the leadership, the driver first transferred it away: 9 of 30 chains.
3. **replace**, the `ReplaceMember` stub: weur's member `#1#f1` is lost (`/retire` deleted its storage) and replaced by `#4#f1` on weur, through the same two changes.
4. **moving**, the owner move: an operator `MovePlacement` commits generation 2 (home weur) on `Names`, with generation 1 in `moving_from`. The group's config is re-homed on weur (`/reconfigure`) and leadership is transferred to weur. In this phase every request also skipped the first record's registry rows, so it had to find the group through `moving_from`.
5. **moved**: `FinishMove { generation: 2 }` clears `moving_from`.

| Phase | Found | Step that found the group |
| --- | --- | --- |
| before | 300 / 300 | bootstrap names 208, registry 90 (every bootstrap-skipping request), cached 2 |
| retry | 300 / 300 | bootstrap 210, registry 90 |
| replace | 300 / 300 | bootstrap 210, registry 90 |
| moving | 300 / 300 | `moving_from`'s bootstrap names 298, `moving_from`'s registry rows 2 |
| moved | 300 / 300 | registry 300: generation 2's bootstrap names are never joined (claimed by generation 1), so after the move every lookup goes to the record's registry rows |

Request latency, cold cache included: p50 404 ms, p99 1,683 ms, max 6,348 ms. All 30 chains ended with one group (`/census`: one bootstrap digest among the configured members). Each had four joined members: the three voters, plus the removed `#1#f2`, which still answers with its last committed config, as the design says a removed member does. All 30 retries took two tries (`#2#f2` failed by injection, `#3#f2` passed). All 30 replacements took one (`#4#f1`). All 30 moves, re-homes, and `FinishMove`s committed.

The 13 lines "`/leader` failed ... no group" in `/tmp/wp7-bf11.log` are the driver's own leader lookups during its transfers, not counted requests. A `/leader` call needs a member to answer a `fresh` read, and the driver retried each one within 4 s.

**A bug the run found, now fixed.** In the first BF-11 run (`g1`, 30 chains), leadership went back to enam within seconds of each move's transfer to weur. The leader's "transfer back to `home`" rule (M1-WP3) read `home` from the member's bootstrap record (`MemberConfig.record.home`), not from the group's config, so a re-homed group was pulled back to its old home. `crates/raft` now reads `home` from the latest config's placement (`health.rs`, `Member::home`). The new test `leadership::home_follows_the_config_not_the_bootstrap_record` fails with the old line and passes with the fix. The second run (`g2`) is on the fixed code.

After the re-home, 28 of 30 groups still had their leader on enam 20 s after the transfer. The transfer to weur answered `Target(SiteId(1))` in all 30. That refusal comes from `may_lead_target`: `/replace` had made weur's member a new one, `#4#f1`, a few seconds earlier, and weur was not yet a target the leader would hand to. The leader's own return-home rule moved it later. On `authz:5001947`, 25 commits a second apart moved the leadership to weur. The g1 run's requests found the group in every phase as well, 1,500 of 1,500, but on the unfixed code. They are kept in `raw/g1.bf11*.jsonl` and not counted.

### What is stubbed

- **Genesis.** The system chains' records come from `tests/m1/wp7/genesis.json` through the `GRAFTYARD_GENESIS` variable, not from an accepted signed `Genesis` (R13).
- **Owner accounts.** BF-10, BF-11, and BF-15 use test account ids with operator `InitPlacement` records. Only the by-hand signup used `ClaimLogin` and the id-block lease. How the gate's accounts get created before REST and sign-in exist is still R14.
- **`ReplaceMember`.** There is no Workflow, no spare site, no `InstallSnapshot`, and no `Joint`. The lost member is deleted (`/retire`), and the leader's `/replace` route adds a new-generation member on the same site with `Degraded` and then `Normal`. The new member catches up from the leader's log, which on these short logs needs no snapshot. The design's `ReplaceMember` enumerates groups from the survivors' `MemberRegistry` rows, adds a spare on the lost site's continent, and uses `Joint`. None of that is built.
- **Owner move.** There is no Workflow. The driver commits `MovePlacement`, re-homes the group's config with one `Normal` change (the move keeps the same three sites, so no member moves), transfers leadership, then commits `FinishMove`. A move to other sites, through `Joint`, is not built. `Member::reconfigure` refuses a change of more than one voter site.
- **The latency check that fails on retry** is injected (`fail_first`). The check itself is `Placement::new` on the members' measured `Rtt` rows, and it ran for every new member.
- **Kills and holds.** `/retire`, `hold_deps_ms`, and `no_claim` are test routes and fields in the product site script, like M1-WP2's kill faults. They must be removed, or gated off, before a real install.

### Design problems found

1. **Two edges holding two generations of an owner's record build two groups.** The design says: "A member with no committed config serves no read, grants no vote, and stores only that entry, after it reads the same record generation from the record's source itself (the `Names` leader, or genesis)" (`design/DESIGN.md`, Shape, `ReplyMeta` and Bootstrap). During a move this does not hold. Edge A, with a cached generation 1 record, creates the chain's generation 1 members, which read generation 1 from `Names` and store the bootstrap entry. The move then commits. Edge B, reading generation 2, creates generation 2's members, which read generation 2 from `Names`. Each set "reads the same record generation from the record's source", and the chain gets two groups. The control showed this in 20 of 20 rounds: six configured members, two bootstrap digests [measured, `tests/m1/wp7/raw/g1.bf10mv.jsonl`, `noClaim: true`].
   - What the site script does instead, which is not in the design: a bootstrap member also claims its generation on the registry of the first site of the oldest record in the source's `lookup_order`, and only the first claim may bootstrap (`POST /claim`). With the claim, 100 of 100 rounds built one group.
   - A design fix could make the bootstrap read and the claim one step on `Names`: for example, a `Bootstrapped { chain, generation }` row the `Names` leader commits once. Or it could say that a chain is created only from its owner's oldest record still in `moving_from`.
2. **A replacement on the same site needs two config changes.** `GroupConfig` counts votes and acks by site (`quorum::voters`), so one config cannot name two members on one site. A member retry ("A failed latency check is retried with a new member generation", `design/DESIGN.md`, "Install operations", Placement failures) on the same site therefore goes through `Degraded { lost }`, then `Normal` with the new member. That means one window running on two voters, and two config entries per retry. The design does not say which change sequence a retry uses.
3. **`home` lives in two places.** `PlacementRecord.home` and `Placement.home` can differ after a move. The design says leadership returns to "the owner's home", and `Placement` holds "Preferred leader site, the owner's home; transfer restores it". The leader followed the record's until this pass (see BF-11). The design should say that the config's `home` wins once the group has one.
4. **`MovePlacement` with the same sites.** The re-home in BF-11 changed only `home`, and `crates/identity` accepts that as a move (generation + 1, `moving_from` kept). The design's move Workflow moves groups "through `Joint`". A change of home alone needs no `Joint`, and the design does not say so.

### Checks

Run after the last code change:

```
cargo test -p graftyard-raft -p graftyard-identity -p graftyard-types --release
    raft: codec 5, leadership 33, liveness 4 (+1 ignored), scenarios 27, sim 5 (+1 ignored); identity 11 + 23 + 5; types 11; all pass
RAFT_SEEDS=2000 RAFT_STEPS=10000 long sweep (after reconfigure and the home fix): pass, 279.3 s
    HARSH: ops 175,878, crashes 225,797, id blocks recorded 24,263, committed 8,198
    GRAY:  ops 2,252,190, gray faults 17,291, id blocks recorded 37,583, committed 29,109
Porcupine (tests/m0/porcupine): /tmp/raft-hist.jsonl Ok, 172,826 operations; .gray Ok, 2,252,169 operations
cargo test --workspace                                               pass
cargo clippy --workspace --all-targets -- -D warnings                clean
cargo clippy -p graftyard-site --target wasm32-unknown-unknown -- -D warnings   clean
cargo build -p graftyard-raft -p graftyard-identity --target wasm32-unknown-unknown   ok; worker-build --release ok
cargo fmt --check -p graftyard-raft -p graftyard-identity -p graftyard-site -p graftyard-types   clean
cargo fmt --check (workspace)    one diff, in crates/git/src/smart.rs (not this package's)
tsc -p tests/m1/wp7/tsconfig.json   clean (edge.ts); run.ts runs under Node 26 type stripping, no @types/node to check it with
python3 scripts/check_names.py      OK
```

### Deployed

- graft-m1-site-a/b/c: the site script with this pass, deployed with `--genesis tests/m1/wp7/genesis.json`.
- graft-m1-wp7-edge and graft-m1-wp7-edge-b, from `tests/m1/wp7/deploy-edge.mjs`.

The M1-WP2 edge (graft-m1-wp2-edge) and graft-m1-driver were left in place and not redeployed. The site script they bind is this pass's, and it still hosts the register on `repo:` chains.

## Genesis and the `Release` group on Durable Objects (2026-11-01, integration)

This pass hosts the `Release` group with `crates/release` and serves the release endpoint on the git edge. It replaces the test-only `GRAFTYARD_GENESIS` path in product code, and proves the CLI's path end to end on a throwaway install.

### The `Release` group

- **The member** (`workers/site/src/release.rs`).
  - The family is `ReleaseState { fold: ReleaseFold, anchor, entries }`. The anchor is this script's own `GRAFTYARD_INSTALL` and `GRAFTYARD_CUSTOMER_KEY`, read once per isolate; a script without them refuses every entry.
  - The command is `Submit(bytes)`, the signed entry as stored, hex in serde. Its format is 0x30.1.
  - `decide` is `ReleaseFold::decide(&anchor, &bytes)`. A follower's `verify` runs `ReleaseFold::check`, and `apply` runs `ReleaseFold::apply`.
  - The event is `Accepted(bytes)`, whose `Record` encoding is the canonical bytes, so every member digests the same bytes.
  - The entries are stored in `release_entry`.
  - `err_json` gives 409 with `prev` for `AlreadyInstalled`, and 422 with `Refusal::variant()` otherwise. `read_json` gives `{genesis, last, entries}`.
- **Bootstrapping without an outside record.** A `Release` member is joined with the genesis bytes (`Join.genesis`, hex). It accepts them against its own anchor (`Source::Anchor`) and bootstraps from the genesis's `Release` record. A member of another install, or one whose script has no anchor, refuses the join.
- **The `Release` view.** A site isolate, or the edge, finds the `Release` group by its `MemberRegistry` rows (`graftyard_lookup::read_genesis`), because a cold isolate does not hold the genesis that names the group's record yet. It reads the genesis, accepts it against its own anchor, and caches it for the isolate.
  - System chains' records (`Names`, `Tokens`, `Ids`) come from this view, both for a member's bootstrap check (`Source::Genesis`) and for the edge's lookups.
  - Milestone 1 has no entry that changes a genesis, so the cache is never refreshed.
- **`GRAFTYARD_GENESIS` is a test-faults shortcut only.** `SiteEnv::genesis_shortcut` and the variable's parse on the site, and `Ctx::system_record`'s read of it on the edge, compile only with `test-faults`. `deploy.mjs --genesis` and `deploy-edge.mjs --genesis` refuse to run without `--test-faults`. The test install `graft-m1-site-*` still uses the shortcut, because it has no anchor and its records predate the `Release` group.

### The endpoint

`POST /api/graftyard/v1/release/entries` on the git edge (`workers/edge/src/release.rs`), per `m1-cli.md`:

1. `accept_genesis` against the edge's anchor. A refusal answers 422 `{message, refusal}` and never reaches the group.
2. The `Release` group is created from the genesis's `Release` record, its joins carrying the genesis.
3. The commit goes through `commit_hedged` under a fresh request id.
4. The answer is 201 `{index, digest}`, 409 `{"message": "stale prev", prev, index}`, or 422.
5. After a 201, or a 409 whose `prev` is the posted entry's own digest (a retried install), the edge runs Genesis step 5:
   - It creates `names:<account_shard(account)>`, `ids`, and `tokens:0` from the genesis's records.
   - It commits `InitPlacement` of the install account, with the system chains' record, on its `Names` shard.
   - It creates `Authz(account)` and commits `Open` as an `Org`.

   Each step is idempotent: a join is, `InitPlacement` is refused `RecordExists`, and `Open` is refused `AlreadyOpened`. So a rerun of `install` repairs a step that failed. The other `Names` and `Tokens` shards are created on first use from the same genesis record.

### End to end with the CLI, on a throwaway install

**Pass.** `tests/m1/integ/throwaway.sh run`, then `delete` (`tests/m1/integ/raw/throwaway-20261101.log`). The site scripts were the product build: `worker-build --release`, no `test-faults`, and 0 of 30 test needles in the bundle. The edge, `graft-m1-gen-edge`, was a product build with the install's anchor. It was deployed by `deploy-edge.mjs --anchor install.toml --no-shard`, because `graftyard sites add` deploys no `GitShard` class yet.

| Step | Output |
| --- | --- |
| `graftyard init --account … --zone git.m1gen.example --customer-key customer.pub` | `install 01M6CQAY5WAY9MYTMWS57WWZ4F: wrote install.toml …; created nothing` |
| `graftyard sites add --script graft-m1-gen-{a,b,c} --region {enam,weur,apac} --bundle <product build>` | Buckets in ENAM, WEUR, APAC. Phases 1 and 2 on each. Latency check rtt 2→0 140 ms, 2→1 229 ms, 0→1 95 ms, 0→2 231 ms |
| `graftyard genesis --out genesis.unsigned --release-key release.pub` | Drain and executor keys written to Secrets Store. `wrote unsigned genesis genesis.unsigned (603 bytes): … 3 sites, system chains Tri on [0, 1, 2]` |
| `graftyard sign genesis.unsigned --out genesis.signed --key customer.key` | `signed: wrote genesis.signed (699 bytes), digest 4c2eaf0d…5524` |
| `install --genesis tampered.signed` (byte 100, bit 0 flipped) | `refused locally (BadSignature)` |
| The same tampered bytes posted straight to the edge | HTTP 422 `{"message":"the customer key's signature does not verify","refusal":"BadSignature"}` |
| `graftyard install --genesis genesis.signed --edge https://graft-m1-gen-edge…` | `installed: the Release group committed genesis 4c2eaf0d…5524 at index 3` (83 s, most of it creating the four groups) |
| `install` again | `already installed: the Release group holds this genesis (4c2eaf0d…5524) and is at index 4` |
| The replay posted straight to the endpoint | HTTP 409 with `prev` = the genesis digest. Every step-5 group answered `created: true` (it exists). `InitPlacement` was refused `RecordExists` on `names:24`, and `Open` was refused `AlreadyOpened` on `authz:1`, so both had committed on the first install |
| A git request for an owner that does not exist | HTTP 404 `Repository not found.`, after the edge read `Names` through the `Release` view, with no `GRAFTYARD_GENESIS` anywhere |
| The product scripts' bindings | `BUCKET_0..2`, `GRAFTYARD_BUNDLE`, `GRAFTYARD_CUSTOMER_KEY`, `GRAFTYARD_INSTALL`, `GRAFTYARD_SITES`, `MEMBER_0..2`, `REGISTRY_0..2`, `SITE_ID`. No test variable |

Then everything was deleted: the scripts `graft-m1-gen-a/b/c` and `graft-m1-gen-edge`, the buckets `graft-m1-gen-{a,b,c}-packs`, the secrets `graftyard-drain-0` and `graftyard-executor-0`, and the store `graft-ops-01m6cqay5way9mytmws57wwz4f`. The Secrets Store API refused to delete a store that still held secrets (`store_not_empty`), so the two secrets were deleted first. That step is now in `throwaway.sh delete`. Final listing: no `graft-m1-gen*` script or bucket, and no secrets store.

### BF-15 rerun on this pass's site script

**Pass.** `node tests/m1/wp7/run.ts bf15 --run i1 --queued 1000` (`tests/m1/wp7/raw/i1.bf15.jsonl`), on `graft-m1-site-*` with the Repo, Release, and `GitShard` changes:

| Case | Change committed at | Decided after it | 401 | Accepted | Entries | Dependency reads |
| --- | --- | --- | --- | --- | --- | --- |
| Revoke the stored token | `tokens:2279` index 21, 2.38 s in | 1,000 of 1,000 | 1,000 (`Unauthorized(Revoked)`) | 0 | 2 | 3 |
| Suspend the account | `names:453` index 164, 2.40 s in | 1,000 of 1,000 | 1,000 (`Unauthorized(Suspended)`) | 0 | 2 | 3 |

The git edge's token path checks the same rows. `clients.sh`'s revoked-token push is refused 401 by the edge's `authenticate`, which reads the `Tokens` shard through `fresh` (`m1-wp6.md`, "the real repository groups").

### What is still stubbed

- **Test install records.** `graft-m1-site-*` has no anchor, so its system chains' records come from `GRAFTYARD_GENESIS` (test-faults builds only). Only the throwaway install ran the product path.
- **`GitShard` from the CLI.** `graftyard sites add` deploys `Member` and `Registry` only (migration `v1`). The `GitShard` container class, its image, and migration `v2` come from `workers/site/deploy.mjs` on test installs. A product install has no git cache shard until `sites add` deploys one.
- **Later release entries.** Only a genesis is accepted, so `Keys`, `FormatsEnable`, and the rest are `NotGenesis`. The view is never refreshed.
- **Accounts and repositories.** These were created by operator commands (`tests/m1/integ/seed.ts`), as R14 still asks, not by a signup or REST.

### Design problems found (quoted, not edited)

1. **The `Release` group has no record source before it exists.** "A member with no committed config serves no read, grants no vote, and stores only that entry, after it reads the same record generation from the record's source itself (the `Names` leader, or genesis)" (`design/DESIGN.md`, Shape, `ReplyMeta` and Bootstrap). For the `Release` group itself, the genesis is the entry it is about to commit, so a member cannot read its record from a source. Here the members are joined with the genesis bytes and accept them against their own anchor, and that check is the source. The design should say so.
2. **A cold isolate cannot find the `Release` group by the lookup rule.** The rule starts from the chain's `PlacementRecord`, and the `Release` record is inside the genesis that the group holds. This pass finds the group by its `MemberRegistry` rows on each site (`graftyard_lookup::read_genesis`). The design's lookup rule should name that step for `Release`.
The CLI's missing `GitShard` is not a design problem. The design's Usage block has `graftyard sites add ... # DO classes, container class, bucket, trust anchor; latency-checked`, so the gap is in `cli/src/sites.rs`: `desired_bindings` and the `v1` migration name `Member` and `Registry` only. The CLI needs the `GitShard` class, its migration, and the container image and config. That belongs to the CLI owner, and is listed under "What is still stubbed" above.

## 2026-11-02 (M1-WP8): `sites add` deploys the git cache shard

`graftyard sites add` and `sites add-spare` now deploy each site's own `GitShard` class, as `workers/site/deploy.mjs` does. Each version gets:

- the `SHARD_<own id>` binding;
- the `--image` container, which must be digest-pinned;
- migration `v2`;
- `cpu_ms` 300000;
- the sweeper cron.

After the upload the CLI creates the class's Durable Object-managed container application. Details are in `m1-wp8.md`, "Fixes before the run", item 3.

**End to end: pass.** `tests/m1/integ/throwaway.sh run`, then `delete`, at 04:00 to 04:04Z (`tests/m1/integ/raw/throwaway-20261102.log`). The site scripts were the product build, with 0 of 34 test needles. The edge was deployed without `--no-shard`.

- `sites add` printed `created container application graft-m1-gen-{a,b,c}-gitshard`. The latency check read 0→2 159 ms, among others.
- The rest of the run matched the 2026-11-01 table:
  - `genesis` and `sign` produced digest `1d23451a…bfb86`;
  - the tampered genesis was refused locally (`BadSignature`) and with HTTP 422 by the edge;
  - `install` reported `committed genesis … at index 3`;
  - the replay reported `already installed … at index 4`, with `RecordExists`;
  - a git request for an owner that does not exist got HTTP 404 after a `Names` read through the `Release` view.
- Each site binds only its own `SHARD_<n>`, with no `script_name`. Each has a container application of `scheduling_policy: durable_object` on that namespace, the cron `*/15 * * * *`, and migration tag `v2`.
- The product scripts' bindings: `BUCKET_0..2`, `GRAFTYARD_BUNDLE`, `GRAFTYARD_CUSTOMER_KEY`, `GRAFTYARD_INSTALL`, `GRAFTYARD_SITES`, `MEMBER_0..2`, `REGISTRY_0..2`, `SHARD_<own>`, `SITE_ID`. No test variable.
- Then everything was deleted: four scripts, three container applications (`throwaway.sh delete` now deletes them by name), three buckets, two secrets, and the store. Final listing: `left: scripts [], buckets []`.
