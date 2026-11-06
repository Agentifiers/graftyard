# M3-WP3: Authz, Identity, and sign-in, the pure half

BUILD-PLAN M3-WP3 asks for `AuthzEvent` and `IdentityEvent` on their chains, `authorize`, `authorize_write`, and `Decision`, the `Account` row read on every write, the edge's `Identity` write after a new credential, `InstanceAuth`, outside collaborators against org grants, invitations, transfer, and an org's enterprise change. This round builds the pure half and runs BF-3, BF-30, BF-33, BF-34, BF-35, and BF-36 in memory on real raft members. Nothing was deployed. The hosted half (the edge's routes, the site day, and the `AccountLogin` backfill) is not built.

## What was built

- **Authorization** (`crates/identity/src/access.rs`). `Decision` (`Allow`, `Forbidden`, `Hidden`), `ReadAction`, `WriteAction` with the role each needs, the `RepoAccess` trait over the repository chain's core tier, and `permission`, `authorize`, `authorize_write`.
  - A user's permission is the highest of: owner admin, a direct grant, a team's grant, the org's base permission for a member, a `Collaborator` row, and `Read` on an internal repository for a member of the org or its enterprise.
  - Access through membership needs the org in `sso_orgs` when the org's policy enforces SSO. A collaborator's does not.
  - Denial on a private repository is `Hidden` (404), on a public one `Forbidden` (403).
  - Pushes, issue commands, and merges all go through it now. A push asks `authorize_write(Push)`.
- **`Authz`** (`crates/identity/src/org.rs`, `authz.rs`). Teams, team grants, `OrgPolicy` (base permission, SSO enforced), org invitations with a 7-day TTL, `InstanceAuth`, the enterprise, and the transfer's `Reserved`, `Live`, and `Alias` steps.
  - Accepting an org invitation appends `Accepted`, `Member`, and each team's `Member` in one entry. An email invitation is accepted by the account that holds the email on `Names`, read as a dep.
  - `SetInstanceAuth` works only on the install's own chain and only from one of its admins. The operator and finishers are refused [inference: a `SiteAdmin` is an admin of the install account's org].
  - `SetEnterprise` is operator only. `ReserveTransfer` needs the user to administer both owners; the leader reads the source owner's `Authz`.
  - The M3 rows sit in the fold's trailing part, so a fold with none keeps Milestone 1's bytes.
- **Sessions** (`crates/identity/src/credential.rs`). `Credential::Session(WebSessionId)`. Its `CredentialShard` names the account and the session's issue time, and every write refuses it with `SignedOut` when the `Account` row's `sessions_not_before` is later. Stored-token wire bytes are unchanged; a session crosses the wire only when there is one.
- **`Identity`** (`crates/identity/src/identity.rs`). A new chain family, 0x14 format 1. `Create`, `AddCredential`, `RemoveCredential`, `SetPassword`, `SignIn`, `SignOut`, `LinkSso`.
  - `AddCredential` is refused unless the credential's `Tokens` row is live for the account; `RemoveCredential` is refused while it is.
  - `SignIn` reads the install's `Authz` and the account's `Account` row: a method other than `InstanceAuth` names is `WrongMethod`, a suspended account `Suspended`. The bootstrap link (`SignInMethod::Link`) is always admitted.
  - `finish::credential` is the `Audit` finisher: it lists a committed credential nobody listed and drops a revoked one.
- **Repository chain** (`crates/git`). `RepoEvent::Collaborator` (tag 25) and `Invitation` (26), core tier. `InviteCollaborator`, `AnswerInvitation`, `RemoveCollaborator` at format 32.6. An org member is refused with `OrgMember` (422), so a user is never both.
  - A user command's `deps` name the owner and the enterprise from `RepoSettings`, so a transfer's or enterprise change's `Settings` moves them by entry.
