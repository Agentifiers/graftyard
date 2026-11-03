//! The repository chain's `Push` command and its ref fold (`design/SHARED.md`, "Refs and pushes"; `design/DESIGN.md`,
//! Shape, "The write path"). A `graftyard_raft::Command`: pure, deciding each update's `expect` against the fold
//! plus the earlier commands of the same entry.
//!
//! The fold holds what M1 needs of the repository's core tier: its refs, and the `RepoSettings` owner that names the
//! owner's `Authz` chain in `deps`. `Genesis` and `Settings` are later work, so a fold is built with its owner.

use std::collections::BTreeMap;

use graftyard_fold::{MemStore, Overlay, Read, TierStore, Tiered, Val, fold_digest, has};
use graftyard_identity::{
    Actor, AuthzRead, Credential, CredentialRefusal, IdentityDep, Own, Principal, RepoPermission, ScopeSet, UserId,
    check_credential, credential_chains,
};
use graftyard_raft::{Command, Enc, Ev, Fold, Format, LogEntry, Record, Scratch, TierSet};
use graftyard_types::{
    AccountId, ChainId, CredentialShard, Digest, Index, RepoId, RequestId, SiteSet, Tier, Timestamp, Ulid,
};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::issues::{
    BranchRule, CheckConclusion, CheckRunUpdate, CheckSource, CheckSuiteUpdate, CommentEvent, HeadSpec, IssueCapture,
    IssueChange, IssueNumber, LabelEvent, PrCapture, RepoSettings, ReviewEvent, StatusUpdate,
};
use crate::key::R2Key;
use crate::pack::{Archived, PackId, PackRef, Promoted};
use crate::refs::{Atomicity, ObjectId, RefName, RefUpdate};
use crate::tiers::{RepoTiers, get_rec, ref_key, tier_of};

/// The repository chain's command records. Version 2 adds `Push.agent`; an edge sends it only once `FormatsEnable`
/// has enabled it. No registry of family numbers exists yet (`crates/identity` took 0x10 to 0x13), so 0x20 is this
/// crate's choice.
pub const REPO_FORMAT_V1: Format = Format { family: 0x20, version: 1 };
pub const REPO_FORMAT_V2: Format = Format { family: 0x20, version: 2 };

/// At most this many ref updates in one `--atomic` push [guess at GitHub's own limit].
pub const ATOMIC_REFS_MAX: usize = 10_000;

/// The design's `PushOptions`: `git push -o` values, in order.
pub type PushOptions = Vec<Box<str>>;

