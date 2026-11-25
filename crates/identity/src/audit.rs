//! The `Audit` consumer's pure half on `Tokens` and `Names` shards (`design/SHARED.md`, "Multi-chain operations",
//! Finisher, and "Event fan-out"; `design/DESIGN.md`, Shape, "Identity").
//!
//! - **What an entry owes.** A credential's `Tokens` row (`Stored`, `StoredWith`, `SshKey`, `Revoked`) owes its
//!   `Identity` side; an App's `AppSlug` claim owes its registration; a `LoginClaimed` owes the account's
//!   `AccountLogin` row. Each is planned by `crate::finish` from folds read now, so work the edge already finished
//!   plans to nothing.
//! - **A minute.** Only entries whose time is `FINISH_AFTER` behind the scan's clock are read, so the edge, which
//!   appends the follow-ups before it answers, is never raced; `scan` reports when the next entry is due.
//! - **The cursor.** `AuditCursor` on the scanned shard, committed after every step its batch owed has committed, so
//!   a new leader, a reset member, or a lost attempt starts again from it. The leader's dispatcher and the in-band
//!   first attempt are `PrSync`'s (`graftyard_git::prsync::Dispatcher`), on the site.
//!
//! - **Transfers and enterprise changes (`Authz`, format 0x12.4).** A `Reserved` name on an owner's `Authz` chain
//!   owes the rest of its transfer (`crate::finish::transfer`), and an org's `Enterprise` event owes the transfer
//!   `Settings` of each repository the org names `Live` (`crate::finish::enterprise_change`). Their plans read the
//!   repository chain too (`AuditRead::Repo`), and two of their steps are on it (`Step::Repo`): the group's move and
//!   the transfer's own `Settings`, which the site performs.
//! - **Repository create and delete (F59, Authz format 0x12.11, repository format 32.17).** A `RepoCreate` on an
//!   owner's `Authz` chain owes the rest of its create (`crate::finish::create`): `RepoOwner`, then the chain's first
//!   settings, or, once `crate::finish::CREATE_ABORT_AFTER` passed with the chain still not created, the abort that
//!   releases the name. A name turned `Deleted` owes the chain's tombstone (`crate::finish::delete`). Both read the
//!   repository chain only once the rest is read (`RepoRead::life`).

use std::time::Duration;

use graftyard_raft::{EntryBody, LogEntry};
use graftyard_types::{AccountId, ChainId, CredentialShard, Index, PlacementRecord, RepoId, SiteId, Timestamp};
use serde::{Deserialize, Serialize};

use crate::apps::{AppId, ClientId, Slug};
use crate::authz::{AuthzCmd, AuthzEvent, AuthzFold, AuthzOp, RepoEntry};
use crate::finish::{self, RepoLife, RepoStep, Step, Transfer, TransferStep, TransferView};
use crate::identity::{CredentialRef, IdentityFold};
use crate::names::{
    Login, LoginKey, NameKey, NamesCmd, NamesEvent, NamesFold, NamesOp, NamesRead, RepoNameKey, account_shard,
    name_shard,
};
use crate::tokens::{TokenKind, TokensCmd, TokensEvent, TokensFold, TokensOp, TokensRead};

/// SHARED: "The `Audit` consumer finishes any other chain operation that nobody touched within a minute".
pub const FINISH_AFTER: Duration = Duration::from_secs(60);
/// Steps, refused ones included, one owed item may take before the batch stops with the cursor unmoved [guess: an
/// App registration takes three]. SHARED parks a chain whose entry fails 10 times; the site counts attempts.
pub const STEPS_MAX: usize = 6;
/// A stretch of entries that owe nothing is still covered by a cursor entry this often, as `PrSync`'s
/// `CURSOR_STRIDE`, so a new leader never reads more than this many entries back [guess]. Below it, a batch that owes
/// nothing commits nothing: a cursor entry owes nothing, so moving the cursor past one would wake the next alarm to
/// move it again, forever.
pub const CURSOR_STRIDE: u64 = 1_000;

