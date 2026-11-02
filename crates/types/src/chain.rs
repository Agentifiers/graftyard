//! `ChainId` and its text form (`design/DESIGN.md`, Shape, "Chains" and "Lifeboat feed").

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ids::{AccountId, DbId, NetworkId, RepoId};

/// 1,024 shards per home family (na, eu, apac).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TokenShard(pub u16);

/// 1,024 shards per install or front account, on `Tri`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NameShard(pub u16);

/// Serialized as its text form, so a chain has one spelling in JSON, export URLs, and member object names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum ChainId {
    Repo(RepoId),
    Network(NetworkId),
    Authz(AccountId),
    Identity(AccountId),
    Tokens(TokenShard),
    Names(NameShard),
    Actions(AccountId, u8),
    Ids,
    Release,
    /// A repository's metadata child group (`CHILD_GROUP_AT`).
    RepoMeta(RepoId),
}

/// The text form the export API uses for `{chain}`, which member object names also embed: `repo:<id>`,
/// `network:<id>`, `authz:<id>`, `identity:<id>`, `tokens:<shard>`, `names:<shard>`, `actions:<id>.<shard>`,
/// `repo-meta:<id>`, `ids`, or `release`.
impl fmt::Display for ChainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChainId::Repo(r) => write!(f, "repo:{}", r.0.0),
            ChainId::Network(n) => write!(f, "network:{}", n.0.0),
            ChainId::Authz(a) => write!(f, "authz:{}", a.0.0),
            ChainId::Identity(a) => write!(f, "identity:{}", a.0.0),
            ChainId::Tokens(s) => write!(f, "tokens:{}", s.0),
            ChainId::Names(s) => write!(f, "names:{}", s.0),
            ChainId::Actions(a, s) => write!(f, "actions:{}.{}", a.0.0, s),
            ChainId::Ids => f.write_str("ids"),
            ChainId::Release => f.write_str("release"),
            ChainId::RepoMeta(r) => write!(f, "repo-meta:{}", r.0.0),
        }
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("not a chain id: {0:?}")]
pub struct ParseChainError(pub String);

impl From<ChainId> for String {
    fn from(c: ChainId) -> String {
        c.to_string()
    }
}

impl TryFrom<String> for ChainId {
    type Error = ParseChainError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl FromStr for ChainId {
    type Err = ParseChainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || ParseChainError(s.to_owned());
        let id = |v: &str| v.parse::<i64>().ok().filter(|n| *n >= 0).map(DbId).ok_or_else(bad);
        let shard = |v: &str| v.parse::<u16>().map_err(|_| bad());
        match s.split_once(':') {
            None if s == "ids" => Ok(ChainId::Ids),
            None if s == "release" => Ok(ChainId::Release),
            None => Err(bad()),
            Some(("repo", v)) => Ok(ChainId::Repo(RepoId(id(v)?))),
            Some(("network", v)) => Ok(ChainId::Network(NetworkId(id(v)?))),
            Some(("authz", v)) => Ok(ChainId::Authz(AccountId(id(v)?))),
            Some(("identity", v)) => Ok(ChainId::Identity(AccountId(id(v)?))),
            Some(("tokens", v)) => Ok(ChainId::Tokens(TokenShard(shard(v)?))),
            Some(("names", v)) => Ok(ChainId::Names(NameShard(shard(v)?))),
            Some(("repo-meta", v)) => Ok(ChainId::RepoMeta(RepoId(id(v)?))),
            Some(("actions", v)) => {
                let (a, s) = v.split_once('.').ok_or_else(bad)?;
                Ok(ChainId::Actions(AccountId(id(a)?), s.parse().map_err(|_| bad())?))
            }
            Some(_) => Err(bad()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_form_round_trips_and_rejects_neighbours() {
        let all = [
            ChainId::Repo(RepoId(DbId(42))),
            ChainId::Network(NetworkId(DbId(7))),
            ChainId::Authz(AccountId(DbId(1))),
            ChainId::Identity(AccountId(DbId(1))),
            ChainId::Tokens(TokenShard(1023)),
            ChainId::Names(NameShard(0)),
            ChainId::Actions(AccountId(DbId(9)), 3),
            ChainId::Ids,
            ChainId::Release,
            ChainId::RepoMeta(RepoId(DbId(42))),
        ];
        for c in all {
            assert_eq!(c.to_string().parse::<ChainId>(), Ok(c), "{c}");
            assert_eq!(serde_json::to_string(&c).unwrap(), format!("\"{c}\""));
            assert_eq!(serde_json::from_str::<ChainId>(&format!("\"{c}\"")).unwrap(), c);
        }
        assert!(serde_json::from_str::<ChainId>(r#"{"Repo":42}"#).is_err());
        assert_eq!(ChainId::RepoMeta(RepoId(DbId(42))).to_string(), "repo-meta:42");
        // `#` separates the parts of a member's object name, so no chain's text may contain it or parse with it.
        for bad in ["repo:-1", "repo:", "repo:4#1", "actions:9", "names:70000", "release:1", "Repo:1", "ids "] {
            assert!(bad.parse::<ChainId>().is_err(), "{bad}");
        }
    }
}
