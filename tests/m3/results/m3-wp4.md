# M3-WP4: Stateless installation tokens in `graft-ops`, the pure half

BUILD-PLAN M3-WP4 asks for `InstallationClaims` and `ghs_` tokens as compact JWS with `kid`, minting with a signing key wrapped by a Secrets Store key and cached at most `SIGNING_KEY_CACHE`, `TokensEvent::VerifyingKey` on `TokenShard(0)`, `DELETE /installation/token` committing `JtiRevoked` on `jti_shard`, App JWTs verified against `AppRecord.keys`, and the `rest:apps` and `github-apps` surfaces. This round builds the pure half and runs BF-27 and BF-32 in memory on real raft members, with key rotation and the installation rows that `authorize` reads. Nothing was deployed. `graft-ops`, the edge's routes, and the site day are not built.

## What was built

- **Tokens** (`crates/identity/src/stateless.rs`). A token is `ghs_`, the App id in decimal, `_`, then a compact JWS. This is GitHub's `ghs_APPID_JWT` (`research/agents.md`, finding 4.3). The header is `{alg: EdDSA, typ: JWT, kid}`. The payload is `iss`, `ins`, `repos` (absent for all), `perms`, `jti` (ULID text), `iat`, and `exp`, in whole seconds.
  - Length varies with the claims: 338 characters for one repository and one permission in the BF-32 gate [measured]. Nothing assumes 40. Every token matches `ghs_[A-Za-z0-9\.\-_]{36,}`, Renovate's pattern (unit test).
  - `parse` is the edge's offline check. `verify` reads the `kid`'s `VerifyingKey` row, refuses a retired key, and checks the signature (`verify_strict`), the lifetime (at most 1 h), `iat` against `CLOCK_SKEW` (60 s [guess]), and `exp`.
- **Minting.** `KeySource` is the host's unwrap of the Secrets Store-wrapped key. `Minter::mint` asks it at most once per `SIGNING_KEY_CACHE` and counts the fetches. `Grant` is what the token request resolved. The `jti` takes the host's 80 random bits, so `crates/identity` stays free of randomness.
- **The credential** (`crates/identity/src/credential.rs`, `crates/types`). `Credential::Stateless(Box<InstallationClaims>)`, `Actor::App { app, installation }`, and `CredentialKey::Jti { shard, jti }`. `Principal::installation(&claims)` builds one.
  - The leader reads the `jti_shard` once per entry, as DESIGN's `CredentialShard` says. Any held `JtiRevoked` row refuses the command with 401 `Revoked`. `decide` has no clock; a row lapses at the token's own `exp`, after which the edge refuses the token anyway.
  - `CredentialShard::is_operator()`: no account and no key. An installation token has no account either, so every check that read `account.is_none()` as "the operator" now reads `is_operator()`, in `Tokens`, `Names`, `Authz`, and `Identity`.
- **`DELETE /installation/token`.** `revoke_command(&claims)` is `RevokeJti` on `jti_shard` under the token itself. The leader accepts it from that token or from `graft-ops`, and refuses any other. A second delete is refused 401 `Revoked`, which the edge answers 204, as GitHub does for a revoked token [inference].
- **Installations on the owner's `Authz` chain** (`authz.rs`). `AuthzEvent::Installation { id, row: Option<InstallationRow>, not_before }`. `InstallationRow` is the App, `RepoScope`, and `PermissionSet`.
  - `AuthzOp::Install` commits only after the id's `Names` route names this App and owner, its commit point. The permissions must be within the App's record, which the leader reads on `app_owner`'s `Authz` chain.
  - A narrowing (a repository or a permission dropped or lowered) and `Uninstall` move `not_before` to the entry's time. A follower refuses `not_before` moving back or an installation changing its App.
- **`authorize` for an App** (`access.rs`, `app_permission`). SHARED: "`authorize` intersects the claims with the installation's current selection in the `Authz` fold". The installation must be live on the repository's owner, for the token's App, with `iat >= not_before`. The repository must be in both the claims and the row. `contents` gives `Read` or `Write`; any other permission gives `Read` [inference]. The lower of the claims' and the row's level wins.
- **App JWTs and client secrets** (`appauth.rs`). `verify_app_jwt` checks RS256 under any of `AppRecord.keys` (DER SPKI). `iss` is the App id (number or string) or its client id, `exp - iat` is at most 600 s, and `iat` is at most 60 s ahead. `client_secret_ok` compares SHA-256 against `client_secrets`, all of them, so the time does not say which one matched.
  - New dependency: `rsa` 0.9 with no default features plus `sha2`. Only public-key verification is used, so RUSTSEC-2023-0071 (decryption timing) does not apply. It pulls in no `getrandom`, and the edge builds for wasm32 with it.