/// Whether `(from, through]`, which owed `owed` and holds `streamed` audit stream rows, needs a batch: work to finish,
/// rows to send, or a stride to cover.
pub fn needs_batch(from: Index, through: Index, owed: &[Owed], streamed: usize) -> bool {
    through > from && (!owed.is_empty() || streamed > 0 || through.0 - from.0 >= CURSOR_STRIDE)
}

/// One row of the audit stream (DESIGN, Stack: Basin Pipelines "Stream audit entries ... into Iceberg tables"): one
/// event of an entry the consumer scanned, sent before the cursor passes the entry, so a row is sent at least once.
/// `id` (chain, index, slot) is the key a reader deduplicates on. Only the variant and the ids an event names are
/// kept, never a hash, key, or secret [inference: the stream is operator data, and the chains stay the record].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StreamRow {
    pub id: String,
    pub chain: String,
    pub index: u64,
    pub slot: u16,
    pub at: u64,
    pub action: String,
    pub fields: serde_json::Map<String, serde_json::Value>,
}

/// The fields of an event a row keeps [inference: who and what, as GHES's audit log names them].
const STREAM_FIELDS: [&str; 11] =
    ["account", "owner", "org", "actor", "repo", "app", "slug", "login", "kind", "from", "to"];

/// The stream rows of entry `e` of `chain`: one per event but the consumer's own cursor.
pub fn stream_rows<E: Serialize>(chain: ChainId, e: &LogEntry<E>) -> Vec<StreamRow> {
    let EntryBody::Events(evs) = &e.body else { return Vec::new() };
    let mut out = Vec::new();
    for (slot, ev) in evs.iter().enumerate() {
        let (action, inner) = match serde_json::to_value(ev) {
            Ok(serde_json::Value::String(s)) => (s, serde_json::Value::Null),
            Ok(serde_json::Value::Object(m)) if m.len() == 1 => {
                let (k, v) = m.into_iter().next().expect("one");
                (k, v)
            }
            _ => continue,
        };
        if action == "AuditCursor" {
            continue;
        }
        let fields = match inner {
            serde_json::Value::Object(m) => {
                m.into_iter().filter(|(k, _)| STREAM_FIELDS.contains(&k.as_str())).collect()
            }
            _ => serde_json::Map::new(),
        };
        out.push(StreamRow {
            id: format!("{chain}:{}:{slot}", e.index.0),
            chain: chain.to_string(),
            index: e.index.0,
            slot: slot as u16,
            at: e.at.0,
            action,
            fields,
        });
    }
    out
}

/// One operation an entry's commit point may have left unfinished.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Owed {
    Credential {
        account: AccountId,
        credential: CredentialRef,
    },
    App {
        slug: Slug,
        app: AppId,
        owner: AccountId,
    },
    Login {
        account: AccountId,
        key: LoginKey,
    },
    /// A `Reserved` name on `to`'s `Authz` chain: the transfer of `repo` from `from`.
    Transfer {
        repo: RepoId,
        from: AccountId,
        to: AccountId,
        key: RepoNameKey,
    },
    /// An `Enterprise` event on `org`'s `Authz` chain.
    Enterprise {
        org: AccountId,
    },
    /// A `RepoCreate` on `owner`'s `Authz` chain in an entry of time `at`: the create of `repo` under `key`, with the
    /// settings asked for.
    Create {
        owner: AccountId,
        key: RepoNameKey,
        repo: RepoId,
        init: Box<str>,
        at: Timestamp,
    },
    /// A name on `owner`'s `Authz` chain turned `Deleted` until `retain_until`: the delete of `repo`.
    Delete {
        owner: AccountId,
        repo: RepoId,
        retain_until: Timestamp,
    },
}