/// SHARED's `Push`. `actor` is the authenticated caller; `credential()` comes from it. `request` is the edge's id for
/// the client request. `provenance`, `protected`, and `changed` come from the git cache's `validate` and are empty in
/// Milestone 1, which has no agent layer or rules, so they are carried as counts of zero in the encoding.
#[derive(Clone, Debug, PartialEq)]
pub struct Push {
    pub updates: Vec<RefUpdate>,
    pub atomicity: Atomicity,
    pub pack: Option<PackRef>,
    pub actor: Principal,
    pub options: PushOptions,
    pub request: RequestId,
    /// The client's `agent=` capability. Format 2 only.
    pub agent: Option<Box<str>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectReason {
    /// The ref's value is not the push's old value: git prints `fetch first`.
    Stale,
    /// Branch rules are Milestone 2; nothing produces this yet.
    NonFastForward,
    /// `refs/pull/` is the host's: GitHub answers `deny updating a hidden ref`.
    Hidden,
    TooManyRefs,
    /// An `AllOrNothing` push in which another ref failed. Not in the design's `RejectReason`, which has no reason for
    /// the refs that would have passed.
    Atomic,
    /// The name and an existing ref (or another ref of the same push) are a directory and a file of each other, such as
    /// `refs/heads/a` and `refs/heads/a/b`: git's refs cannot hold both, so the shard's repository could not either.
    /// Not in the design's `RejectReason`. Found by git's t5541 #20 (`tests/m1/results/m1-wp6.md`).
    NameConflict,
    /// Every ref of an `AllOrNothing` push in which a ref hit a `NameConflict`: upstream receive-pack finds those only
    /// when it commits the transaction, and then reports `atomic transaction failed` on every ref (t5541 #20).
    AtomicTransaction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefOutcome {
    Ok,
    Rejected(RejectReason),
}

/// One `RefOutcome` per update, in the push's order, with its ref, so `report_status` needs only the `Acked`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushOut {
    pub refs: Vec<(RefName, RefOutcome)>,
}

impl PushOut {
    pub fn outcomes(&self) -> Vec<RefOutcome> {
        self.refs.iter().map(|(_, o)| *o).collect()
    }
}

/// A refusal of the whole command. Nothing is applied and no `request` row is written, so a re-send is decided again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushErr {
    /// 401.
    Unauthorized(CredentialRefusal),
    /// 403 (404 for a private repository is the renderer's call).
    Forbidden,
    /// An atomic push over `ATOMIC_REFS_MAX`. The edge refuses these before `commit`.
    TooManyRefs { got: usize },
    /// The same ref twice in one push.
    Duplicate(RefName),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepoEvent {
    /// SHARED's `Refs { push, actor, updates, pack }`: one push's applied updates and the pack that carries their
    /// objects. `push` is its request id, and `actor` the account, which is all the fold reads of the `Actor`.
    Refs {
        push: RequestId,
        actor: Option<AccountId>,
        updates: Vec<RefUpdate>,
        pack: Option<PackRef>,
    },
    /// The refs one push's `decide` refused, each with its reason, so a re-send answered from the push's `request`
    /// row reports that reason (`Push::replay`) and not `Stale`. It changes no ref, and a consumer of `Refs` skips it.
    /// Not in SHARED's event list; found by git's t5541 #20 (`tests/m1/results/m1-wp8.md`).
    Refused {
        push: RequestId,
        refs: Vec<(RefName, RejectReason)>,
    },
    /// The design's `EntryBody::Promoted`: the objects of every inline pack in `from..=to` packed into one stored pack
    /// by the network's home shard, committed by the leader in an entry of its own with no request
    /// (`graftyard_raft::Member::record`). Ranges follow each other from index 1, and the pack is always stored.
    Promoted(Promoted),
    /// The design's `EntryBody::Archived`: the entries `from..=to` are in the archive object `key`, put create-only in
    /// two buckets, so members truncate their logs through `to` once they can HEAD it in two. It never runs past the
    /// last `Promoted`, and ranges follow each other from index 1.
    Archived(Archived),
    // Milestone 2 (`design/SHARED.md`, "Event types" and "Issues and pull requests"). Tags 4 onward, in this order;
    // each payload encodes by `graftyard_fold::canon`. `crate::tiers::tier_of` gives each its tiers.
    IssueOpened {
        n: IssueNumber,
        capture: IssueCapture,
    },
    PrOpened {
        n: IssueNumber,
        head: HeadSpec,
        base: RefName,
        capture: PrCapture,
    },
    PrClosed {
        n: IssueNumber,
        final_head: ObjectId,
    },
    IssueChanged {
        n: IssueNumber,
        change: IssueChange,
        actor: Option<AccountId>,
    },
    Comment(CommentEvent),
    Review(ReviewEvent),
    Labels(LabelEvent),
    Rule(BranchRule),
    Settings(RepoSettings),
    /// A required conclusion for `sha`. `conclusion: None` is a queued or in-progress run or a pending status, so a
    /// re-run blocks a merge again. For one (sha, name, source), the highest `version` wins.
    RequiredCheck {
        sha: ObjectId,
        name: Box<str>,
        source: CheckSource,
        version: u64,
        conclusion: Option<CheckConclusion>,
    },
    CheckRun(CheckRunUpdate),
    CheckSuite(CheckSuiteUpdate),
    Status(StatusUpdate),
}

impl RepoEvent {
    /// The ref updates the event applies: a `Refs`'s, and none for a `Refused`.
    pub fn updates(&self) -> &[RefUpdate] {
        match self {
            RepoEvent::Refs { updates, .. } => updates,
            _ => &[],
        }
    }
}

/// The repository chain's fold: its two tiers (`design/SHARED.md`, "Folds"; `crate::tiers`). The core tier's `ref`
/// table is the only copy of the refs: `Push` decides on it and a `fresh` read answers from it, so a push and a pull
/// request's head read one source. `repo` and `owner` are what every member knows without the log; the owner names the
/// `Authz` chain in `deps` until a `Settings` entry names another.
///
/// The tiers are stores the fold owns: `MemStore` in tests, and in a site member handles on its SQLite tables, which
/// clone as handles on the same tables (`graftyard_fold::TierStore`). Until the metadata tier is `loaded`, the fold
/// applies the core tier alone, and a core request never waits for the metadata tier.
#[derive(Clone, Debug)]
pub struct RepoFold {
    pub repo: Option<RepoId>,
    pub owner: Option<AccountId>,
    pub core: Box<dyn TierStore>,
    pub meta: Box<dyn TierStore>,
}

impl Default for RepoFold {
    fn default() -> RepoFold {
        RepoFold { repo: None, owner: None, core: Box::new(MemStore::default()), meta: Box::new(MemStore::default()) }
    }
}

/// Two folds are equal when their tiers hold the same rows: the same `fold_digest` per tier.
impl PartialEq for RepoFold {
    fn eq(&self, o: &RepoFold) -> bool {
        let meta = |f: &RepoFold| f.meta.loaded().then(|| fold_digest(&*f.meta, Tier::Metadata));
        (self.repo, self.owner) == (o.repo, o.owner)
            && fold_digest(&*self.core, Tier::Core) == fold_digest(&*o.core, Tier::Core)
            && meta(self) == meta(o)
    }
}

impl RepoFold {
    pub fn new(repo: RepoId, owner: AccountId) -> RepoFold {
        RepoFold { repo: Some(repo), owner: Some(owner), ..RepoFold::default() }
    }

    /// A fold over tiers a host holds.
    pub fn with_tiers(
        repo: Option<RepoId>,
        owner: Option<AccountId>,
        core: Box<dyn TierStore>,
        meta: Box<dyn TierStore>,
    ) -> RepoFold {
        RepoFold { repo, owner, core, meta }
    }

