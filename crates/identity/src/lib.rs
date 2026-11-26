//! Identity: the `Ids` group and `DbId` blocks, the `Names` shards, the `Tokens` shards, and an owner's `Authz` chain.
//!
//! Milestone 1 built the minimal identity (`build/BUILD-PLAN.md`, M1-WP7; `decisions/DECISIONS.md`, "Milestone 1 builds
//! minimal identity"): login claims, `Account` rows, owner placement records and `RepoOwner`, an owner's `Authz`
//! record with admin grants and `RepoName` entries, and stored-token authentication. Milestone 3 adds App and OAuth app
//! records and their names (M3-WP1, `apps`), every token kind's routing and the short-lived codes (M3-WP2), each
//! account's login on its `account_shard` (F50), the `Audit` finisher's plans (`finish`), and the follow-ups the edge
//! sends after each commit point (`edge`). Every group is a
//! `graftyard_raft::Command` with its fold. Pure Rust: no Workers dependency, no clock and no randomness but what the
//! caller passes in.

mod access;
mod agents;
pub mod appauth;
mod apps;
pub mod audit;
mod authz;
mod cache;
mod compat;
mod credential;
pub mod edge;
mod enc;
pub mod export_token;
pub mod finish;
pub mod glob;
pub mod group_move;
mod identity;
mod ids;
pub mod lifeboat;
mod names;
pub mod oidc;
mod org;
mod overlay;
pub mod password;
mod placement;
pub mod sealed;
pub mod stateless;
mod tokens;

pub use access::{
    Decision, ReadAction, RepoAccess, Visibility, WriteAction, app_permission, authorize, authorize_app,
    authorize_read, authorize_write, permission, token_allows,
};
pub use agents::{AgentApp, AgentKind, SessionId, WorkspaceMode, install_account};
pub use apps::{
    Access, AppId, AppKey, AppRecord, CLIENT_SECRETS_MAX, ClientId, EventKind, InstallationId, OAuthAppRecord,
    PermissionSet, RsaPublic, SecretHash, Slug, Url,
};
pub use authz::{
    AccountKind, ActionsRows, ActionsSubstitution, AgentRows, AuthzCmd, AuthzErr, AuthzEvent, AuthzFold, AuthzMore,
    AuthzOp, AuthzOut, AuthzRead, AuthzScratch, CREATE_INIT_MAX, CursorRows, InstallationRow, InstallationState,
    NameEpoch, OrgRole, OrgRows, RepoEntry, RepoPermission, SubstitutionRows, actions_row,
};
pub use cache::{IDENTITY_BUDGET_PER_ISOLATE, IdentityCache, Lookup, READ_CACHE, RequestClass};
pub use credential::{
    Actor, Credential, CredentialRefusal, IdentityDep, OrgId, Own, Principal, UserId, WebSessionId, check_credential,
    credential_chains,
};
pub use identity::{
    AUDIT_KEEP, AccountCapture, AuditEvent, CredentialEvent, CredentialRef, GrantRow, IDENTITY_FORMAT,
    IDENTITY_FORMAT_V2, IDENTITY_FORMAT_V3, IDENTITY_FORMAT_V4, IdentityCmd, IdentityErr, IdentityEvent, IdentityFold,
    IdentityMore, IdentityOp, IdentityOut, IdentityScratch, McpGrant, McpRows, RecordAt, SessionEvent, SsoEvent,
    ThreadId, ThreadRow, UserRecord, UserRows, WatchRow, key_fingerprint,
};
pub use ids::{IdBlock, IdCursor, IdsCmd, IdsErr, IdsEvent, IdsFold, IdsScratch};
pub use names::{
    AccountRow, ClientRoute, CursorPart, ForkRows, Login, LoginKey, LoginRow, NAME_SHARDS, NameError, NameKey,
    NamesCmd, NamesErr, NamesEvent, NamesFold, NamesMore, NamesOp, NamesOut, NamesRead, NamesScratch, RESERVED,
    RepoName, RepoNameKey, Shortcode, SlugRow, account_shard, name_shard,
};
pub use org::{
    INVITATION_TTL, InstanceAuth, InvitationEvent, InvitationGrant, InvitationId, InvitationRow, Invitee, LdapConfig,
    OidcConfig, OrgPolicy, SamlConfig, SignInMethod, TeamEvent, TeamId, TeamRow, is_install_account,
};
pub use placement::validate_record;
pub use tokens::{
    AUTHORIZATION_CODE_TTL, CodeKind, CodeRow, DEVICE_CODE_TTL, Ed25519Public, Family, Fingerprint, GrantId, Jti, Kid,
    OpaqueToken, RedrawExhausted, RepoScope, SHARDS_PER_FAMILY, SIGN_IN_LINK_TTL, ScopeSet, SshKeyRow, TOKEN_SHARDS,
    TokenError, TokenFields, TokenKind, TokenRow, TokensCmd, TokensErr, TokensEvent, TokensFold, TokensMore, TokensOp,
    TokensRead, TokensScratch, VerifyingKeyRow, WriteScope, code_hash, draw_device_code, draw_secret, draw_secret_on,
    family_of, fingerprint_shard, jti_shard, lifeboat_shard, shard_of_hash, token_shard,
};

