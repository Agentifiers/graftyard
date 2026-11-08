//! The transfer's own `Settings` command, `{"transfer": ..}`, repository format 32.10 (`design/SHARED.md`,
//! "Multi-chain operations", Transfer and Enterprise change; `decisions/DECISIONS.md`, "A transfer's own `Settings`
//! command, and the move between placements pending").
//!
//! The repository chain's own `Settings` refuses any change of `owner` or `enterprise` (`IssueErr::OwnerChange`), so
//! an admin cannot move a repository by editing its settings. This command is the only one that changes them. It
//! carries no account and no key, as every finisher step does, and is sent only by the transfer's or the enterprise
//! change's finisher. `decide` reads the named owner's `Authz` chain as its one dependency and records the new owner
//! only when that chain holds the repository's name as `Reserved` or `Live` (the transfer's commit point committed),
//! and the new enterprise only when it is the one that chain names. Every other field of `RepoSettings` stays as it
//! was, or takes the default a repository with no `Settings` row reads as.
//!
//! A member reads 32.10 only after a site day put every member of the group on a release that lists it, and the
//! finisher sends it only once `FormatsEnable` (the test edge's `FORMATS_ENABLED`) enabled it.

use graftyard_identity::{AuthzFold, IdentityDep, RepoEntry};
use graftyard_raft::{Format, TierSet};
use graftyard_types::{AccountId, ChainId, DbId, RequestId};
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

use crate::issues::{CHECK_RETENTION_DEFAULT, MergeMethod, RepoSettings, Visibility};
use crate::refs::RefName;
use crate::repo::{RepoEvent, RepoScratch};
use crate::tiers::get_rec;

pub const REPO_FORMAT_TRANSFER: Format = Format { family: 0x20, version: 10 };

/// The new owner and its enterprise, under the finisher's request id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferSettings {
    pub owner: AccountId,
    pub enterprise: Option<AccountId>,
    pub request: RequestId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferErr {
    /// The owner's `Authz` chain holds no `Reserved` or `Live` name for this repository: no transfer committed.
    NotReserved,
    /// The enterprise is not the one the owner's `Authz` chain names.
    WrongEnterprise,
    /// The host did not pass the owner's `Authz` fold: fail closed.
    NotRead,
}

/// What `decide` recorded: the settings now in force.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferOut {
    pub owner: AccountId,
    pub enterprise: Option<AccountId>,
    /// Whether this command wrote an event; a re-sent one finds the settings already in force.
    pub changed: bool,
}

/// What a repository with no `Settings` row reads as (`crate::issue_ops`, `graftyard_api::gql`): private, issues on,
/// every merge method allowed, 90 days of check retention, and `main`.
fn defaults(owner: AccountId) -> RepoSettings {
    RepoSettings {
        owner,
        enterprise: None,
        visibility: Visibility::Private,
        default_branch: RefName::parse(b"refs/heads/main").expect("a valid ref"),
        archived: false,
        description: None,
        homepage: None,
        topics: SmallVec::new(),
        has_issues: true,
        has_wiki: false,
        has_projects: false,
        merge_methods: smallvec![MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase],
        delete_branch_on_merge: false,
        allow_auto_merge: false,
        allow_update_branch: false,
        check_retention_days: CHECK_RETENTION_DEFAULT,
    }
}

/// Whether the owner's fold holds the repository's name as the transfer's reservation or as its live name.
fn names_repo(f: &AuthzFold, repo: graftyard_types::RepoId) -> bool {
    f.names.values().any(|e| match e {
        RepoEntry::Reserved { repo: r, .. } | RepoEntry::Live { repo: r, .. } => *r == repo,
        RepoEntry::Alias { .. } | RepoEntry::Deleted { .. } => false,
    })
}

impl TransferSettings {
    pub fn format(&self) -> Format {
        REPO_FORMAT_TRANSFER
    }

    /// `Settings` writes the core `settings` row and the metadata tier's `retention`.
    pub fn tiers(&self) -> TierSet {
        TierSet::CORE.union(TierSet::METADATA)
    }

    /// The named owner's `Authz` chain, and nothing else: the command has no credential.
    pub fn deps(&self) -> SmallVec<[ChainId; 3]> {
        smallvec![ChainId::Authz(self.owner)]
    }

    pub fn decide(
        &self,
        s: &RepoScratch<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[RepoEvent; 4]>, TransferOut), TransferErr> {
        let authz = deps
            .iter()
            .find_map(|(c, d)| match (c, d) {
                (ChainId::Authz(o), IdentityDep::Authz(f)) if *o == self.owner && f.owner == Some(self.owner) => {
                    Some(f)
                }
                _ => None,
            })
            .ok_or(TransferErr::NotRead)?;
        let repo = s.fold().repo.ok_or(TransferErr::NotReserved)?;
        if !names_repo(authz, repo) {
            return Err(TransferErr::NotReserved);
        }
        if graftyard_identity::AuthzRead::enterprise(authz) != self.enterprise {
            return Err(TransferErr::WrongEnterprise);
        }
        let cur: Option<RepoSettings> = get_rec(s.core(), "settings", &[]);
        let out = TransferOut { owner: self.owner, enterprise: self.enterprise, changed: true };
        if cur.as_ref().is_some_and(|c| c.owner == self.owner && c.enterprise == self.enterprise) {
            return Ok((SmallVec::new(), TransferOut { changed: false, ..out }));
        }
        let mut next = cur.unwrap_or_else(|| defaults(self.owner));
        next.owner = self.owner;
        next.enterprise = self.enterprise;
        Ok((smallvec![RepoEvent::Settings(next)], out))
    }

    pub fn replay(&self, events: &[RepoEvent]) -> TransferOut {
        TransferOut { owner: self.owner, enterprise: self.enterprise, changed: !events.is_empty() }
    }
}

/// The JSON a `TransferSettings` crosses the binding as: `format` first, so a member that does not read 32.10
/// refuses it without reading the rest.
#[derive(Serialize, Deserialize)]
struct Wire {
    format: (u16, u16),
    owner: i64,
    enterprise: Option<i64>,
    request: RequestId,
}

impl Serialize for TransferSettings {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let f = REPO_FORMAT_TRANSFER;
        Wire {
            format: (f.family, f.version),
            owner: self.owner.0.0,
            enterprise: self.enterprise.map(|e| e.0.0),
            request: self.request,
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for TransferSettings {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<TransferSettings, D::Error> {
        let w = Wire::deserialize(d)?;
        let f = REPO_FORMAT_TRANSFER;
        if w.format != (f.family, f.version) {
            return Err(serde::de::Error::custom(format!(
                "transfer command format {}.{} is not one this release reads",
                w.format.0, w.format.1
            )));
        }
        Ok(TransferSettings {
            owner: AccountId(DbId(w.owner)),
            enterprise: w.enterprise.map(|e| AccountId(DbId(e))),
            request: w.request,
        })
    }
}

impl graftyard_raft::Record for TransferSettings {
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(0x54);
        graftyard_fold::canon::write(out, &(self.owner, self.enterprise, self.request));
    }
}