    /// The tiers' chain-specific half. A fold with no repository (the CLI's latency probe) uses repository 0.
    pub fn tiers(&self) -> RepoTiers {
        RepoTiers {
            repo: self.repo.unwrap_or(RepoId(graftyard_types::DbId(0))),
            owner: self.owner.unwrap_or(AccountId(graftyard_types::DbId(0))),
        }
    }

    pub fn get(&self, name: &RefName) -> Option<ObjectId> {
        get_rec(&*self.core, "ref", &[ref_key(name)])
    }

    /// Every ref, in name order.
    pub fn refs(&self) -> Vec<(RefName, ObjectId)> {
        self.core
            .prefix("ref", &[])
            .into_iter()
            .filter_map(|(k, v)| match (k.first(), v.first()) {
                (Some(Val::Blob(n)), Some(Val::Blob(o))) => {
                    Some((RefName::parse(n).ok()?, postcard::from_bytes(o).ok()?))
                }
                _ => None,
            })
            .collect()
    }

    pub fn promoted(&self) -> Option<Promoted> {
        get_rec(&*self.core, "promoted", &[])
    }

    pub fn archived(&self) -> Option<Archived> {
        get_rec(&*self.core, "archived", &[])
    }

    /// The owner whose `Authz` chain gates the repository: `RepoSettings.owner` once a `Settings` entry committed.
    pub fn authz_owner(&self) -> Option<AccountId> {
        get_rec::<RepoSettings>(&*self.core, "settings", &[]).map(|s| s.owner).or(self.owner)
    }

    /// The first index still in the chain's log (`ChainLog::first_index`).
    pub fn first_index(&self) -> Index {
        Index(self.archived().map_or(0, |a| a.to.0) + 1)
    }

    /// Applies one committed entry to the core tier, and to the metadata tier when it is loaded. A tier refuses an
    /// entry that does not follow its own `applied`; that is a host bug, so it panics as `GroupFold::apply` does.
    pub fn apply_core(&mut self, e: &LogEntry<RepoEvent>) {
        let t = self.tiers();
        graftyard_fold::apply(&t, &mut *self.core, Tier::Core, e).expect("core tier applies the next entry");
    }

    pub fn apply_meta(&mut self, e: &LogEntry<RepoEvent>) {
        let t = self.tiers();
        if self.meta.loaded() {
            graftyard_fold::apply(&t, &mut *self.meta, Tier::Metadata, e)
                .expect("metadata tier applies the next entry");
        }
    }
}

/// `verify`'s rule for a `Promoted` after `prev`: the range starts right after the previous one (at 1 for the first),
/// is not empty, and names a stored pack.
fn check_promoted(prev: Option<&Promoted>, p: &Promoted) -> Result<(), Box<str>> {
    let from = Index(prev.map_or(0, |q| q.to.0) + 1);
    if p.from != from || p.to < p.from {
        return Err(format!("promoted {:?}..={:?} where the next range starts at {from:?}", p.from, p.to).into());
    }
    if !matches!(p.pack, PackRef::Stored { .. }) {
        return Err("a promoted pack is always stored".into());
    }
    Ok(())
}

/// `verify`'s rule for an `Archived` after `prev` with `promoted` the last promotion: the range starts right after
/// the previous one, ends at or before the last promotion's `to`, and its key is the `Archive` key of this chain,
/// range, and digest. Stricter than the design, which lets a range that names no inline pack go without `Promoted`
/// [inference]: the fold cannot see the range's entries, so every archived range must be promoted first. An
/// `Archived` whose key is a `Snapshot` of this chain names a snapshot of one tier at `to`, from 0, with no ordering
/// against the log's archive ranges.
fn check_archived(
    repo: Option<RepoId>,
    prev: Option<&Archived>,
    promoted: Option<&Promoted>,
    a: &Archived,
) -> Result<(), Box<str>> {
    if let R2Key::Snapshot { chain, from, to, digest, .. } = &a.key {
        if repo.map(ChainId::Repo) != Some(*chain) || *from != Index(0) || *to != a.to || *digest != a.digest {
            return Err(format!("snapshot key {} does not name this chain at {:?} from 0", a.key, a.to).into());
        }
        return Ok(());
    }
    let from = Index(prev.map_or(0, |q| q.to.0) + 1);
    if a.from != from || a.to < a.from {
        return Err(format!("archived {:?}..={:?} where the next range starts at {from:?}", a.from, a.to).into());
    }
    if promoted.is_none_or(|p| a.to > p.to) {
        return Err(format!("archived through {:?} past the last promotion", a.to).into());
    }
    let key = repo.map(|r| R2Key::Archive { chain: ChainId::Repo(r), from: a.from, to: a.to, digest: a.digest });
    if key.as_ref() != Some(&a.key) {
        return Err(format!("archive key {} does not name this chain, range, and digest", a.key).into());
    }
    if a.to_at < a.from_at {
        return Err("archived range ends before it starts".into());
    }
    Ok(())
}

/// The fold at `applied` with an entry's earlier events written on top, by the same code `apply` runs: what `decide`
/// and `verify` read. Only the tiers the entry's tag names are written, as only those apply it; `check` refuses an
/// event whose tiers the tag leaves out. `check` reads only the core tier, so a follower whose metadata tier is still
/// loading verifies as any other; a command whose `decide` reads the metadata tier finds `view()` empty then.
pub struct RepoScratch<'a> {
    base: &'a RepoFold,
    tag: TierSet,
    at: Timestamp,
    core: Overlay<'a, dyn Read + 'a>,
    meta: Option<Overlay<'a, dyn Read + 'a>>,
}