- **Transfer** (`finish::transfer`). The next step after the destination's `Reserved` name, in SHARED's order: the group's move when the placements differ, `Settings`, the destination's `Live`, the source's `Alias`, then `RepoOwner`. The move goes first, so no route names a placement the group is not in.
- **The edge's follow-ups** (`crates/identity/src/edge.rs`). The command lists the edge sends after each commit point:
  - After a signup's `ClaimLogin`: `SetAccountLogin`, `PutAccount`, and `Identity` `Created`.
  - A rename: the new `ClaimLogin`, the old `ReleaseLogin`, then `SetAccountLogin`.
  - After a credential's `Tokens` row: `AddCredential`.
  - After a verified sign-in: `SignIn`.
  The edge does not send them yet; that is the hosted half.

## The gates

All run on three real `graftyard_raft::Member`s per chain (`crates/identity/tests/common`), with every dep read through `fresh` on its own group, except where noted.

### BF-3: suspended user's push; one key from two edges. Pass.

- `a_suspended_users_pushes_are_all_refused_including_those_admitted_before_the_suspension` (`crates/git/tests/push.rs`, the stand-in group). 100 pushes admitted and waiting, then the suspension, then 100 more: 200 of 200 refused 401, no ref moved. The owner still pushes.
- `one_ssh_key_added_from_two_edges_at_once_gets_one_owner`. 200 keys, each added for two accounts at once: 100 in one entry, 100 in two. 200 of 200 have one owner and one `SshKey` row on every member; the second gets `KeyTaken`, and a follower's check refuses a second owner's row.

The SSH push of the suspended user is BF-52 (M6-WP2).

### BF-30: transfer and enterprise change. Pass.

- `a_transfer_keeps_every_route_on_the_group_and_names_the_new_enterprise`. A repository moves from org alpha (enterprise E1) to beta (E2), once across placements and once on the same placement. Each step is submitted twice, as a racing edge and the finisher would.
  - After every step, every route that resolves (old name, new name, and `/repositories/{id}` through `RepoOwner`) names a record the group is in or was in. At least one name resolves throughout. At the end both do, and the old one redirects.
  - The plan took 5 steps across placements and 4 on the same one. `Settings` named beta and E2.
  - A user who does not administer the source is refused `ReserveTransfer`. `SetEnterprise` from a user is refused; from the operator it commits.
- `after_settings_a_pushs_deps_name_the_new_owner_and_enterprise` (`crates/git/tests/push.rs`). After the transfer's `Settings`, a push's `deps` name beta and E2 and not alpha. Beta's grant lets WRITER push; alpha's admin is refused.

The group move itself is modelled as a step. The `Joint` change runs in M1-WP7's move Workflow, which this round does not drive.

### BF-33: device flow under a leader kill. Pass.

`bf33_device_flow_under_a_leader_kill_completes_or_restarts_cleanly`, 100 runs. The leader is killed at a different point in each quarter: before any follower stored the exchange, after the followers stored it but before the leader heard, and with the edge dying between `CodeUsed` and the token's `Stored`. The last quarter has no kill.

| Result | Count |
| --- | --- |
| Logins completed with one token each | 100 of 100 |
| Restarted with a new code after a spent code and no token | 25 |
| Replay of a used code refused (`CodeUsed`) | 100 of 100 |
| A code used after 900 s refused (`NoCode`) | 100 of 100 |

### BF-34: collaborators, invitations. Pass.

- `an_org_invitation_accepted_grants_in_one_entry_and_declined_grants_nothing`. Accept appends 3 events in one entry; decline grants nothing; a lapsed, answered, or someone else's invitation fails; an email invitation is accepted only by the email's holder.
- `collaborators_and_org_grants_never_overlap_and_acceptance_grants_in_one_entry` (`crates/git/tests/issues.rs`). Accepting a repository invitation appends `Accepted` and `Collaborator` in one entry. An invitation for an org member, or an acceptance after joining, is refused. A user who joins keeps the higher of the grant and the old `Collaborator` row.

`TestAccGithubRepositoryCollaborator` needs the hosted half.

### BF-35: listed at once. Pass.