/// A fold the planner reads, as the site's `dep` read answers it.
#[derive(Clone, Debug, PartialEq)]
pub enum AuditRead {
    Names(NamesFold),
    Tokens(TokensFold),
    Authz(AuthzFold),
    Identity(IdentityFold),
    /// A repository chain: the owner and enterprise its `RepoSettings` names (the chain's owner and none before a
    /// `Settings`), and its group's committed voter sites and home.
    Repo(RepoRead),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoRead {
    pub owner: AccountId,
    pub enterprise: Option<AccountId>,
    pub voters: Vec<SiteId>,
    pub home: SiteId,
    /// Whether the chain has a group, and whether it holds `Created` or `Deleted` (F59). A chain with no group reads
    /// as `NoGroup`, with no voters.
    pub life: RepoLife,
    /// F125: read only for a chain that holds `Deleted`: whether the owner's Actions shard still holds any of the
    /// repository's cache entries, or its home bucket any of its artifacts. Always `false` otherwise.
    pub actions_held: bool,
}

/// The credentials an account lists (`GET /user/keys`, PATs, OAuth tokens): not installation, refresh, or lifeboat
/// tokens, which are never listed [inference: the edge lists only what it issues to a person].
fn listed_kind(k: TokenKind) -> bool {
    matches!(k, TokenKind::Pat | TokenKind::FineGrained | TokenKind::OAuth | TokenKind::UserToServer)
}

/// What a `Tokens` entry owes. `fold` is the shard's fold now, for a `Revoked` row's account and kind.
pub fn tokens_owed(fold: &dyn TokensRead, e: &LogEntry<TokensEvent>) -> Vec<Owed> {
    let EntryBody::Events(evs) = &e.body else { return Vec::new() };
    let token = |hash: [u8; 32], kind: TokenKind, account: AccountId| {
        listed_kind(kind).then_some(Owed::Credential { account, credential: CredentialRef::Token { hash, kind } })
    };
    evs.iter()
        .filter_map(|ev| match ev {
            TokensEvent::Stored { hash, kind, account, .. } | TokensEvent::StoredWith { hash, kind, account, .. } => {
                token(*hash, *kind, *account)
            }
            TokensEvent::Revoked { hash } => fold.token(hash).and_then(|r| token(*hash, r.kind, r.account)),
            TokensEvent::SshKey { fingerprint, account, .. } => {
                Some(Owed::Credential { account: *account, credential: CredentialRef::SshKey(*fingerprint) })
            }
            _ => None,
        })
        .collect()
}

/// What an `Authz` entry of `owner`'s chain owes.
pub fn authz_owed(owner: AccountId, e: &LogEntry<AuthzEvent>) -> Vec<Owed> {
    let EntryBody::Events(evs) = &e.body else { return Vec::new() };
    evs.iter()
        .filter_map(|ev| match ev {
            AuthzEvent::RepoName { key, entry: Some(RepoEntry::Reserved { repo, from }) } => {
                Some(Owed::Transfer { repo: *repo, from: *from, to: owner, key: key.clone() })
            }
            AuthzEvent::Enterprise { .. } => Some(Owed::Enterprise { org: owner }),
            AuthzEvent::RepoCreate { key, repo, init } => {
                Some(Owed::Create { owner, key: key.clone(), repo: *repo, init: init.clone(), at: e.at })
            }
            AuthzEvent::RepoName { entry: Some(RepoEntry::Deleted { repo, retain_until }), .. } => {
                Some(Owed::Delete { owner, repo: *repo, retain_until: *retain_until })
            }
            _ => None,
        })
        .collect()
}

/// What a `Names` entry owes.
pub fn names_owed(e: &LogEntry<NamesEvent>) -> Vec<Owed> {
    let EntryBody::Events(evs) = &e.body else { return Vec::new() };
    evs.iter()
        .filter_map(|ev| match ev {
            NamesEvent::AppSlug { slug, app: Some(app), owner } => {
                Some(Owed::App { slug: slug.clone(), app: *app, owner: *owner })
            }
            NamesEvent::LoginClaimed { key, account, .. } => Some(Owed::Login { account: *account, key: key.clone() }),
            _ => None,
        })
        .collect()
}

impl Owed {
    /// The chains `plan` reads, each once per plan.
    pub fn reads(&self) -> Vec<ChainId> {
        match self {
            Owed::Credential { account, credential } => {
                vec![ChainId::Tokens(credential.shard()), ChainId::Identity(*account)]
            }
            Owed::App { slug, app, owner } => vec![
                ChainId::Names(name_shard(&NameKey::AppSlug(slug.clone()))),
                ChainId::Authz(*owner),
                ChainId::Names(name_shard(&NameKey::App(*app))),
                ChainId::Names(name_shard(&NameKey::Client(ClientId::for_app(*app)))),
            ],
            Owed::Login { account, key } => {
                vec![ChainId::Names(name_shard(&NameKey::Login(key.clone()))), ChainId::Names(account_shard(*account))]
            }
            // Both owners' `Authz` and `Placement` rows, the repository's `RepoOwner` row, and its chain.
            Owed::Transfer { repo, from, to, .. } => {
                let mut v = vec![ChainId::Authz(*from), ChainId::Authz(*to), ChainId::Names(account_shard(*from))];
                let to_shard = ChainId::Names(account_shard(*to));
                let repo_shard = ChainId::Names(name_shard(&NameKey::Repo(*repo)));
                for c in [to_shard, repo_shard, ChainId::Repo(*repo)] {
                    if !v.contains(&c) {
                        v.push(c);
                    }
                }
                v
            }
            // The org's `Authz`, then each repository it names `Live` (`Owed::repo_reads`).
            Owed::Enterprise { org } => vec![ChainId::Authz(*org)],
            // The owner's `Authz` and the `RepoOwner` row; the repository chain only once that row names the owner,
            // since the site finds the chain's members through it (`plan`).
            Owed::Create { owner, repo, .. } => {
                vec![ChainId::Authz(*owner), ChainId::Names(name_shard(&NameKey::Repo(*repo)))]
            }
            // The same two; the repository chain only while the name is still held `Deleted` for it.
            Owed::Delete { owner, repo, .. } => {
                vec![ChainId::Authz(*owner), ChainId::Names(name_shard(&NameKey::Repo(*repo)))]
            }
        }
    }