impl RepoScratch<'_> {
    pub fn get(&self, name: &RefName) -> Option<ObjectId> {
        get_rec(&self.core, "ref", &[ref_key(name)])
    }

    pub fn fold(&self) -> &RepoFold {
        self.base
    }

    /// The owner whose `Authz` chain gates the repository, with the entry's earlier `Settings` on top.
    pub fn authz_owner(&self) -> Option<AccountId> {
        get_rec::<RepoSettings>(&self.core, "settings", &[]).map(|s| s.owner).or(self.base.owner)
    }

    /// Both tiers as `IssueCmd::decide` reads them. `None` while the metadata tier is not loaded.
    pub fn view(&self) -> Option<(&dyn Read, &dyn Read)> {
        Some((&self.core, self.meta.as_ref()?))
    }

    /// The refs, in the fold or written earlier in the entry, that `name` is a directory or a file of.
    pub fn conflicts(&self, name: &RefName) -> Vec<RefName> {
        let b = name.as_bytes();
        let mut out: Vec<RefName> = Vec::new();
        let mut consider = |n: &RefName| {
            if dir_or_file(name, n) && self.get(n).is_some() && !out.contains(n) {
                out.push(n.clone());
            }
        };
        // Its parents: each prefix that ends before a '/'.
        for (i, _) in b.iter().enumerate().filter(|(_, c)| **c == b'/') {
            if let Ok(p) = RefName::parse(&b[..i]) {
                consider(&p);
            }
        }
        // Its children: refs under "name/". Ref keys are blobs, which sort by bytes, so they are one run from "name/".
        let mut dir = b.to_vec();
        dir.push(b'/');
        let mut from = vec![Val::Blob(dir.clone().into())];
        // Few refs sit under a name, often none, so the scan reads a few rows at a time: one large read here costs
        // every push in an entry a copy of the rows the entry wrote after it (BF-20).
        loop {
            let batch = self.core.scan("ref", &from, 8);
            let full = batch.len() == 8;
            for (k, _) in &batch {
                let Some(Val::Blob(n)) = k.first() else { continue };
                if !n.starts_with(&dir) {
                    return out;
                }
                if let Ok(n) = RefName::parse(n) {
                    consider(&n);
                }
            }
            match batch.last() {
                Some((k, _)) if full => {
                    let Some(Val::Blob(n)) = k.first() else { return out };
                    // The next key after `n`: `n` followed by a zero byte.
                    let mut next = n.to_vec();
                    next.push(0);
                    from = vec![Val::Blob(next.into())];
                }
                _ => return out,
            }
        }
    }
}

/// Whether one name is a proper path prefix of the other at a '/': `refs/heads/a` and `refs/heads/a/b`.
pub fn dir_or_file(a: &RefName, b: &RefName) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (short, long) = if a.len() < b.len() { (a, b) } else { (b, a) };
    long.len() > short.len() && long.starts_with(short) && long[short.len()] == b'/'
}

impl Scratch for RepoScratch<'_> {
    type Event = RepoEvent;

    /// Writes the event into the tiers the entry's tag names, as `apply` will.
    fn apply(&mut self, ev: &RepoEvent) {
        let (t, set) = (self.base.tiers(), tier_of(ev));
        if has(set, Tier::Core) && has(self.tag, Tier::Core) {
            t.write(ev, Tier::Core, self.at, &mut self.core);
        }
        if let Some(m) = self.meta.as_mut()
            && has(set, Tier::Metadata)
            && has(self.tag, Tier::Metadata)
        {
            t.write(ev, Tier::Metadata, self.at, m);
        }
    }

    /// `verify`'s replay: the entry's tag covers the event's tiers, each update's `expect` holds against the running
    /// state, and `Promoted` and `Archived` follow the last committed ranges. An entry whose order or base differs
    /// from the follower's fold is refused. Only the core tier is read, so a member whose metadata tier is not loaded
    /// verifies the same.
    fn check(&self, ev: &RepoEvent) -> Result<(), Box<str>> {
        let needs = tier_of(ev);
        if needs.0 & !self.tag.0 != 0 {
            return Err(format!("the entry is tagged {:?} but the event needs {needs:?}", self.tag).into());
        }
        let promoted: Option<Promoted> = get_rec(&self.core, "promoted", &[]);
        let updates = match ev {
            RepoEvent::Refs { updates, .. } => updates,
            RepoEvent::Promoted(p) => return check_promoted(promoted.as_ref(), p),
            RepoEvent::Archived(a) => {
                let prev: Option<Archived> = get_rec(&self.core, "archived", &[]);
                return check_archived(self.base.repo, prev.as_ref(), promoted.as_ref(), a);
            }
            _ => return Ok(()),
        };
        let mut seen: BTreeMap<&RefName, Option<ObjectId>> = BTreeMap::new();
        for u in updates {
            let have = seen.get(&u.name).copied().unwrap_or_else(|| self.get(&u.name));
            if have != u.expect {
                return Err(format!("{:?} is {:?}, the event expects {:?}", u.name, have, u.expect).into());
            }
            seen.insert(&u.name, u.new);
        }
        Ok(())
    }
}

