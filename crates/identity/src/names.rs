//! `Names` groups (`design/DESIGN.md`, Shape, "Identity" and "Sites and placement"; `design/SHARED.md`, "Names").
//!
//! 1,024 shards on `Tri`. A login claim is unique on `name_shard(Login(key))`, an account's `Account` row and owner
//! `Placement` record are on `account_shard(id)`, and a repository's `RepoOwner` row is on `name_shard(Repo(id))`.

use std::collections::BTreeMap;
use std::fmt;

use graftyard_raft::{Command, Enc, Ev, Fold, Format, Record, Scratch, TierSet};
use graftyard_types::{
    AccountId, ChainId, CredentialShard, NameShard, Placement, PlacementError, PlacementName, PlacementRecord, RepoId,
    SiteId, Timestamp,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use smallvec::{SmallVec, smallvec};

use crate::NAMES_FORMAT;
use crate::authz::AccountKind;
use crate::credential::{CredentialRefusal, IdentityDep, Own, check_credential, credential_chains, encode_cred};
use crate::enc;
use crate::ids::{IdBlock, IdCursor};
use crate::overlay::{Overlay, Table};

pub const NAME_SHARDS: u16 = 1024;

/// Every first path segment the host serves, so no login can shadow a route. SHARED's list ends in `/* .. */`: the
/// rest is to be generated from the router, which does not exist yet.
pub const RESERVED: &[&str] = &[
    "api",
    "login",
    "logout",
    "session",
    "settings",
    "orgs",
    "organizations",
    "users",
    "repositories",
    "notifications",
    "search",
    "status",
    "site",
    "raw",
    "gist",
    "codeload",
    "uploads",
    "_services",
    "_t",
    "_codeload",
    "_registry",
];

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    #[error("a login is 1 to 39 ASCII letters, digits, or single hyphens, not starting or ending with a hyphen")]
    Login,
    #[error("a repository name is 1 to 100 of [A-Za-z0-9._-], not \".\" or \"..\"")]
    Repo,
}

/// 1..=39 ASCII letters, digits, or single hyphens, not starting or ending with a hyphen. Keeps the spelling the
/// owner chose; its `key` is the only form compared. Its serde form is the text, parsed again on decode.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Login(Box<str>);

/// ASCII lowercase of a `Login`. Logins are ASCII, so lowercasing is exact.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LoginKey(Box<str>);

impl Login {
    pub fn parse(s: &str) -> Result<Login, NameError> {
        let b = s.as_bytes();
        let ok = (1..=39).contains(&b.len())
            && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
            && b[0] != b'-'
            && b[b.len() - 1] != b'-'
            && !s.contains("--");
        if ok { Ok(Login(s.into())) } else { Err(NameError::Login) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn key(&self) -> LoginKey {
        LoginKey(self.0.to_ascii_lowercase().into())
    }
}

impl LoginKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the key is a first path segment the host serves.
    pub fn is_reserved(&self) -> bool {
        RESERVED.contains(&&*self.0)
    }
}

/// 1..=100 of [A-Za-z0-9._-], not "." or "..", with one trailing ".git" stripped first.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepoName(Box<str>);

/// The compared form of a `RepoName`: ASCII lowercase, as for logins [inference: SHARED does not say how the key is
/// formed; GitHub repository names are case-insensitive].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepoNameKey(Box<str>);

impl RepoName {
    pub fn parse(s: &str) -> Result<RepoName, NameError> {
        let s = s.strip_suffix(".git").unwrap_or(s);
        let ok = (1..=100).contains(&s.len())
            && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
            && s != "."
            && s != "..";
        if ok { Ok(RepoName(s.into())) } else { Err(NameError::Repo) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn key(&self) -> RepoNameKey {
        RepoNameKey(self.0.to_ascii_lowercase().into())
    }
}

impl RepoNameKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

macro_rules! debug_as_str {
    ($($t:ident),*) => {$(
        impl fmt::Debug for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({:?})", stringify!($t), &*self.0)
            }
        }
    )*};
}
debug_as_str!(Login, LoginKey, RepoName, RepoNameKey);

/// The serde forms: the text, which decoding parses again, so a stored or sent name keeps its invariants.
macro_rules! text_serde {
    ($($t:ident => $parse:expr),*) => {$(
        impl From<$t> for String {
            fn from(v: $t) -> String {
                v.0.into()
            }
        }
        impl TryFrom<String> for $t {
            type Error = NameError;
            fn try_from(s: String) -> Result<$t, NameError> {
                let parse: fn(&str) -> Result<$t, NameError> = $parse;
                parse(&s)
            }
        }
    )*};
}
text_serde!(
    Login => Login::parse,
    RepoName => |s| RepoName::parse(s).and_then(|r| if r.as_str() == s { Ok(r) } else { Err(NameError::Repo) }),
    LoginKey => |s| Login::parse(s).map(|l| l.key()).and_then(|k| if k.as_str() == s { Ok(k) } else { Err(NameError::Login) }),
    RepoNameKey => |s| RepoName::parse(s).map(|r| r.key()).and_then(|k| if k.as_str() == s { Ok(k) } else { Err(NameError::Repo) })
);

/// The design's `NameKey`. Milestone 1 has logins and repositories; `Email`, `Shortcode`, `App`, `AppSlug`, `Client`,
/// and `Installation` come with Milestone 3.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NameKey {
    Login(LoginKey),
    Repo(RepoId),
}