    /// The repository chains an enterprise change reads, from the org's fold: every repository it names `Live`.
    fn repo_reads(authz: &AuthzFold) -> Vec<RepoId> {
        authz
            .names
            .values()
            .filter_map(|e| match e {
                RepoEntry::Live { repo, .. } => Some(*repo),
                _ => None,
            })
            .collect()
    }
}

/// The next step `o` owes at `now`, from the folds `read` gives for `o.reads()`; `Err` names a chain `read` lacks. A
/// claim whose record never committed is aborted (`finish::app_registration` with no record): the consumer has none.
pub fn plan<'a>(
    o: &Owed,
    now: Timestamp,
    read: impl Fn(ChainId) -> Option<&'a AuditRead>,
) -> Result<Option<Step>, ChainId> {
    let names = |c: ChainId| match read(c) {
        Some(AuditRead::Names(f)) => Ok(f),
        _ => Err(c),
    };
    Ok(match o {
        Owed::Credential { account, credential } => {
            let c = ChainId::Tokens(credential.shard());
            let Some(AuditRead::Tokens(tokens)) = read(c) else { return Err(c) };
            let i = ChainId::Identity(*account);
            let Some(AuditRead::Identity(identity)) = read(i) else { return Err(i) };
            // An account with no `Created` lists nothing yet: its signup's follow-ups come first.
            if identity.created.is_none() {
                return Ok(None);
            }
            let title = match credential {
                CredentialRef::Token { kind, .. } => format!("{kind:?}"),
                CredentialRef::SshKey(_) => "SSH key".into(),
            };
            finish::credential(*account, *credential, &title, tokens, identity)
        }
        Owed::App { slug, app, owner } => {
            let [s, a, ar, cr] = <[ChainId; 4]>::try_from(o.reads()).expect("four reads");
            let Some(claim) = names(s)?.slug(slug).filter(|c| c.app == *app && c.owner == *owner) else {
                return Ok(None);
            };
            let Some(AuditRead::Authz(authz)) = read(a) else { return Err(a) };
            finish::app_registration(slug, claim, authz, names(ar)?, names(cr)?, None)
        }
        Owed::Login { account, key } => {
            let [l, h] = <[ChainId; 2]>::try_from(o.reads()).expect("two reads");
            // `LoginClaimed` holds only the key, so a login the finisher writes is spelled as the key; the edge's own
            // `SetAccountLogin` keeps the spelling the person chose (`finish::account_login` compares keys).
            let home = names(h)?;
            if home.account_login(*account).is_some_and(|have| have.key() == *key) {
                return Ok(None);
            }
            let Ok(login) = Login::parse(key.as_str()) else { return Ok(None) };
            finish::account_login(*account, &login, names(l)?, home)
        }
        Owed::Transfer { repo, from, to, key } => {
            let authz = |c: ChainId| match read(c) {
                Some(AuditRead::Authz(f)) => Ok(f),
                _ => Err(c),
            };
            let (fa, ta) = (authz(ChainId::Authz(*from))?, authz(ChainId::Authz(*to))?);
            // The source's live name for the repository, which becomes the `Alias`; a source that already holds the
            // `Alias` (or nothing) passes the reserved key, which `transfer` finds no `Live` entry under.
            let from_key = fa
                .names
                .iter()
                .find_map(|(k, e)| matches!(e, RepoEntry::Live { repo: r, .. } if r == repo).then(|| k.clone()))
                .unwrap_or_else(|| key.clone());
            let place = |owner: AccountId| -> Result<Option<PlacementRecord>, ChainId> {
                Ok(names(ChainId::Names(account_shard(owner)))?.placement(owner).cloned())
            };
            let (Some(fp), Some(tp)) = (place(*from)?, place(*to)?) else { return Ok(None) };
            let rc = ChainId::Repo(*repo);
            let Some(AuditRead::Repo(r)) = read(rc) else { return Err(rc) };
            let repo_owner = names(ChainId::Names(name_shard(&NameKey::Repo(*repo))))?.repo_owner(*repo);
            let t = Transfer { repo: *repo, from: *from, from_key, to: *to, to_key: key.clone() };
            let v = TransferView {
                from_authz: fa,
                to_authz: ta,
                from_placement: &fp,
                to_placement: &tp,
                group: (&r.voters, r.home),
                settings_owner: r.owner,
                settings_enterprise: r.enterprise,
                repo_owner,
            };
            finish::transfer(&t, &v).map(|s| match s {
                TransferStep::Step(s) => s,
                TransferStep::Move { to } => Step::Repo(*repo, RepoStep::Move { to }),
                TransferStep::Settings { owner, enterprise } => {
                    Step::Repo(*repo, RepoStep::Settings { owner, enterprise })
                }
            })
        }
        Owed::Enterprise { org } => {
            let a = ChainId::Authz(*org);
            let Some(AuditRead::Authz(f)) = read(a) else { return Err(a) };
            let live = Owed::repo_reads(f);
            let repos = |r: RepoId| match read(ChainId::Repo(r)) {
                Some(AuditRead::Repo(x)) => Some((x.owner, x.enterprise)),
                _ => None,
            };
            finish::enterprise_change(*org, f, &live, &repos).map_err(ChainId::Repo)?
        }
        Owed::Create { owner, repo, init, at, .. } => {
            let a = ChainId::Authz(*owner);
            let Some(AuditRead::Authz(authz)) = read(a) else { return Err(a) };
            let repo_owner = names(ChainId::Names(name_shard(&NameKey::Repo(*repo))))?.repo_owner(*repo);
            let rc = ChainId::Repo(*repo);
            let life = match read(rc) {
                Some(AuditRead::Repo(r)) => Some(r.life),
                _ => None,
            };
            let c = finish::Create { owner: *owner, repo: *repo, init, at: *at };
            finish::create(&c, authz, repo_owner, life, now).map_err(|()| rc)?
        }
        Owed::Delete { owner, repo, retain_until } => {
            let a = ChainId::Authz(*owner);
            let Some(AuditRead::Authz(authz)) = read(a) else { return Err(a) };
            let repo_owner = names(ChainId::Names(name_shard(&NameKey::Repo(*repo))))?.repo_owner(*repo);
            let rc = ChainId::Repo(*repo);
            let (life, held) = match read(rc) {
                Some(AuditRead::Repo(r)) => (Some(r.life), r.actions_held),
                _ => (None, false),
            };
            finish::delete(*owner, *repo, *retain_until, authz, repo_owner, life, held, now).map_err(|()| rc)?
        }
    })
}

