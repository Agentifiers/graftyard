//! The finisher's half of a repository create and delete, `{"lifecycle": ..}`, repository format 32.17
//! (`decisions/OPEN-QUESTIONS.md`, F59; `design/SHARED.md`, "Multi-chain operations").
//!
//! The edge creates a repository in band: `CreateRepoWith` on the owner's `Authz` chain (the commit point, Authz
//! format 0x12.11, which records the settings asked for), `RepoOwner` on `Names`, the README push, then `InitRepo`.
//! It deletes one by turning the name `Deleted` on the owner's chain (the commit point), then the chain's `Deleted`.
//! When the edge dies in between, the `Audit` consumer on the owner's chain finishes the rest
//! (`graftyard_identity::finish::create` and `delete`) with this command, which carries no account and no key, as
//! every finisher step does. Its one dependency is the owner's `Authz` chain, which is what authorizes it:
//!
//! - `Init` writes the chain's first `Settings` and `Created` only while that chain names the repository `Live`, so a
//!   create the finisher aborted (`AuthzOp::AbortCreate`) can never be initialized after it. A chain that already has
//!   settings or `Created` commits nothing: the edge's `InitRepo` won the race.
//! - `Delete` writes the tombstone only while that chain holds the repository's name `Deleted` until the same
//!   `retain_until`. A chain already deleted commits nothing.
//!
//! A member reads 32.17 only after a site day put every member of the group on a release that lists it, and the
//! finisher sends it only once `FormatsEnable` (the test edge's `FORMATS_ENABLED`) enabled it.

use graftyard_identity::{AuthzFold, AuthzRead, IdentityDep, RepoEntry};
use graftyard_raft::{Format, TierSet};
use graftyard_types::{AccountId, ChainId, RequestId, Timestamp};
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

use crate::issues::{CHECK_RETENTION_MAX, RepoSettings};
use crate::repo::{RepoEvent, RepoFold, RepoScratch};
use crate::tiers::{created, deleted, get_rec};

/// Versions 15 and 16 are other work's.
pub const REPO_FORMAT_LIFECYCLE: Format = Format { family: 0x20, version: 17 };

/// Wire tags: JSON by name (`crates/git/tests/wire_tags.golden`). Add a variant only at the end.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifecycleOp {
    /// A create's first settings, with `owner` and `enterprise` as the owner's `Authz` names them.
    Init(RepoSettings),
    /// A delete's tombstone, kept until the name's `retain_until`.
    Delete { retain_until: Timestamp },
}

/// One finisher step on the repository chain, under the finisher's request id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lifecycle {
    pub op: LifecycleOp,
    pub request: RequestId,
}

/// Wire tags: JSON by name (`crates/git/tests/wire_tags.golden`). Add a variant only at the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifecycleErr {
    /// `Init`: the owner's chain does not name the repository `Live`; the create never committed or was aborted.
    NotLive,
    /// `Delete`: the owner's chain does not hold the repository's name `Deleted` until this `retain_until`.
    NotDeleted,
    /// `Init`: the settings name another owner than the chain's, or another enterprise than the owner's chain.
    WrongOwner,
    /// `Init`'s settings are out of range, or `Delete`'s `retain_until` is not after the entry.
    Invalid,
    /// `Init` on a deleted chain.
    Gone,
    /// The host did not pass the owner's `Authz` fold: fail closed.
    NotRead,
}

/// Whether the command wrote an event; a re-sent one finds its work done.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleOut {
    pub changed: bool,
}

impl Lifecycle {
    pub fn format(&self) -> Format {
        REPO_FORMAT_LIFECYCLE
    }

    /// `Init` writes the core `settings` and `created` rows and the metadata tier's `retention`, as `InitRepo` does.
    pub fn tiers(&self) -> TierSet {
        match self.op {
            LifecycleOp::Init(_) => TierSet::CORE.union(TierSet::METADATA),
            LifecycleOp::Delete { .. } => TierSet::CORE,
        }
    }

    /// The owner whose `Authz` chain authorizes the step.
    fn owner(&self, fold: &RepoFold) -> Option<AccountId> {
        match &self.op {
            LifecycleOp::Init(s) => Some(s.owner),
            LifecycleOp::Delete { .. } => fold.authz_owner(),
        }
    }

