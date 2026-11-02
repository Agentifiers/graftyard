//! Milestone 1's minimal identity (`build/BUILD-PLAN.md`, M1-WP7; `decisions/DECISIONS.md`, "Milestone 1 builds
//! minimal identity"): the `Ids` group and `DbId` blocks, `Names` login claims, `Account` rows, owner placement
//! records and `RepoOwner`, an owner's `Authz` record with admin grants and `RepoName` entries, and stored-token
//! authentication. Every group is a `graftyard_raft::Command` with its fold. Pure Rust: no Workers dependency, no
//! clock and no randomness but what the caller passes in.

mod authz;
mod cache;
mod credential;
mod enc;
mod ids;
mod names;
mod overlay;
mod placement;
mod tokens;

pub use authz::{
    AccountKind, AuthzCmd, AuthzErr, AuthzEvent, AuthzFold, AuthzOp, AuthzOut, AuthzRead, AuthzScratch, NameEpoch,
    OrgRole, RepoEntry, RepoPermission,
};
pub use cache::{IDENTITY_BUDGET_PER_ISOLATE, IdentityCache, Lookup, READ_CACHE, RequestClass};
pub use credential::{
    Actor, Credential, CredentialRefusal, IdentityDep, OrgId, Own, Principal, UserId, check_credential,
    credential_chains,
};
pub use ids::{IdBlock, IdCursor, IdsCmd, IdsErr, IdsEvent, IdsFold, IdsScratch};
pub use names::{
    AccountRow, Login, LoginKey, LoginRow, NAME_SHARDS, NameError, NameKey, NamesCmd, NamesErr, NamesEvent, NamesFold,
    NamesOp, NamesOut, NamesRead, NamesScratch, RESERVED, RepoName, RepoNameKey, account_shard, name_shard,
};
pub use placement::validate_record;
pub use tokens::{
    Family, OpaqueToken, RedrawExhausted, SHARDS_PER_FAMILY, ScopeSet, TokenError, TokenKind, TokenRow, TokensCmd,
    TokensErr, TokensEvent, TokensFold, TokensOp, TokensRead, TokensScratch, draw_secret, family_of, shard_of_hash,
    token_shard,
};

use graftyard_raft::Format;

/// Stored formats of the identity groups' commands and events. `crates/raft` uses family 0 and the test and
/// register folds family 1; no registry of family numbers exists yet, so these are this crate's choice.
pub const NAMES_FORMAT: Format = Format { family: 0x10, version: 1 };
pub const TOKENS_FORMAT: Format = Format { family: 0x11, version: 1 };
pub const AUTHZ_FORMAT: Format = Format { family: 0x12, version: 1 };
pub const IDS_FORMAT: Format = Format { family: 0x13, version: 1 };
