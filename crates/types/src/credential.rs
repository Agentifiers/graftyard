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

/// A stored token's row, a stateless installation token's `jti` row, or a web session, which names no row of its own
/// and is checked against the `Account` row's `sessions_not_before` by its issue time. The design's `SshKey` comes with
/// SSH pushes (M6-WP2).
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
    /// DESIGN's: a stateless installation token, revoked by a `JtiRevoked` row on `jti_shard(jti)`. It names no
    /// account, so a command under it is never an operator's.
    Jti {
        shard: TokenShard,
        jti: crate::Ulid,
    },
}

impl CredentialShard {
    /// Names nothing: a command `graft-ops` or a consumer submits.
    pub const NONE: CredentialShard = CredentialShard { account: None, key: None };

    /// Whether `graft-ops`, the CLI, or a finisher submitted the command: it names neither an account nor a key. An
    /// installation token names a key and no account, and is not the operator.
    pub fn is_operator(&self) -> bool {
        self.account.is_none() && self.key.is_none()
    }
}
