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

/// Milestone 1 has stored tokens only. The design's `Jti` and `SshKey` keys come with installation tokens and SSH keys
/// (Milestone 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CredentialKey {
    Token { shard: TokenShard, hash: [u8; 32] },
}

impl CredentialShard {
    /// Names nothing: a command `graft-ops` or a consumer submits.
    pub const NONE: CredentialShard = CredentialShard { account: None, key: None };
}