fn shard_of(tag: &[u8], body: &[u8]) -> NameShard {
    let h = Sha256::new().chain_update(tag).chain_update(body).finalize();
    // Four bytes, so the modulo bias is about 2^-22.
    NameShard((u32::from_be_bytes([h[0], h[1], h[2], h[3]]) % u32::from(NAME_SHARDS)) as u16)
}

pub fn name_shard(key: &NameKey) -> NameShard {
    match key {
        NameKey::Login(k) => shard_of(b"graftyard/names/login/", k.0.as_bytes()),
        NameKey::Repo(r) => shard_of(b"graftyard/names/repo/", &r.0.0.to_be_bytes()),
    }
}

/// The shard of an account's `Account` row and its owner `Placement` record.
pub fn account_shard(id: AccountId) -> NameShard {
    shard_of(b"graftyard/names/account/", &id.0.0.to_be_bytes())
}

/// What a login key resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoginRow {
    Claimed {
        account: AccountId,
        kind: AccountKind,
    },
    /// An owner renamed away: the old login still points at the account until someone claims it.
    Former {
        account: AccountId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRow {
    pub suspended: bool,
    pub sessions_not_before: Timestamp,
}

/// The `NamesEvent` variants Milestone 1 needs. Id blocks arrive as `EntryBody::IdBlock` entries
/// (`Fold::apply_id_block`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NamesEvent {
    LoginClaimed {
        key: LoginKey,
        account: AccountId,
        kind: AccountKind,
    },
    Account {
        id: AccountId,
        suspended: bool,
        sessions_not_before: Timestamp,
    },
    FormerName {
        key: LoginKey,
        account: AccountId,
    },
    /// On `account_shard(owner)`.
    Placement {
        owner: AccountId,
        record: PlacementRecord,
    },
    /// On `name_shard(NameKey::Repo(repo))`.
    RepoOwner {
        repo: RepoId,
        owner: AccountId,
    },
}

impl Record for NamesEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        match self {
            NamesEvent::LoginClaimed { key, account, kind } => {
                e.u8(0);
                e.str(&key.0);
                enc::account(&mut e, *account);
                e.u8(*kind as u8);
            }
            NamesEvent::Account { id, suspended, sessions_not_before } => {
                e.u8(1);
                enc::account(&mut e, *id);
                e.u8(u8::from(*suspended));
                enc::at(&mut e, *sessions_not_before);
            }
            NamesEvent::FormerName { key, account } => {
                e.u8(2);
                e.str(&key.0);
                enc::account(&mut e, *account);
            }
            // Tags follow the design's `NamesEvent` declaration order, so the variants Milestone 3 adds keep theirs.
            NamesEvent::Placement { owner, record } => {
                e.u8(10);
                enc::account(&mut e, *owner);
                enc::placement_record(&mut e, record);
            }
            NamesEvent::RepoOwner { repo, owner } => {
                e.u8(11);
                enc::repo(&mut e, *repo);
                enc::account(&mut e, *owner);
            }
        }
    }
}

/// One `Names` shard's fold. `shard` is the chain's own shard: a command whose key belongs to another shard is
/// refused, because the shard is the key's uniqueness point. `None`, the `Default` a member starts from before its
/// host sets the shard, checks no key placement.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamesFold {
    pub shard: Option<NameShard>,
    pub logins: BTreeMap<LoginKey, LoginRow>,
    pub accounts: BTreeMap<AccountId, AccountRow>,
    pub placements: BTreeMap<AccountId, PlacementRecord>,
    pub repo_owners: BTreeMap<RepoId, AccountId>,
    pub ids: IdCursor,
}