impl Fold for RepoFold {
    type Event = RepoEvent;
    type Scratch<'a> = RepoScratch<'a>;

    fn scratch(&self) -> RepoScratch<'_> {
        self.scratch_at(Timestamp(0), TierSet(u8::MAX))
    }

    fn scratch_at(&self, at: Timestamp, tiers: TierSet) -> RepoScratch<'_> {
        RepoScratch {
            base: self,
            tag: tiers,
            at,
            core: Overlay::new(&self.core as &dyn Read),
            meta: self.meta.loaded().then(|| Overlay::new(&self.meta as &dyn Read)),
        }
    }

    /// Never called: a tiered fold applies whole entries (`apply_entry`), which carry the index, `prev`, time, and tag
    /// a tier checks and stamps.
    fn apply(&mut self, _ev: &RepoEvent) {
        unreachable!("RepoFold applies whole entries")
    }

    fn apply_entry(&mut self, e: &LogEntry<RepoEvent>) {
        self.apply_core(e);
        self.apply_meta(e);
    }

    fn tiers(ev: &RepoEvent) -> TierSet {
        tier_of(ev)
    }

    /// A `request` row keeps a push's `Refs` without its inline pack's bytes: `Push::replay` reads only the refs,
    /// and the sweeper's `RequestSource` only stored packs (`packs_of`), while the bytes stay in the log entry. Two
    /// days of 1 MB inline packs in every member's memory ran its isolate out of memory (F45).
    fn row_event(ev: &RepoEvent) -> RepoEvent {
        match ev {
            RepoEvent::Refs { push, actor, updates, pack: Some(PackRef::Inline(_)) } => {
                RepoEvent::Refs { push: *push, actor: *actor, updates: updates.clone(), pack: None }
            }
            other => other.clone(),
        }
    }
}

/// The ref namespaces a push may not write.
pub fn hidden(name: &RefName) -> bool {
    name.starts_with("refs/pull/")
}

fn may_write(actor: &Principal, owner: Option<AccountId>, repo: Option<RepoId>, authz: Option<&dyn AuthzRead>) -> bool {
    let Actor::User(user) = actor.actor else { return false };
    let Some(authz) = authz else { return false };
    if owner.is_none_or(|o| authz.owner() != Some(o)) {
        return false;
    }
    authz.is_admin(user) || repo.and_then(|r| authz.grant(r, user)).is_some_and(|p| p >= RepoPermission::Write)
}

impl Push {
    fn decide_refs(&self, s: &RepoScratch<'_>) -> Result<(SmallVec<[RepoEvent; 4]>, PushOut), PushErr> {
        if self.atomicity == Atomicity::AllOrNothing && self.updates.len() > ATOMIC_REFS_MAX {
            return Err(PushErr::TooManyRefs { got: self.updates.len() });
        }
        let mut names = std::collections::BTreeSet::new();
        for u in &self.updates {
            if !names.insert(&u.name) {
                return Err(PushErr::Duplicate(u.name.clone()));
            }
        }
        let mut refs: Vec<RefOutcome> = self
            .updates
            .iter()
            .map(|u| {
                if hidden(&u.name) {
                    RefOutcome::Rejected(RejectReason::Hidden)
                } else if s.get(&u.name) != u.expect {
                    RefOutcome::Rejected(RejectReason::Stale)
                } else {
                    RefOutcome::Ok
                }
            })
            .collect();
        // A create whose name is a directory or a file of a ref that stays: one of this push's other passing updates,
        // or a fold ref this push does not delete.
        let after = |n: &RefName, refs: &[RefOutcome]| -> bool {
            match self.updates.iter().zip(refs).find(|(u, r)| &u.name == n && **r == RefOutcome::Ok) {
                Some((u, _)) => u.new.is_some(),
                None => s.get(n).is_some(),
            }
        };
        // Against the outcomes before this pass, so two creates that conflict with each other are both refused.
        let checked = refs.clone();
        for (i, u) in self.updates.iter().enumerate() {
            if checked[i] != RefOutcome::Ok || u.new.is_none() || s.get(&u.name).is_some() {
                continue;
            }
            let clash = s.conflicts(&u.name).iter().any(|n| after(n, &checked))
                || self.updates.iter().enumerate().any(|(j, v)| {
                    j != i && checked[j] == RefOutcome::Ok && v.new.is_some() && dir_or_file(&u.name, &v.name)
                });
            if clash {
                refs[i] = RefOutcome::Rejected(RejectReason::NameConflict);
            }
        }
        let failed = refs.iter().any(|r| *r != RefOutcome::Ok);
        let conflict = refs.contains(&RefOutcome::Rejected(RejectReason::NameConflict));
        let refs = if conflict && self.atomicity == Atomicity::AllOrNothing {
            vec![RefOutcome::Rejected(RejectReason::AtomicTransaction); refs.len()]
        } else if failed && self.atomicity == Atomicity::AllOrNothing {
            refs.into_iter()
                .map(|r| if r == RefOutcome::Ok { RefOutcome::Rejected(RejectReason::Atomic) } else { r })
                .collect()
        } else {
            refs
        };
        let applied: Vec<RefUpdate> =
            self.updates.iter().zip(&refs).filter(|(_, r)| **r == RefOutcome::Ok).map(|(u, _)| u.clone()).collect();
        let refs = PushOut { refs: self.updates.iter().map(|u| u.name.clone()).zip(refs).collect() };
        let refused: Vec<(RefName, RejectReason)> = refs
            .refs
            .iter()
            .filter_map(|(n, o)| match o {
                RefOutcome::Rejected(r) => Some((n.clone(), *r)),
                RefOutcome::Ok => None,
            })
            .collect();
        let mut events = SmallVec::new();
        if !applied.is_empty() {
            let account = match self.actor.actor {
                Actor::User(u) => Some(u.0),
                Actor::Anonymous => None,
            };
            // A pack with zero objects names nothing, and a push of deletes alone has none.
            let pack = if applied.iter().all(|u| u.new.is_none()) { None } else { self.pack.clone() };
            events.push(RepoEvent::Refs { push: self.request, actor: account, updates: applied, pack });
        }
        if !refused.is_empty() {
            events.push(RepoEvent::Refused { push: self.request, refs: refused });
        }
        Ok((events, refs))
    }

