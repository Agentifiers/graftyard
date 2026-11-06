//! What a write command's leader re-reads for it once per entry (`design/DESIGN.md`, Shape, "The write path",
//! `CredentialShard`). Shared because `crates/raft`'s `Command::credential()` returns it and `crates/identity` reads it.

use serde::{Deserialize, Serialize};

use crate::chain::TokenShard;
use crate::ids::AccountId;

/// The actor's `Account` row and the `Tokens` shard row of the command's credential. Commands that `graft-ops` or a
/// consumer submits name neither. A missing, revoked, or suspended row refuses the command with 401.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CredentialShard {
    pub account: Option<AccountId>,
    pub key: Option<CredentialKey>,
}

/// A stored token's row, or a web session, which names no row of its own and is checked against the `Account` row's
/// `sessions_not_before` by its issue time. The design's `Jti` key comes with installation tokens (M3-WP4), and
/// `SshKey` with SSH pushes (M6-WP2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CredentialKey {
    Token {
        shard: TokenShard,
        hash: [u8; 32],
    },
    /// [inference: DESIGN's `CredentialShard` says web sessions "name only the `Account` row"; the issue time is what
    /// that row's `sessions_not_before` is compared with, and `decide` has no clock.]
    Session {
        issued: crate::ids::Timestamp,
    },
}

impl CredentialShard {
    /// Names nothing: a command `graft-ops` or a consumer submits.
    pub const NONE: CredentialShard = CredentialShard { account: None, key: None };
}