impl NamesFold {
    pub fn new(shard: NameShard) -> NamesFold {
        NamesFold { shard: Some(shard), ..NamesFold::default() }
    }
}

/// Every read a `Names` shard serves, over the fold or a scratch view.
pub trait NamesRead {
    fn login(&self, key: &LoginKey) -> Option<LoginRow>;
    fn account(&self, id: AccountId) -> Option<AccountRow>;
    fn placement(&self, owner: AccountId) -> Option<&PlacementRecord>;
    fn repo_owner(&self, repo: RepoId) -> Option<AccountId>;
}

impl NamesRead for NamesFold {
    fn login(&self, key: &LoginKey) -> Option<LoginRow> {
        self.logins.get(key).copied()
    }
    fn account(&self, id: AccountId) -> Option<AccountRow> {
        self.accounts.get(&id).copied()
    }
    fn placement(&self, owner: AccountId) -> Option<&PlacementRecord> {
        self.placements.get(&owner)
    }
    fn repo_owner(&self, repo: RepoId) -> Option<AccountId> {
        self.repo_owners.get(&repo).copied()
    }
}

struct Tables<'t, L, A, P, R> {
    logins: &'t mut L,
    accounts: &'t mut A,
    placements: &'t mut P,
    repo_owners: &'t mut R,
    ids: &'t mut IdCursor,
}

/// The writes of one event, shared by `NamesFold::apply` and `NamesScratch::apply` so the two never diverge.
fn write<L, A, P, R>(ev: &NamesEvent, t: Tables<'_, L, A, P, R>)
where
    L: Table<LoginKey, LoginRow>,
    A: Table<AccountId, AccountRow>,
    P: Table<AccountId, PlacementRecord>,
    R: Table<RepoId, AccountId>,
{
    match ev {
        NamesEvent::LoginClaimed { key, account, kind } => {
            t.logins.put(key.clone(), LoginRow::Claimed { account: *account, kind: *kind });
            t.ids.apply_assigned(account.0);
        }
        NamesEvent::Account { id, suspended, sessions_not_before } => {
            t.accounts.put(*id, AccountRow { suspended: *suspended, sessions_not_before: *sessions_not_before });
        }
        NamesEvent::FormerName { key, account } => t.logins.put(key.clone(), LoginRow::Former { account: *account }),
        NamesEvent::Placement { owner, record } => t.placements.put(*owner, record.clone()),
        NamesEvent::RepoOwner { repo, owner } => t.repo_owners.put(*repo, *owner),
    }
}

pub struct NamesScratch<'a> {
    fold: &'a NamesFold,
    logins: Overlay<'a, LoginKey, LoginRow>,
    accounts: Overlay<'a, AccountId, AccountRow>,
    placements: Overlay<'a, AccountId, PlacementRecord>,
    repo_owners: Overlay<'a, RepoId, AccountId>,
    ids: IdCursor,
}

impl NamesScratch<'_> {
    pub fn shard(&self) -> Option<NameShard> {
        self.fold.shard
    }
    pub fn ids(&self) -> &IdCursor {
        &self.ids
    }
}

impl NamesRead for NamesScratch<'_> {
    fn login(&self, key: &LoginKey) -> Option<LoginRow> {
        self.logins.get(key).copied()
    }
    fn account(&self, id: AccountId) -> Option<AccountRow> {
        self.accounts.get(&id).copied()
    }
    fn placement(&self, owner: AccountId) -> Option<&PlacementRecord> {
        self.placements.get(&owner)
    }
    fn repo_owner(&self, repo: RepoId) -> Option<AccountId> {
        self.repo_owners.get(&repo).copied()
    }
}

/// Whether `next` may replace `prev` as an owner's record: a first record has no `moving_from`; a move bumps
/// `generation` by one and keeps the record it replaces, `moving_from` included, in `moving_from`; finishing a move
/// clears `moving_from` and keeps everything else.
pub(crate) fn follows(prev: Option<&PlacementRecord>, next: &PlacementRecord) -> bool {
    match (prev, next.moving_from.as_deref()) {
        (None, None) => true,
        (Some(p), Some(from)) => from == p && next.generation == p.generation + 1,
        (Some(p), None) => p.moving_from.is_some() && PlacementRecord { moving_from: None, ..p.clone() } == *next,
        (None, Some(_)) => false,
    }
}