/// The command that moves the consumer's cursor on `chain`, a `Names` or `Tokens` shard or an `Authz` chain, to
/// `through`.
pub fn cursor(chain: ChainId, through: Index) -> Option<Step> {
    let none = CredentialShard::NONE;
    match chain {
        ChainId::Authz(_) => {
            Some(Step::Authz(chain, Box::new(AuthzCmd { cred: none, op: AuthzOp::AuditCursor { through } })))
        }
        ChainId::Names(s) => {
            Some(Step::Names(chain, NamesCmd { cred: none, op: NamesOp::AuditCursor { shard: s, through } }))
        }
        ChainId::Tokens(s) => Some(Step::Tokens(
            chain,
            Box::new(TokensCmd { cred: none, op: TokensOp::AuditCursor { shard: s, through } }),
        )),
        _ => None,
    }
}

/// Where a scan stopped: the last entry read, and the time the next entry is due, if one is waiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scanned {
    pub through: Index,
    pub next_due: Option<Timestamp>,
}

/// Reads `(from, applied]` in order, at most `max` entries, each by `entry`, and hands each entry that is due at
/// `now` to `owed` with its index. Stops at the first entry younger than `FINISH_AFTER`, and at one `entry` cannot
/// give (`Err` names it), so nothing past either is read.
pub fn scan<E>(
    from: Index,
    applied: Index,
    now: Timestamp,
    max: u64,
    mut entry: impl FnMut(Index) -> Option<LogEntry<E>>,
    mut owed: impl FnMut(Index, &LogEntry<E>),
) -> Result<Scanned, Index> {
    let after = FINISH_AFTER.as_millis() as u64;
    let mut through = from;
    for i in from.0 + 1..=applied.0.min(from.0.saturating_add(max)) {
        let e = entry(Index(i)).ok_or(Index(i))?;
        if e.at.0.saturating_add(after) > now.0 {
            return Ok(Scanned { through, next_due: Some(Timestamp(e.at.0 + after)) });
        }
        owed(Index(i), &e);
        through = Index(i);
    }
    Ok(Scanned { through, next_due: None })
}