    /// The command's canonical encoding with no pack: what `COMMAND_MAX` measures for "the ref updates alone".
    pub fn encoded_len_without_pack(&self) -> usize {
        let mut v = Vec::new();
        self.encode_with(&mut v, None);
        v.len()
    }

    fn encode_with(&self, out: &mut Vec<u8>, pack: Option<&PackRef>) {
        let mut e = Enc(out);
        e.u8(match self.atomicity {
            Atomicity::AllOrNothing => 0,
            Atomicity::PerRef => 1,
        });
        e.count(self.updates.len());
        for u in &self.updates {
            e.bytes(u.name.as_bytes());
            e.option(u.expect.as_ref(), |e, o| e.0.extend_from_slice(&o.0));
            e.option(u.new.as_ref(), |e, o| e.0.extend_from_slice(&o.0));
        }
        e.option(pack, encode_pack);
        encode_principal(&mut e, &self.actor);
        e.count(self.options.len());
        for o in &self.options {
            e.str(o);
        }
        e.u128(self.request.0.0);
        // provenance, protected, changed
        e.count(0);
        e.count(0);
        e.count(0);
        if self.format() == REPO_FORMAT_V2 {
            e.option(self.agent.as_ref(), |e, a| e.str(a));
        }
    }

    /// The bytes a member decodes: the format, then the canonical encoding.
    pub fn to_wire(&self) -> Vec<u8> {
        let f = self.format();
        let mut out = Vec::with_capacity(64 + self.updates.len() * 64);
        out.extend_from_slice(&f.family.to_be_bytes());
        out.extend_from_slice(&f.version.to_be_bytes());
        self.encode(&mut out);
        out
    }

    /// Decodes `to_wire` bytes in any format this release reads. `Err(Some(format))` names a format it does not.
    pub fn from_wire(b: &[u8]) -> Result<Push, WireError> {
        let mut d = Dec(b);
        let format = Format { family: d.u16()?, version: d.u16()? };
        if format != REPO_FORMAT_V1 && format != REPO_FORMAT_V2 {
            return Err(WireError::Format(format));
        }
        let atomicity = match d.u8()? {
            0 => Atomicity::AllOrNothing,
            1 => Atomicity::PerRef,
            _ => return Err(WireError::Malformed("atomicity")),
        };
        let n = d.u32()? as usize;
        let mut updates = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            let name = RefName::parse(d.bytes()?).map_err(|_| WireError::Malformed("ref name"))?;
            let expect = d.option(|d| d.oid())?;
            let new = d.option(|d| d.oid())?;
            updates.push(RefUpdate { name, expect, new });
        }
        let pack = d.option(decode_pack)?;
        let actor = decode_principal(&mut d)?;
        let n = d.u32()? as usize;
        let mut options = Vec::with_capacity(n.min(1 << 10));
        for _ in 0..n {
            options.push(d.str()?.into());
        }
        let request = RequestId(Ulid(d.u128()?));
        for _ in 0..3 {
            if d.u32()? != 0 {
                return Err(WireError::Malformed("validate output is empty in Milestone 1"));
            }
        }
        let agent = if format == REPO_FORMAT_V2 { d.option(|d| d.str().map(Into::into))? } else { None };
        if !d.0.is_empty() {
            return Err(WireError::Malformed("trailing bytes"));
        }
        Ok(Push { updates, atomicity, pack, actor, options, request, agent })
    }
}

fn encode_pack(e: &mut Enc, p: &PackRef) {
    match p {
        PackRef::Inline(b) => {
            e.u8(0);
            e.bytes(b);
        }
        PackRef::Stored { id, checksum, bytes, sites } => {
            e.u8(1);
            e.u128(id.0.0);
            e.0.extend_from_slice(&checksum.0);
            e.u64(*bytes);
            e.u64(sites.0);
        }
    }
}

