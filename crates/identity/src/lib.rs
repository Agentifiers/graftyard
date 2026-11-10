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
pub mod finish;
pub mod glob;
pub mod group_move;
mod identity;
mod ids;
mod names;
pub mod oidc;
mod org;
mod overlay;
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
    AccountKind, AgentRows, AuthzCmd, AuthzErr, AuthzEvent, AuthzFold, AuthzMore, AuthzOp, AuthzOut, AuthzRead,
    AuthzScratch, InstallationRow, InstallationState, NameEpoch, OrgRole, OrgRows, RepoEntry, RepoPermission,
};
pub use cache::{IDENTITY_BUDGET_PER_ISOLATE, IdentityCache, Lookup, READ_CACHE, RequestClass};
pub use credential::{
    Actor, Credential, CredentialRefusal, IdentityDep, OrgId, Own, Principal, UserId, WebSessionId, check_credential,
    credential_chains,
};
pub use identity::{
    AUDIT_KEEP, AccountCapture, AuditEvent, CredentialEvent, CredentialRef, IDENTITY_FORMAT, IDENTITY_FORMAT_V2,
    IdentityCmd, IdentityErr, IdentityEvent, IdentityFold, IdentityMore, IdentityOp, IdentityScratch, SessionEvent,
    SsoEvent, key_fingerprint,
};
pub use ids::{IdBlock, IdCursor, IdsCmd, IdsErr, IdsEvent, IdsFold, IdsScratch};
pub use names::{
    AccountRow, ClientRoute, Login, LoginKey, LoginRow, NAME_SHARDS, NameError, NameKey, NamesCmd, NamesErr,
    NamesEvent, NamesFold, NamesMore, NamesOp, NamesOut, NamesRead, NamesScratch, RESERVED, RepoName, RepoNameKey,
    Shortcode, SlugRow, account_shard, name_shard,
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
    family_of, fingerprint_shard, jti_shard, shard_of_hash, token_shard,
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
/// The `Audit` consumer's cursor on `Names` and `Tokens` shards (`crate::audit`), M3's close. Read and enabled on the
/// same terms as version 2.
pub const NAMES_FORMAT_V3: Format = Format { family: 0x10, version: 3 };
pub const TOKENS_FORMAT_V3: Format = Format { family: 0x11, version: 3 };