use graftyard_raft::Format;

/// Stored formats of the identity groups' commands and events. `crates/raft` uses family 0 and the test and
/// register folds family 1; no registry of family numbers exists yet, so these are this crate's choice.
pub const NAMES_FORMAT: Format = Format { family: 0x10, version: 1 };
pub const TOKENS_FORMAT: Format = Format { family: 0x11, version: 1 };
pub const AUTHZ_FORMAT: Format = Format { family: 0x12, version: 1 };
pub const IDS_FORMAT: Format = Format { family: 0x13, version: 1 };
/// Milestone 3's formats: every command that writes an event Milestone 1 did not have. A member reads them once its
/// release lists them, and the edge sends them only once `FormatsEnable` enabled them (`design/SHARED.md`, "Record
/// formats and the two-release rule"). Milestone 1's commands keep version 1, so their entries are unchanged.
pub const NAMES_FORMAT_V2: Format = Format { family: 0x10, version: 2 };
pub const TOKENS_FORMAT_V2: Format = Format { family: 0x11, version: 2 };
pub const AUTHZ_FORMAT_V2: Format = Format { family: 0x12, version: 2 };
/// AM1-WP2's `AgentApp` rows and the org's agent settings. Sent only once `FormatsEnable` enabled it.
pub const AUTHZ_FORMAT_V3: Format = Format { family: 0x12, version: 3 };
/// The `Audit` consumer's cursor on `Authz` chains (`crate::audit`), which finishes transfers and enterprise changes.
/// Read and enabled on the same terms as version 2.
pub const AUTHZ_FORMAT_V4: Format = Format { family: 0x12, version: 4 };
/// F59: `AuthzOp::CreateRepoWith`, whose entry also records `AuthzEvent::RepoCreate` so the `Audit` consumer can
/// finish or abort the create, and `AuthzOp::AbortCreate`, the abort's release of the name. Read and enabled on the
/// same terms as version 2. Versions 5 to 10 are other work's.
pub const AUTHZ_FORMAT_V11: Format = Format { family: 0x12, version: 11 };
/// M4-WP3: the owner's `SealKey`s and its org Actions secrets and variables (`AuthzOp::PutSealKey` to
/// `DeleteOrgVariable`, `AuthzEvent::SealKey`, `OrgSecret`, `OrgVariable`). Read and enabled on the same terms as
/// version 2.
pub const AUTHZ_FORMAT_V12: Format = Format { family: 0x12, version: 12 };
/// `app-hook-config`: `AuthzOp::SetAppHook`, `PATCH /app/hook/config`'s patch of an App's webhook and secret. It writes
/// only `AuthzEvent::App`, which members read since version 2, so the format guards the op alone. Read and enabled on
/// the same terms as version 2.
pub const AUTHZ_FORMAT_V13: Format = Format { family: 0x12, version: 13 };
/// `gh-token`: `AuthzOp::Install` of the Actions App on an owner (`crate::authz::actions_row`), which `acquirejob`
/// commits before it mints the owner's first job token. It writes only `AuthzEvent::Installation`, which members read
/// since version 2; the format guards `decide`'s rule for it, which needs no `Names` route and no App record. Read and
/// enabled on the same terms as version 2.
pub const AUTHZ_FORMAT_V14: Format = Format { family: 0x12, version: 14 };
/// BF-76 (`pin-map`): `AuthzOp::SetActionsSubstitution` and `AuthzEvent::ActionsSubstitution`, an org's tag
/// substitution and approved pin-map digest. Read and enabled on the same terms as version 2.
pub const AUTHZ_FORMAT_V15: Format = Format { family: 0x12, version: 15 };
/// The `Audit` consumer's cursor on `Names` and `Tokens` shards (`crate::audit`), M3's close. Read and enabled on the
/// same terms as version 2.
pub const NAMES_FORMAT_V3: Format = Format { family: 0x10, version: 3 };
/// AM3-WP2a: `NamesOp::RepoOwnerIn` and `NamesEvent::ForkOwner`, a fork's `RepoOwner` row with its network
/// (`build/BUILD-PLAN.md`, AM3-WP2a). Read and enabled on the same terms as version 2.
pub const NAMES_FORMAT_V4: Format = Format { family: 0x10, version: 4 };
pub const TOKENS_FORMAT_V3: Format = Format { family: 0x11, version: 3 };
/// AM2-WP1: `StoreWith` of a `ghu_` from `token/scoped`, which may carry `repos` and an MCP grant's `grant` and
/// `write` (`TokenFields::fits_at`). Read and enabled on the same terms as version 2.
pub const TOKENS_FORMAT_V4: Format = Format { family: 0x11, version: 4 };
/// M6-WP3's `lifeboat-revoke`: `StoreLifeboat`, whose entry revokes every earlier lifeboat token of the account with
/// `TokensEvent::LifeboatRevoked`. Read and enabled on the same terms as version 2; the lifeboat token route answers
/// 503 until it is enabled.
pub const TOKENS_FORMAT_V5: Format = Format { family: 0x11, version: 5 };