- **The token request** (`stateless::access_token_grant`). Verify the App JWT, find the live installation of that App, then `narrow` by `repositories`, `repository_ids`, and `permissions`. A permission the App or the installation lacks is 422 `NotGranted`, as is a repository outside the selection. More than `TOKEN_REPOS_MAX` (500 [unverified]) is refused.
- **Rotation** (`rotation_step`). Publish the new `kid` first. Switch the key source after one token lifetime. Retire the old `kid` at the switch plus `SIGNING_KEY_CACHE` plus one token lifetime plus `CLOCK_SKEW`, because an isolate that cached the old key may sign with it for up to an hour after the switch.
- **Pushes under an installation token** (`crates/git`). Push format 0x20.7 carries actor tag 2 (`App`) and credential tag 2 (the claims, canonically encoded). A release that does not read it answers `Unsupported`. `may_write` goes through `app_permission`. The stand-in gained `install` and `revoke_jti`.

## The gates

All run on three real `graftyard_raft::Member`s per chain (`crates/identity/tests/common`), with every dep read through `fresh` on its own group. The git gates use the stand-in group (`crates/git/src/standin.rs`), whose identity reads come from seeded folds.

### BF-27: revoke a `jti` with 1,000 commands queued. Pass.

- `bf27_jti_revoked_with_1000_commands_queued` (`crates/identity/tests/wp4.rs`). An App installed on an org; a token minted, verified at the edge against `TokenShard(0)`, and turned into a principal. One command decided before the revocation reads the `jti` row and passes it: it is refused 403, not 401, because the token is not the operator. Then 1,000 commands queue on the org's `Authz` leader. `DELETE /installation/token` commits `JtiRevoked` on `jti_shard` under the token. 1,000 of 1,000 are refused 401 `Revoked` [measured]. There were 2 dependency reads across 8 log entries, one per entry.
- `bf27_a_revoked_jti_refuses_all_1000_queued_pushes` (`crates/git/tests/push.rs`). The token pushes once and the ref moves. Then 1,000 pushes are admitted and wait, the `jti` is revoked, and 1,000 of 1,000 are refused 401 [measured]. So is the next push. Only the first ref moved.
- `delete_installation_token_is_the_tokens_own_and_idempotent`: another token's delete is 403, a stored-token user's is 401, a second delete is 401 `Revoked`, the operator's is a no-op, and the shard holds one row that lapses at `exp`.

### BF-32: App JWT and OAuth app secret, no secret in `Identity`. Pass, pure half.

- `bf32_an_octokit_app_jwt_verifies_against_app_record_keys_and_mints`. `crates/identity/tests/fixtures/app-jwt.json` holds three JWTs made by `universal-github-app-jwt` 2.2.2, the signer `@octokit/app` uses, with a throwaway RSA-2048 key. Only the public half is committed. The payload is Octokit's: `iat` 30 s back, `exp` 600 s after `iat`, `iss` a number.
  - The App registered as M3-WP1 does it: slug claim, record, then route. Its JWT verifies against the record's key, read fresh from the owner's `Authz` leader.
  - Refused: a JWT from a key not in the record (`Signature`), one naming another App (`Issuer`), one tampered, one at `exp` (`Expired`), and one 120 s early (`NotYet`).
  - The token request narrows to one repository and `contents: read`. `issues: write`, which the App lacks, is 422. A repository the installation cannot see is 422. An unknown installation is 404.
  - The minted token verifies at the edge, and `authorize_write(Push)` is `Forbidden` on the selected repository (read only) and `Hidden` on the other.
- `bf32_a_wrong_oauth_secret_is_refused_and_no_identity_chain_holds_a_client_secret`. An OAuth app registered with its client id claim first. The right secret passes; a secret one character off and an empty one fail, which the edge renders 401. A dump of every `Identity` chain's committed events and fold holds neither the secret's text nor even its hash. The owner's `Authz` dump holds the hash and not the text.

The `@octokit/app` flow over HTTPS, and `rest:apps` and `github-apps` at T0, need the hosted half.

### Rotation with an overlap window. Pass.

`key_rotation_keeps_both_keys_valid_through_the_overlap`, on `TokenShard(0)`:

1. Kid 2 is published while kid 1 signs. `Wait` holds until one lifetime has passed, then `SignWithNew`.
2. An isolate that fetched kid 1 just before the switch keeps signing with it until its cache lapses at `SIGNING_KEY_CACHE`; a fresh isolate signs with kid 2.
3. Kid 1's `retired` is set by `Retire`, after the last `exp` it could have signed. Inside the overlap, both kids' tokens verify.
4. After `retired`, a kid 1 token, as a stolen key would sign one, fails `Retired`, and kid 2's pass. A `PutVerifyingKey` that changes kid 2's key is refused `KeyChanged`.