impl Scratch for NamesScratch<'_> {
    type Event = NamesEvent;
    fn apply(&mut self, ev: &NamesEvent) {
        write(
            ev,
            Tables {
                logins: &mut self.logins,
                accounts: &mut self.accounts,
                placements: &mut self.placements,
                repo_owners: &mut self.repo_owners,
                ids: &mut self.ids,
            },
        );
    }
    fn apply_id_block(&mut self, b: IdBlock) {
        self.ids.apply_block(b);
    }
    /// What a follower checks before it stores an entry: no claim of a reserved key or over another account's live
    /// claim, and a placement that `follows` the record it replaces. `verify` checks that an id block follows the
    /// group's last one.
    fn check(&self, ev: &NamesEvent) -> Result<(), Box<str>> {
        match ev {
            NamesEvent::LoginClaimed { key, account, .. } => {
                if key.is_reserved() {
                    return Err(format!("{key:?} is reserved").into());
                }
                match self.login(key) {
                    Some(LoginRow::Claimed { account: a, .. }) if a != *account => {
                        Err(format!("{key:?} is claimed by {a:?}").into())
                    }
                    _ => Ok(()),
                }
            }
            NamesEvent::Placement { owner, record } if !follows(self.placement(*owner), record) => {
                Err(format!("placement of {owner:?} does not follow {:?}", self.placement(*owner)).into())
            }
            _ => Ok(()),
        }
    }
}

impl Fold for NamesFold {
    type Event = NamesEvent;
    type Scratch<'a> = NamesScratch<'a>;
    fn scratch(&self) -> NamesScratch<'_> {
        NamesScratch {
            fold: self,
            logins: Overlay::new(&self.logins),
            accounts: Overlay::new(&self.accounts),
            placements: Overlay::new(&self.placements),
            repo_owners: Overlay::new(&self.repo_owners),
            ids: self.ids,
        }
    }
    fn apply(&mut self, ev: &NamesEvent) {
        write(
            ev,
            Tables {
                logins: &mut self.logins,
                accounts: &mut self.accounts,
                placements: &mut self.placements,
                repo_owners: &mut self.repo_owners,
                ids: &mut self.ids,
            },
        );
    }
    fn apply_id_block(&mut self, b: IdBlock) {
        self.ids.apply_block(b);
    }
}

// ---- Commands ----