fn encode_principal(e: &mut Enc, p: &Principal) {
    match p.actor {
        Actor::User(u) => {
            e.u8(0);
            e.u64(u.0.0.0 as u64);
        }
        Actor::Anonymous => e.u8(1),
    }
    e.u64(p.scopes.0);
    match p.credential {
        Credential::Stored(h) => {
            e.u8(0);
            e.0.extend_from_slice(&h);
        }
    }
    e.count(p.sso_orgs.len());
    for o in &p.sso_orgs {
        e.u64(o.0.0.0 as u64);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    #[error("format {0:?} is not one this release reads")]
    Format(Format),
    #[error("malformed push record: {0}")]
    Malformed(&'static str),
}

struct Dec<'a>(&'a [u8]);

impl<'a> Dec<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        if self.0.len() < n {
            return Err(WireError::Malformed("truncated"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, WireError> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().expect("2")))
    }
    fn u32(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().expect("4")))
    }
    fn u64(&mut self) -> Result<u64, WireError> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().expect("8")))
    }
    fn u128(&mut self) -> Result<u128, WireError> {
        Ok(u128::from_be_bytes(self.take(16)?.try_into().expect("16")))
    }
    fn bytes(&mut self) -> Result<&'a [u8], WireError> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn str(&mut self) -> Result<&'a str, WireError> {
        std::str::from_utf8(self.bytes()?).map_err(|_| WireError::Malformed("utf-8"))
    }
    fn oid(&mut self) -> Result<ObjectId, WireError> {
        Ok(ObjectId(self.take(20)?.try_into().expect("20")))
    }
    fn option<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, WireError>) -> Result<Option<T>, WireError> {
        match self.u8()? {
            0 => Ok(None),
            1 => f(self).map(Some),
            _ => Err(WireError::Malformed("option tag")),
        }
    }
}

fn decode_pack(d: &mut Dec) -> Result<PackRef, WireError> {
    match d.u8()? {
        0 => Ok(PackRef::Inline(d.bytes()?.to_vec())),
        1 => Ok(PackRef::Stored {
            id: PackId(Ulid(d.u128()?)),
            checksum: Digest(d.take(32)?.try_into().expect("32")),
            bytes: d.u64()?,
            sites: SiteSet(d.u64()?),
        }),
        _ => Err(WireError::Malformed("pack tag")),
    }
}

fn decode_principal(d: &mut Dec) -> Result<Principal, WireError> {
    let actor = match d.u8()? {
        0 => Actor::User(UserId(AccountId(graftyard_types::DbId(d.u64()? as i64)))),
        1 => Actor::Anonymous,
        _ => return Err(WireError::Malformed("actor tag")),
    };
    let scopes = ScopeSet(d.u64()?);
    let credential = match d.u8()? {
        0 => Credential::Stored(d.take(32)?.try_into().expect("32")),
        _ => return Err(WireError::Malformed("credential tag")),
    };
    let n = d.u32()? as usize;
    let mut sso_orgs = SmallVec::new();
    for _ in 0..n.min(64) {
        sso_orgs.push(graftyard_identity::OrgId(AccountId(graftyard_types::DbId(d.u64()? as i64))));
    }
    Ok(Principal { actor, scopes, credential, sso_orgs })
}

/// A push's serde form is the base64 of `to_wire`, so a member's JSON `cmd` carries the format explicitly and a field
/// a release does not know is never dropped silently. Deserializing a format this release does not read fails with
/// `WireError::Format`'s message; a host answers `Unsupported` for it (`group::decode_commit`).
impl Serialize for Push {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&crate::pack::b64::encode(&self.to_wire()))
    }
}

impl<'de> Deserialize<'de> for Push {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Push, D::Error> {
        let s = String::deserialize(d)?;
        let b = crate::pack::b64::decode(&s).ok_or_else(|| serde::de::Error::custom("push is not base64"))?;
        Push::from_wire(&b).map_err(serde::de::Error::custom)
    }
}

impl Record for Push {
    fn encode(&self, out: &mut Vec<u8>) {
        self.encode_with(out, self.pack.as_ref());
    }
}