`a_new_ssh_key_and_pat_are_listed_at_once_without_the_audit_consumer`: 100 of 100 key and PAT pairs are listed by a `fresh` read of the `Identity` fold right after the edge's two commits. `audit_finishes_a_credential_nobody_listed_and_only_a_committed_one_is_listed` covers the finisher.

### BF-36: InstanceAuth. Pass.

- `only_a_site_admin_writes_instance_auth_and_the_next_sign_in_follows_it`: a site admin sets OIDC; a member and the operator are refused; another owner's chain gets `NotInstall`.
- `a_sign_in_takes_only_the_method_instance_auth_names`: after the switch, a password sign-in is refused, OIDC and the bootstrap link pass, and a suspended account starts no session.

### The edge's writes

`the_edges_writes_keep_the_account_login_and_identity_in_step`: a signup, a sign-in, a PAT, and a rename, each step sent twice. The login read from `account_shard` alone follows the signup and the rename, and a late write of the old login is refused.

## Planted bugs

`tests/m3/wp3/plant.sh` applies each plant alone, runs the identity and git gate tests, and reverts. 13 of 13 were caught.

| Plant | Caught by |
| --- | --- |
| Suspension not checked on write | `every_write_reads_the_account_row_...`, `a_suspended_users_pushes_are_all_refused_...` |
| A taken SSH fingerprint not refused | `one_ssh_key_added_from_two_edges_...`, `every_credential_kind_is_refused_off_its_shard` |
| `sessions_not_before` not checked | `every_write_reads_the_account_row_...` |
| A push's deps keep the first owner | `after_settings_a_pushs_deps_name_the_new_owner_and_enterprise` |
| Transfer names go live before the move | `a_transfer_keeps_every_route_on_the_group_...` |
| A used code spent again | `bf33_...`, `a_device_code_yields_one_token_and_lapses_at_900_s` |
| Accept without `Member` | `an_org_invitation_accepted_grants_in_one_entry_...` |
| `Collaborator` invitation for an org member | `collaborators_and_org_grants_never_overlap_...` |
| An invitation never lapses | `an_org_invitation_accepted_grants_in_one_entry_...` |
| `permission` ignores collaborators | `hidden_forbidden_and_the_higher_of_grant_and_collaborator`, `collaborators_and_org_grants_never_overlap_...` |
| `AddCredential` without a live `Tokens` row | `audit_finishes_a_credential_nobody_listed_...` |
| `InstanceAuth` written by anyone | `only_a_site_admin_writes_instance_auth_...` |
| A suspended account signs in | `a_sign_in_takes_only_the_method_instance_auth_names` |

## Formats and tags taken

- `AuthzEvent`: Team 2, TeamGrant 3, Policy 6, Invitation 7, InstanceAuth 10, Enterprise 13. 11 and 12 stay free for `Deleted` and `Undeleted`. `AuthzOp` command tags 10 to 23, format 0x12.2.
- `IdentityEvent`: Created 0, Credential 1, Sso 2, Audit 3, Session 5; 4 is kept for `Record`. Format 0x14.1, a new family.
- `CredentialKey::Session` encodes as 3; 1 and 2 stay free for `Jti` and `SshKey`.
- `RepoEvent::Collaborator` 25, `Invitation` 26, format 32.6.

## What the hosted half needs

- A site day that ships a release reading 0x10.2, 0x11.2, 0x12.2, 0x14.1, and 32.6, then `FormatsEnable` (or the edge's `FORMATS_ENABLED`) for them. Until then the edge must not send M3 commands.
- The edge's routes: signup, sign-in, rename, `rest:orgs`, `rest:teams`, `rest:users`, `rest:collaborators`, `rest:deploy-keys` over HTTPS, and the OAuth web and device flows, each sending `crate::edge`'s steps.
- The `AccountLogin` backfill for accounts seeded before F50: `finish::account_login` per account, then a run that shows the edge renders every login with `LOGINS` empty.
- The `Audit` consumer submitting `finish::credential` and `finish::transfer`, and the move Workflow driving `TransferStep::Move` through `Joint`.
- `graftyard admin bootstrap`.