/// One `Names` command and the credential it was submitted under (`CredentialShard::NONE` for `graft-ops`, the CLI,
/// and consumers).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamesCmd {
    pub cred: CredentialShard,
    pub op: NamesOp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NamesOp {
    /// Claim `login` on its shard. `account: None` is a signup: the account takes the next id of the group's block.
    /// `Some` is an existing account taking another login, the commit point of an owner rename.
    ClaimLogin {
        login: Login,
        kind: AccountKind,
        account: Option<AccountId>,
    },
    /// The account gives up `key`, which then points at it as a former name until someone claims it.
    ReleaseLogin {
        key: LoginKey,
        account: AccountId,
    },
    /// Writes the account's `Account` row on `account_shard(id)`: suspension and sign-out-everywhere.
    PutAccount {
        id: AccountId,
        suspended: bool,
        sessions_not_before: Timestamp,
    },
    /// A new owner's first record: a copy of the install account's record (`graftyard placement set default`).
    InitPlacement {
        owner: AccountId,
        record: PlacementRecord,
    },
    /// `graftyard placement set <owner>`, after `validate_record`: bumps `generation` and keeps the old record in
    /// `moving_from`.
    MovePlacement {
        owner: AccountId,
        name: PlacementName,
        sites: SmallVec<[SiteId; 3]>,
        home: SiteId,
        at: Timestamp,
    },
    /// The move Workflow's last step, once every group of the owner moved. Refused unless `generation` is the
    /// record's, so a stale Workflow cannot clear a later move's `moving_from`.
    FinishMove {
        owner: AccountId,
        generation: u32,
    },
    RepoOwner {
        repo: RepoId,
        owner: AccountId,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NamesOut {
    Account(AccountId),
    Placement(PlacementRecord),
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamesErr {
    /// 401: the credential's row is missing, revoked, or suspended.
    Unauthorized(CredentialRefusal),
    /// 403: the actor may not submit this op.
    Forbidden,
    /// The key belongs to another shard: the edge routed it wrong.
    WrongShard {
        want: NameShard,
    },
    Reserved,
    /// 422 `already_exists`.
    Taken {
        by: AccountId,
    },
    /// The account does not hold the login it releases.
    NotHolder,
    /// The group has no free id: lease a block from `Ids`, commit it here (`Member::id_block`), and re-send.
    NeedIdBlock,
    NoRecord,
    RecordExists,
    /// The move names the record the owner already has.
    Unchanged,
    StaleGeneration {
        current: u32,
    },
    BadRecord(PlacementError),
}

impl Record for NamesCmd {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        encode_cred(&mut e, &self.cred);
        match &self.op {
            NamesOp::ClaimLogin { login, kind, account } => {
                e.u8(0);
                e.str(&login.0);
                e.u8(*kind as u8);
                e.option(account.as_ref(), |e, a| enc::account(e, *a));
            }
            NamesOp::ReleaseLogin { key, account } => {
                e.u8(1);
                e.str(&key.0);
                enc::account(&mut e, *account);
            }
            NamesOp::PutAccount { id, suspended, sessions_not_before } => {
                e.u8(2);
                enc::account(&mut e, *id);
                e.u8(u8::from(*suspended));
                enc::at(&mut e, *sessions_not_before);
            }
            NamesOp::InitPlacement { owner, record } => {
                e.u8(3);
                enc::account(&mut e, *owner);
                enc::placement_record(&mut e, record);
            }
            NamesOp::MovePlacement { owner, name, sites, home, at } => {
                e.u8(4);
                enc::account(&mut e, *owner);
                let r = PlacementRecord {
                    name: *name,
                    sites: sites.clone(),
                    home: *home,
                    generation: 0,
                    moving_from: None,
                    at: *at,
                };
                enc::placement_record(&mut e, &r);
            }
            NamesOp::FinishMove { owner, generation } => {
                e.u8(5);
                enc::account(&mut e, *owner);
                e.u32(*generation);
            }
            NamesOp::RepoOwner { repo, owner } => {
                e.u8(6);
                enc::repo(&mut e, *repo);
                enc::account(&mut e, *owner);
            }
        }
    }
}

impl NamesCmd {
    /// The shard this command must run on, when it names a key.
    pub fn shard(&self) -> Option<NameShard> {
        match &self.op {
            NamesOp::ClaimLogin { login, .. } => Some(name_shard(&NameKey::Login(login.key()))),
            NamesOp::ReleaseLogin { key, .. } => Some(name_shard(&NameKey::Login(key.clone()))),
            NamesOp::PutAccount { id: owner, .. }
            | NamesOp::InitPlacement { owner, .. }
            | NamesOp::MovePlacement { owner, .. }
            | NamesOp::FinishMove { owner, .. } => Some(account_shard(*owner)),
            NamesOp::RepoOwner { repo, .. } => Some(name_shard(&NameKey::Repo(*repo))),
        }
    }

    /// Who may submit the op. `graft-ops`, the CLI, and finishers name no account and may submit any. A user may
    /// sign up an org, take or give up a login for itself, and end its own sessions; only an operator suspends,
    /// places, or records a repository owner. The follow-ups of a multi-chain operation
    /// (`RepoOwner`) are submitted by the finisher with no account, so a credential revoked in between cannot
    /// strand them.
    fn permitted(&self, s: &NamesScratch<'_>) -> bool {
        let Some(actor) = self.cred.account else { return true };
        match &self.op {
            NamesOp::ClaimLogin { account, kind, .. } => match account {
                Some(a) => *a == actor,
                None => *kind == AccountKind::Org,
            },
            NamesOp::ReleaseLogin { account, .. } => *account == actor,
            NamesOp::PutAccount { id, suspended, .. } => {
                *id == actor && s.account(*id).is_some_and(|r| r.suspended == *suspended)
            }
            NamesOp::InitPlacement { .. }
            | NamesOp::MovePlacement { .. }
            | NamesOp::FinishMove { .. }
            | NamesOp::RepoOwner { .. } => false,
        }
    }

    fn decide_op(&self, s: &NamesScratch<'_>) -> Result<(SmallVec<[NamesEvent; 4]>, NamesOut), NamesErr> {
        if !self.permitted(s) {
            return Err(NamesErr::Forbidden);
        }
        let placed = |owner: AccountId, record: PlacementRecord| {
            Ok((smallvec![NamesEvent::Placement { owner, record: record.clone() }], NamesOut::Placement(record)))
        };
        match &self.op {
            NamesOp::ClaimLogin { login, kind, account } => {
                let key = login.key();
                if key.is_reserved() {
                    return Err(NamesErr::Reserved);
                }
                let account = match (s.login(&key), account) {
                    (Some(LoginRow::Claimed { account: by, .. }), _) => return Err(NamesErr::Taken { by }),
                    (_, Some(a)) => *a,
                    (_, None) => AccountId(s.ids.peek().ok_or(NamesErr::NeedIdBlock)?),
                };
                Ok((smallvec![NamesEvent::LoginClaimed { key, account, kind: *kind }], NamesOut::Account(account)))
            }
            NamesOp::ReleaseLogin { key, account } => match s.login(key) {
                Some(LoginRow::Claimed { account: a, .. }) if a == *account => Ok((
                    smallvec![NamesEvent::FormerName { key: key.clone(), account: *account }],
                    NamesOut::Account(*account),
                )),
                _ => Err(NamesErr::NotHolder),
            },
            NamesOp::PutAccount { id, suspended, sessions_not_before } => Ok((
                smallvec![NamesEvent::Account {
                    id: *id,
                    suspended: *suspended,
                    sessions_not_before: *sessions_not_before
                }],
                NamesOut::Done,
            )),
            NamesOp::InitPlacement { owner, record } => {
                if s.placement(*owner).is_some() {
                    return Err(NamesErr::RecordExists);
                }
                Placement::from_record(ChainId::Authz(*owner), record).map_err(NamesErr::BadRecord)?;
                placed(*owner, PlacementRecord { moving_from: None, ..record.clone() })
            }
            NamesOp::MovePlacement { owner, name, sites, home, at } => {
                // A move during a move nests: the new record keeps the whole chain, so a lookup still walks to
                // every record a group may sit in (`PlacementRecord::lookup_order`).
                let prev = s.placement(*owner).ok_or(NamesErr::NoRecord)?;
                if prev.name == *name && prev.sites == *sites && prev.home == *home {
                    return Err(NamesErr::Unchanged);
                }
                let record = PlacementRecord {
                    name: *name,
                    sites: sites.clone(),
                    home: *home,
                    generation: prev.generation + 1,
                    moving_from: Some(Box::new(prev.clone())),
                    at: *at,
                };
                Placement::from_record(ChainId::Authz(*owner), &record).map_err(NamesErr::BadRecord)?;
                placed(*owner, record)
            }
            NamesOp::FinishMove { owner, generation } => {
                let prev = s.placement(*owner).ok_or(NamesErr::NoRecord)?;
                if prev.generation != *generation || prev.moving_from.is_none() {
                    return Err(NamesErr::StaleGeneration { current: prev.generation });
                }
                placed(*owner, PlacementRecord { moving_from: None, ..prev.clone() })
            }
            NamesOp::RepoOwner { repo, owner } => {
                Ok((smallvec![NamesEvent::RepoOwner { repo: *repo, owner: *owner }], NamesOut::Done))
            }
        }
    }
}

impl Command for NamesCmd {
    type Fold = NamesFold;
    type Dep = IdentityDep;
    type Out = NamesOut;
    type Err = NamesErr;

    fn format(&self) -> Format {
        NAMES_FORMAT
    }
    fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
    fn deps(&self, fold: &NamesFold) -> SmallVec<[ChainId; 3]> {
        credential_chains(&self.cred, fold.shard.map(ChainId::Names))
    }
    fn decide(
        &self,
        s: &NamesScratch<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[Ev<Self>; 4]>, NamesOut), NamesErr> {
        let own = match s.fold.shard {
            Some(shard) => Own::Names(shard, s),
            None => Own::Other,
        };
        check_credential(&self.cred, own, deps).map_err(NamesErr::Unauthorized)?;
        if let (Some(want), Some(have)) = (self.shard(), s.fold.shard)
            && want != have
        {
            return Err(NamesErr::WrongShard { want });
        }
        self.decide_op(s)
    }
    fn replay(&self, events: &[NamesEvent]) -> NamesOut {
        match events.first() {
            Some(NamesEvent::LoginClaimed { account, .. } | NamesEvent::FormerName { account, .. }) => {
                NamesOut::Account(*account)
            }
            Some(NamesEvent::Placement { record, .. }) => NamesOut::Placement(record.clone()),
            _ => NamesOut::Done,
        }
    }
}