    pub fn deps(&self, fold: &RepoFold) -> SmallVec<[ChainId; 3]> {
        self.owner(fold).map(ChainId::Authz).into_iter().collect()
    }

    pub fn decide(
        &self,
        s: &RepoScratch<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[RepoEvent; 4]>, LifecycleOut), LifecycleErr> {
        let owner = self.owner(s.fold()).ok_or(LifecycleErr::NotRead)?;
        let authz: &AuthzFold = deps
            .iter()
            .find_map(|(c, d)| match (c, d) {
                (ChainId::Authz(o), IdentityDep::Authz(f)) if *o == owner && f.owner == Some(owner) => Some(f),
                _ => None,
            })
            .ok_or(LifecycleErr::NotRead)?;
        let repo = s.fold().repo.ok_or(LifecycleErr::NotLive)?;
        let none = Ok((SmallVec::new(), LifecycleOut { changed: false }));
        let done = LifecycleOut { changed: true };
        match &self.op {
            LifecycleOp::Init(settings) => {
                if deleted(s.core()).is_some() {
                    return Err(LifecycleErr::Gone);
                }
                if created(s.core()).is_some() || get_rec::<RepoSettings>(s.core(), "settings", &[]).is_some() {
                    return none;
                }
                let live = authz.names.values().any(|e| matches!(e, RepoEntry::Live { repo: r, .. } if *r == repo));
                if !live {
                    return Err(LifecycleErr::NotLive);
                }
                if Some(settings.owner) != s.authz_owner() || settings.enterprise != authz.enterprise() {
                    return Err(LifecycleErr::WrongOwner);
                }
                if !(1..=CHECK_RETENTION_MAX).contains(&settings.check_retention_days) || settings.merge_methods.is_empty()
                {
                    return Err(LifecycleErr::Invalid);
                }
                Ok((smallvec![RepoEvent::Settings(settings.clone()), RepoEvent::Created { actor: None }], done))
            }
            LifecycleOp::Delete { retain_until } => {
                if deleted(s.core()).is_some() {
                    return none;
                }
                let held = authz.names.values().any(
                    |e| matches!(e, RepoEntry::Deleted { repo: r, retain_until: t } if *r == repo && t == retain_until),
                );
                if !held {
                    return Err(LifecycleErr::NotDeleted);
                }
                if *retain_until <= s.at() {
                    return Err(LifecycleErr::Invalid);
                }
                Ok((smallvec![RepoEvent::Deleted { retain_until: *retain_until, actor: None }], done))
            }
        }
    }

    pub fn replay(&self, events: &[RepoEvent]) -> LifecycleOut {
        LifecycleOut { changed: !events.is_empty() }
    }
}

/// The first settings a create's finisher writes: the settings the create asked for (`init`, the JSON of a
/// `RepoSettings`, as `AuthzOp::CreateRepoWith` recorded it), with `owner` and `enterprise` as the owner's `Authz`
/// names them now.
pub fn init_settings(init: &str, owner: AccountId, enterprise: Option<AccountId>) -> Result<RepoSettings, String> {
    let mut s: RepoSettings = serde_json::from_str(init).map_err(|e| format!("a create's settings: {e}"))?;
    s.owner = owner;
    s.enterprise = enterprise;
    Ok(s)
}

/// The JSON a `Lifecycle` crosses the binding as: `format` first, so a member that does not read 32.17 refuses it
/// without reading the rest.
#[derive(Serialize, Deserialize)]
struct Wire {
    format: (u16, u16),
    op: LifecycleOp,
    request: RequestId,
}

impl Serialize for Lifecycle {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let f = REPO_FORMAT_LIFECYCLE;
        Wire { format: (f.family, f.version), op: self.op.clone(), request: self.request }.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Lifecycle {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Lifecycle, D::Error> {
        let w = Wire::deserialize(d)?;
        let f = REPO_FORMAT_LIFECYCLE;
        if w.format != (f.family, f.version) {
            return Err(serde::de::Error::custom(format!(
                "lifecycle command format {}.{} is not one this release reads",
                w.format.0, w.format.1
            )));
        }
        Ok(Lifecycle { op: w.op, request: w.request })
    }
}

impl graftyard_raft::Record for Lifecycle {
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(0x4c);
        graftyard_fold::canon::write(out, &(&self.op, self.request));
    }
}