The overlap from publish to retire was 10,860 s, and kid 1 verified for 7,260 s after the switch [measured, in the test's clock].

### Installations. Pass.

`an_installation_is_bounded_by_its_app_and_uninstall_fails_its_tokens_at_once`:

- An installation wider than its App's permissions is refused `BadRecord`. One with no `Names` route is refused `NotClaimed`.
- A token of another installation gets nothing.
- After a narrowing to repository `two`, the old token gets nothing anywhere: `not_before` moved past its `iat`. A newer token gets `Write` on `two` and nothing on `one`, though its claims name all.
- After uninstall, every token gets nothing. A second uninstall is a no-op, and an unknown id is refused.
- The token itself cannot uninstall: 403. Two members hold the same rows.

This test caught a real bug while it was being written: `Install` compared the new scope against the old one backwards, so a narrowing did not move `not_before`. It was fixed before the first commit of the test.

## Planted bugs

`tests/m3/wp4/plant.sh` applies each plant alone, runs the identity (`--lib`, `wp4`, `m3`, `wp3`) and git push tests, and reverts. 14 of 14 were caught. The first run caught 12. `narrow-ignores-app` and `claims-not-intersected` escaped it, so two asserts were added (commit `9ae66ed`), and a rerun of those two caught both.

| Plant | Caught by |
| --- | --- |
| The leader does not read the `jti` row | `bf27_jti_revoked_...`, `bf27_a_revoked_jti_refuses_all_1000_queued_pushes`, `delete_installation_token_...` |
| A token with no account counts as the operator in `Authz` | `bf27_jti_revoked_...`, `an_installation_is_bounded_...` |
| Anyone's token may revoke a `jti` | `bf27_jti_revoked_...`, `delete_installation_token_...`, `a_revoked_jti_lapses_at_exp` |
| A retired `kid` still verifies | `key_rotation_keeps_both_keys_valid_...` |
| The signing key cache never lapses | `key_rotation_...`, `the_signing_key_is_cached_for_at_most_signing_key_cache` |
| Rotation signs with a key the moment it is published | `key_rotation_...`, `rotation_signs_with_the_new_key_after_one_lifetime_...` |
| Narrowing ignores the App's permissions | `narrowing_never_exceeds_the_installation_or_the_app` (after `9ae66ed`) |
| Narrowing names a repository outside the selection | `narrowing_never_exceeds_the_installation_or_the_app` |
| An App JWT's signature is not checked | `bf32_an_octokit_app_jwt_...` |
| A wrong client secret is accepted | `bf32_a_wrong_oauth_secret_...` |
| An installation wider than its App commits | `an_installation_is_bounded_...` |
| An installation commits before its `Names` route | `an_installation_is_bounded_...` |
| `authorize` skips `not_before` | `an_installation_is_bounded_...` |
| `authorize` honors the claims alone | `an_installation_is_bounded_...` (after `9ae66ed`) |

## Formats and tags taken

- `AuthzEvent::Installation` 5, as the comment in `authz.rs` reserved it. `AuthzOp::Install` 24, `Uninstall` 25. Format 0x12.2, which M3-WP1 already introduced; no new `Authz` format.
- `CredentialKey::Jti` encodes as 1, as reserved; `SshKey` keeps 2.
- `RevokeJti` and `PutVerifyingKey` were built in M3-WP2 (`TokensOp` 0x11.2); unchanged but for who may submit `RevokeJti`.
- Push format 0x20.7 (`REPO_FORMAT_STATELESS`): actor tag 2 and credential tag 2. Versions 3 to 6 are the other repository commands'.
- `AuthzMore.installations` is a new last field of the `Authz` fold's trailing part, as M3-WP3 added `org`. Postcard is positional, so an `Authz` blob whose trailing part an earlier M3 build wrote does not decode under this one [inference from postcard's tuple encoding]. No deployed fold holds a trailing part: none is written before `FormatsEnable` enables 0x12.2 (`crates/identity/src/compat.rs`).

## What the hosted half needs

- **A site day** that ships a release reading 0x20.7 next to 0x10.2, 0x11.2, 0x12.2, 0x14.1, and 32.6, then `FormatsEnable` (or the edge's `FORMATS_ENABLED`) for them. Until then the edge sends no push under a `ghs_` token, and no `Install`.
- **The `AuthzMore` trailing part.** Before `FormatsEnable` for 0x12.2, check that no deployed `Authz` fold holds a trailing part; one written by an earlier M3 build would not decode.
- **`graft-ops`.** A Worker with a `KeySource` that unwraps the signing key with the `MintWrapKey` Secrets Store key; a `fetch()` route the edge's binding calls with a `Grant`; and `graftyard mint-key rotate`, driving `rotation_step` and committing `PutVerifyingKey`.
- **The edge.**
  - `POST /app/installations/{id}/access_tokens`: read `NameKey::Installation` for the owner and `NameKey::App` for the App's owner, read both `Authz` folds fresh, call `access_token_grant`, and mint through `graft-ops`. Render GitHub's body with `token`, `expires_at`, `permissions`, and `repositories`.
  - `DELETE /installation/token`: verify, commit `revoke_command`, answer 204.
  - `authenticate`: a `ghs_` token goes through `stateless::verify` against a `TokenShard(0)` read, with `READ_CACHE`, and a read-class request checks the `jti` row within `READ_CACHE`.
  - The App's installation flow: `RegisterInstallation`, then `Install`.
- **The JSON wire forms.** Issue and merge commands (`issue_ops::call`, `merge_call`) do not carry a stateless principal yet. They send an all-zero hash, so such a command fails closed with 401. An App that comments or merges needs a format for them like 0x20.7.
- **BF-32 over HTTPS** with `@octokit/app` against the deployed edge, and BM-3's revoked stateless token in journey 4.