/// What a batch's driver wants next (`Finisher::want`).
#[derive(Clone, Debug, PartialEq)]
pub enum Want {
    /// Read these chains through `fresh` and hand each to `Finisher::read`; a chain with no group reads as its empty
    /// fold.
    Read(Vec<ChainId>),
    /// Submit this command, then call `Finisher::submitted`.
    Submit(Step),
    /// Every owed step and then the cursor committed.
    Done(Report),
    /// This item took `STEPS_MAX` steps and still owes one: the batch stops, and the cursor stays.
    Stuck(Owed),
}

/// What one batch did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub owed: u32,
    pub steps: u32,
    pub refused: u32,
}

/// One batch of the `Audit` consumer, without I/O: the entries `(cursor, through]` of `chain` owed `owed`. Each item
/// is planned from folds read for it, its step submitted, and planned again on fresh reads, until it owes nothing. A
/// refused step is planned again too: a racing writer changed a fold, and the next plan sees it. Only after the last
/// item does the cursor move, so a batch that stops partway, or an item `Stuck` after `STEPS_MAX` steps, leaves the
/// cursor where a new attempt starts it again.
#[derive(Clone, Debug)]
pub struct Finisher {
    chain: ChainId,
    /// The batch's clock: a create aborts only once `finish::CREATE_ABORT_AFTER` passed by it.
    now: Timestamp,
    from: Index,
    through: Index,
    owed: Vec<Owed>,
    at: usize,
    steps: usize,
    reads: Vec<(ChainId, AuditRead)>,
    cursor_sent: bool,
    /// Stream rows the host sent for `(from, through]`, so the cursor moves past them.
    streamed: usize,
    report: Report,
}

