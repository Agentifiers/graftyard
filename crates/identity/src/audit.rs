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
//! A transfer is not finished here: its commit point, the destination's `Reserved` name, is on an `Authz` chain,
//! and its `Settings` step has no command yet (`crate::finish::transfer`).

use std::time::Duration;

use graftyard_raft::{EntryBody, LogEntry};
use graftyard_types::{AccountId, ChainId, CredentialShard, Index, Timestamp};
use serde::{Deserialize, Serialize};

use crate::apps::{AppId, ClientId, Slug};
use crate::authz::AuthzFold;
use crate::finish::{self, Step};
use crate::identity::{CredentialRef, IdentityFold};
use crate::names::{
    Login, LoginKey, NameKey, NamesCmd, NamesEvent, NamesFold, NamesOp, NamesRead, account_shard, name_shard,
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

/// Whether `(from, through]`, which owed `owed`, needs a batch: work to finish, or a stride to cover.
pub fn needs_batch(from: Index, through: Index, owed: &[Owed]) -> bool {
    through > from && (!owed.is_empty() || through.0 - from.0 >= CURSOR_STRIDE)
}

/// One operation an entry's commit point may have left unfinished.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Owed {
    Credential { account: AccountId, credential: CredentialRef },
    App { slug: Slug, app: AppId, owner: AccountId },
    Login { account: AccountId, key: LoginKey },
}

/// A fold the planner reads, as the site's `dep` read answers it.
#[derive(Clone, Debug, PartialEq)]
pub enum AuditRead {
    Names(NamesFold),
    Tokens(TokensFold),
    Authz(AuthzFold),
    Identity(IdentityFold),
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
        }
    }
}

/// The next step `o` owes, from the folds `read` gives for `o.reads()`; `Err` names a chain `read` lacks. A claim
/// whose record never committed is aborted (`finish::app_registration` with no record): the consumer has none.
pub fn plan<'a>(o: &Owed, read: impl Fn(ChainId) -> Option<&'a AuditRead>) -> Result<Option<Step>, ChainId> {
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
    })
}

/// The command that moves the consumer's cursor on `chain`, a `Names` or `Tokens` shard, to `through`.
pub fn cursor(chain: ChainId, through: Index) -> Option<Step> {
    let none = CredentialShard::NONE;
    match chain {
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
    from: Index,
    through: Index,
    owed: Vec<Owed>,
    at: usize,
    steps: usize,
    reads: Vec<(ChainId, AuditRead)>,
    cursor_sent: bool,
    report: Report,
}

impl Finisher {
    /// The batch for `(from, through]`, where `from` is the committed cursor.
    pub fn new(chain: ChainId, from: Index, through: Index, mut owed: Vec<Owed>) -> Finisher {
        owed.dedup();
        let report = Report { owed: owed.len() as u32, ..Report::default() };
        Finisher { chain, from, through, owed, at: 0, steps: 0, reads: Vec::new(), cursor_sent: false, report }
    }

    pub fn want(&mut self) -> Want {
        while let Some(o) = self.owed.get(self.at) {
            let reads = &self.reads;
            match plan(o, |c| reads.iter().find(|(r, _)| *r == c).map(|(_, f)| f)) {
                Ok(Some(_)) if self.steps >= STEPS_MAX => return Want::Stuck(o.clone()),
                Ok(Some(step)) => return Want::Submit(step),
                Ok(None) => self.item_done(),
                Err(_) => {
                    let missing = o.reads().into_iter().filter(|c| !reads.iter().any(|(r, _)| r == c)).collect();
                    return Want::Read(missing);
                }
            }
        }
        if self.cursor_sent || !needs_batch(self.from, self.through, &self.owed) {
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

/// The cursor a fold holds, for a `Names` or `Tokens` shard.
pub fn cursor_of(read: &AuditRead) -> Option<Index> {
    match read {
        AuditRead::Names(f) => Some(f.audit_cursor()),
        AuditRead::Tokens(f) => Some(f.audit_cursor()),
        _ => None,
    }
}