impl Record for RepoEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        let (push, actor, updates, pack) = match self {
            RepoEvent::Refs { push, actor, updates, pack } => (push, actor, updates, pack),
            RepoEvent::Promoted(p) => {
                e.u8(2);
                e.u64(p.from.0);
                e.u64(p.to.0);
                encode_pack(&mut e, &p.pack);
                return;
            }
            RepoEvent::Archived(a) => {
                e.u8(3);
                e.str(&a.key.to_string());
                e.u64(a.from.0);
                e.u64(a.to.0);
                e.u64(a.from_at.0);
                e.u64(a.to_at.0);
                e.0.extend_from_slice(&a.digest.0);
                return;
            }
            RepoEvent::Refused { push, refs } => {
                e.u8(1);
                e.u128(push.0.0);
                e.count(refs.len());
                for (n, r) in refs {
                    e.bytes(n.as_bytes());
                    e.u8(reason_tag(*r));
                }
                return;
            }
            RepoEvent::IssueOpened { n, capture } => return tagged(out, 4, &(n, capture)),
            RepoEvent::PrOpened { n, head, base, capture } => return tagged(out, 5, &(n, head, base, capture)),
            RepoEvent::PrClosed { n, final_head } => return tagged(out, 6, &(n, final_head)),
            RepoEvent::IssueChanged { n, change, actor } => return tagged(out, 7, &(n, change, actor)),
            RepoEvent::Comment(c) => return tagged(out, 8, c),
            RepoEvent::Review(r) => return tagged(out, 9, r),
            RepoEvent::Labels(l) => return tagged(out, 10, l),
            RepoEvent::Rule(r) => return tagged(out, 11, r),
            RepoEvent::Settings(s) => return tagged(out, 12, s),
            RepoEvent::RequiredCheck { sha, name, source, version, conclusion } => {
                return tagged(out, 13, &(sha, name, source, version, conclusion));
            }
            RepoEvent::CheckRun(c) => return tagged(out, 14, c),
            RepoEvent::CheckSuite(c) => return tagged(out, 15, c),
            RepoEvent::Status(st) => return tagged(out, 16, st),
        };
        e.u8(0);
        e.u128(push.0.0);
        e.option(actor.as_ref(), |e, a| e.u64(a.0.0 as u64));
        e.count(updates.len());
        for u in updates {
            e.bytes(u.name.as_bytes());
            e.option(u.expect.as_ref(), |e, o| e.0.extend_from_slice(&o.0));
            e.option(u.new.as_ref(), |e, o| e.0.extend_from_slice(&o.0));
        }
        e.option(pack.as_ref(), encode_pack);
    }
}

/// A Milestone 2 event's bytes: its fixed tag, then its payload by the canonical rule.
fn tagged<T: Serialize + ?Sized>(out: &mut Vec<u8>, tag: u8, payload: &T) {
    out.push(tag);
    graftyard_fold::canon::write(out, payload);
}

impl Command for Push {
    type Fold = RepoFold;
    type Dep = IdentityDep;
    type Out = PushOut;
    type Err = PushErr;

    fn format(&self) -> Format {
        if self.agent.is_some() { REPO_FORMAT_V2 } else { REPO_FORMAT_V1 }
    }
    fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
    /// The actor's `Account` and token rows, and the owner's `Authz`, named by the fold's `RepoSettings`.
    fn deps(&self, fold: &RepoFold) -> SmallVec<[ChainId; 3]> {
        let mut d = credential_chains(&self.actor.credential_shard(), None);
        if let Some(o) = fold.authz_owner() {
            d.push(ChainId::Authz(o));
        }
        d
    }
    fn decide(
        &self,
        s: &RepoScratch<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[RepoEvent; 4]>, PushOut), PushErr> {
        let cred: CredentialShard = self.actor.credential_shard();
        // A push names its account and its token, so the leader re-reads both.
        if cred.account.is_none() || cred.key.is_none() {
            return Err(PushErr::Unauthorized(CredentialRefusal::NoToken));
        }
        check_credential(&cred, Own::Other, deps).map_err(PushErr::Unauthorized)?;
        let owner = s.authz_owner();
        let authz = owner.and_then(|o| {
            deps.iter().find_map(|(c, d)| match d {
                IdentityDep::Authz(f) if *c == ChainId::Authz(o) => Some(f as &dyn AuthzRead),
                _ => None,
            })
        });
        if !may_write(&self.actor, owner, s.fold().repo, authz) {
            return Err(PushErr::Forbidden);
        }
        self.decide_refs(s)
    }
    /// Each ref as the first decide answered it: `Ok` if a stored `Refs` applied it, else the reason a stored
    /// `Refused` gives. A ref in neither (a row with no `Refused`) is reported `Stale`.
    fn replay(&self, events: &[RepoEvent]) -> PushOut {
        let mut was: BTreeMap<&RefName, RefOutcome> = BTreeMap::new();
        for ev in events {
            match ev {
                RepoEvent::Refs { updates, .. } => was.extend(updates.iter().map(|u| (&u.name, RefOutcome::Ok))),
                RepoEvent::Refused { refs, .. } => was.extend(refs.iter().map(|(n, r)| (n, RefOutcome::Rejected(*r)))),
                _ => {}
            }
        }
        PushOut {
            refs: self
                .updates
                .iter()
                .map(|u| {
                    (u.name.clone(), was.get(&u.name).copied().unwrap_or(RefOutcome::Rejected(RejectReason::Stale)))
                })
                .collect(),
        }
    }
}

/// A reason's byte in `RepoEvent::Refused`'s canonical encoding, fixed per reason so that reordering the enum cannot
/// change a digest.
fn reason_tag(r: RejectReason) -> u8 {
    match r {
        RejectReason::Stale => 0,
        RejectReason::NonFastForward => 1,
        RejectReason::Hidden => 2,
        RejectReason::TooManyRefs => 3,
        RejectReason::Atomic => 4,
        RejectReason::NameConflict => 5,
        RejectReason::AtomicTransaction => 6,
    }
}

/// Every pack the events name, for the shard's `IngestCursor` catch-up.
pub fn packs_of(events: &[Ev<Push>]) -> Vec<PackRef> {
    events
        .iter()
        .filter_map(|ev| match ev {
            RepoEvent::Refs { pack, .. } => pack.clone(),
            _ => None,
        })
        .collect()
}