impl Finisher {
    /// The batch for `(from, through]`, where `from` is the committed cursor, run at `now`.
    pub fn new(chain: ChainId, now: Timestamp, from: Index, through: Index, mut owed: Vec<Owed>) -> Finisher {
        owed.dedup();
        let report = Report { owed: owed.len() as u32, ..Report::default() };
        Finisher {
            chain,
            now,
            from,
            through,
            owed,
            at: 0,
            steps: 0,
            reads: Vec::new(),
            cursor_sent: false,
            streamed: 0,
            report,
        }
    }

    /// The host sent `n` stream rows for the batch's range: the cursor must pass them even when nothing is owed.
    pub fn streamed(mut self, n: usize) -> Finisher {
        self.streamed = n;
        self
    }

    pub fn want(&mut self) -> Want {
        while let Some(o) = self.owed.get(self.at) {
            let reads = &self.reads;
            match plan(o, self.now, |c| reads.iter().find(|(r, _)| *r == c).map(|(_, f)| f)) {
                Ok(Some(_)) if self.steps >= STEPS_MAX => return Want::Stuck(o.clone()),
                Ok(Some(step)) => return Want::Submit(step),
                Ok(None) => self.item_done(),
                Err(c) => {
                    let mut missing: Vec<ChainId> =
                        o.reads().into_iter().filter(|c| !reads.iter().any(|(r, _)| r == c)).collect();
                    // A chain the plan found it needs only after reading the others (an enterprise change's
                    // repositories).
                    if missing.is_empty() {
                        missing.push(c);
                    }
                    return Want::Read(missing);
                }
            }
        }
        if self.cursor_sent || !needs_batch(self.from, self.through, &self.owed, self.streamed) {
            return Want::Done(self.report);
        }
        match cursor(self.chain, self.through) {
            Some(step) => Want::Submit(step),
            None => Want::Done(self.report),
        }
    }

    pub fn read(&mut self, chain: ChainId, fold: AuditRead) {
        self.reads.retain(|(c, _)| *c != chain);
        self.reads.push((chain, fold));
    }

    /// The step `want` gave committed, or was refused. Every fold is read again before the next plan, so no plan
    /// rests on a fold from before a write it caused.
    pub fn submitted(&mut self, refused: bool) {
        if self.at >= self.owed.len() {
            self.cursor_sent = true;
            return;
        }
        self.reads.clear();
        self.report.steps += 1;
        self.steps += 1;
        if refused {
            self.report.refused += 1;
        }
    }

    fn item_done(&mut self) {
        self.at += 1;
        self.steps = 0;
        self.reads.clear();
    }
}

/// The cursor a fold holds, for a `Names` or `Tokens` shard or an `Authz` chain.
pub fn cursor_of(read: &AuditRead) -> Option<Index> {
    match read {
        AuditRead::Names(f) => Some(f.audit_cursor()),
        AuditRead::Tokens(f) => Some(f.audit_cursor()),
        AuditRead::Authz(f) => Some(f.audit_cursor()),
        _ => None,
    }
}
