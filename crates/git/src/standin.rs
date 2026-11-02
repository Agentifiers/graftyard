//! A stand-in for the repository chain's group until a site member's `Repo` slot hosts `Push`: three real
//! `graftyard_raft::Member<Push>`s of one chain on `Tri`, driven in one process, whose messages are delivered at once.
//! It runs the real Raft core, so every outcome the edge renders comes from the core's `outcome` logic and every `ok`
//! from a real `Acked`. What it does not have is distance: the three members share one object, so the quorum spans
//! two site ids, not two regions, and no message is ever lost or late.
//!
//! The `deps` reads (`Names`, `Tokens`, and the owner's `Authz`) are answered from identity folds built from a seed,
//! because the edge cannot reach those groups' leaders yet. The host persists every `Write` the members make through
//! `drain_writes` and rebuilds the group with `restore`.
//!
//! It answers in the boundary's wire types (`crate::group`), as a member's `/commit`, `/outcome`,
//! `/outcome_by_request`, and `/read` routes do.

use std::collections::{BTreeMap, HashMap};

use graftyard_identity::{
    AccountKind, AuthzEvent, AuthzFold, IdentityDep, NamesEvent, NamesFold, RepoPermission, ScopeSet, TokenKind,
    TokensEvent, TokensFold, UserId,
};
use graftyard_raft::{
    ByRequest, DepsWanted, EntryBody, Envelope, Fold, Format, LogEntry, Member, MemberConfig, MemberState, ReadId,
    ReleaseTag, RequestId, Resolution, Role, Storage, Stored, Ticket, Write, encode_entry,
};
use graftyard_types::{
    AccountId, ChainId, DbId, NameShard, PlacementName, PlacementRecord, Region, ReleaseId, RepoId, SiteId, Timestamp,
    TokenShard,
};
use serde::{Deserialize, Serialize};

use crate::group::{CallErrorWire, CommitReply, OutcomeReply, PushOutcome, ReadReply, ReadValue, outcome_wire};
use crate::ingest::LogPacks;
use crate::pack::PackRef;
use crate::repo::{Push, RepoEvent, RepoFold, packs_of};

pub const SITES: [SiteId; 3] = [SiteId(0), SiteId(1), SiteId(2)];
const LEADER: SiteId = SITES[0];

/// One account the stand-in's identity folds know, with the token it authenticates with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedUser {
    pub account: i64,
    /// Hex of the token's stored hash (`OpaqueToken::hash`).
    pub token_hash: String,
    #[serde(default)]
    pub revoked: bool,
    #[serde(default)]
    pub suspended: bool,
    /// A `Write` grant on the repository. The owner needs none.
    #[serde(default)]
    pub write: bool,
}

/// What the stand-in is built from: the repository, its owner, and the identity rows its `deps` read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seed {
    pub repo: i64,
    pub owner: i64,
    pub users: Vec<SeedUser>,
    /// The formats the members' release reads (release N). An edge on N+1 may know more.
    pub reads: Vec<(u16, u16)>,
}

fn hash_of(hex: &str) -> Option<[u8; 32]> {
    graftyard_types::Digest::parse_hex(hex).map(|d| d.0)
}

/// The identity folds the members' `deps` reads are answered from.
#[derive(Clone, Debug)]
pub struct Identity {
    names: NamesFold,
    tokens: TokensFold,
    authz: AuthzFold,
}

impl Identity {
    fn new(seed: &Seed, repo: RepoId) -> Identity {
        let owner = AccountId(DbId(seed.owner));
        let mut names = NamesFold::new(NameShard(0));
        let mut tokens = TokensFold::new(TokenShard(0));
        let mut authz = AuthzFold::new(owner);
        authz.apply(&AuthzEvent::Opened { kind: AccountKind::User, enterprise: None });
        for u in &seed.users {
            let id = AccountId(DbId(u.account));
            names.apply(&NamesEvent::Account { id, suspended: u.suspended, sessions_not_before: Timestamp(0) });
            if let Some(hash) = hash_of(&u.token_hash) {
                tokens.apply(&TokensEvent::Stored {
                    hash,
                    kind: TokenKind::Pat,
                    account: id,
                    scopes: ScopeSet(1),
                    expires: None,
                });
                if u.revoked {
                    tokens.apply(&TokensEvent::Revoked { hash });
                }
            }
            if u.write {
                authz.apply(&AuthzEvent::Grant { repo, grantee: UserId(id), permission: Some(RepoPermission::Write) });
            }
        }
        Identity { names, tokens, authz }
    }

    fn read(&self, c: ChainId) -> IdentityDep {
        match c {
            ChainId::Names(_) => IdentityDep::Names(self.names.clone()),
            ChainId::Tokens(_) => IdentityDep::Tokens(self.tokens.clone()),
            // The fold's own `RepoSettings` names the owner; any other `Authz` chain reads as unopened.
            ChainId::Authz(a) if Some(a) == graftyard_identity::AuthzRead::owner(&self.authz) => {
                IdentityDep::Authz(self.authz.clone())
            }
            ChainId::Authz(a) => IdentityDep::Authz(AuthzFold::new(a)),
            other => panic!("Push reads no {other} dep"),
        }
    }

    /// The token row an edge's authentication reads: (account, revoked).
    pub fn token(&self, hash: &[u8; 32]) -> Option<(AccountId, bool)> {
        graftyard_identity::TokensRead::token(&self.tokens, hash).map(|r| (r.account, r.revoked))
    }
}

/// Writes to the member's disk and keeps a copy for the host to persist.
struct Journal<'a> {
    site: SiteId,
    disk: &'a mut Stored<RepoFold>,
    out: &'a mut Vec<(SiteId, Write<RepoEvent>)>,
}

impl Storage<RepoEvent> for Journal<'_> {
    fn write(&mut self, w: &Write<RepoEvent>) {
        self.disk.write(w);
        self.out.push((self.site, w.clone()));
    }
}

/// What a host stored for one member: its state row and its log.
#[derive(Clone, Debug, Default)]
pub struct MemberRows {
    pub state: Option<MemberState>,
    pub log: Vec<LogEntry<RepoEvent>>,
}

/// One log entry as `/log` reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EntryInfo {
    pub index: u64,
    pub term: u64,
    pub format: (u16, u16),
    /// The canonical encoding's length, which the 1.5 MB entry bound measures.
    pub bytes: usize,
    pub requests: usize,
    pub ref_updates: usize,
    pub kind: &'static str,
    /// The packs the entry names: `stored <id> <bytes>` or `inline <bytes>`.
    pub packs: Vec<String>,
}

pub struct StandIn {
    pub chain: ChainId,
    pub now: Timestamp,
    pub repo: RepoId,
    members: Vec<Member<Push>>,
    disks: Vec<Stored<RepoFold>>,
    inbox: Vec<Envelope<RepoEvent>>,
    deps: Vec<(SiteId, DepsWanted)>,
    tickets: HashMap<Ticket, PushOutcome>,
    /// A read's answer, taken from the fold in the step that granted it: a `ReadGrant` is valid only until the
    /// member's next input, and `settle` delivers more.
    reads: HashMap<ReadId, ReadReply>,
    writes: Vec<(SiteId, Write<RepoEvent>)>,
    identity: Identity,
    pub unsupported: u64,
    boots: u64,
}

fn record() -> PlacementRecord {
    PlacementRecord {
        name: PlacementName::Tri,
        sites: SITES.iter().copied().collect(),
        home: LEADER,
        generation: 1,
        moving_from: None,
        at: Timestamp(1),
    }
}

fn config(chain: ChainId, me: SiteId, reads: &[Format]) -> MemberConfig {
    MemberConfig {
        chain,
        me,
        record: record(),
        regions: vec![(SITES[0], Region::Enam), (SITES[1], Region::Weur), (SITES[2], Region::Apac)],
        release: ReleaseTag { id: ReleaseId([0x4e; 32]), seq: 1 },
        reads: reads.to_vec(),
    }
}

impl StandIn {
    /// The group as its rows hold it; empty rows are a new group. Every member restarts, as a reset object does.
    pub fn restore(seed: &Seed, rows: Vec<MemberRows>, now: Timestamp) -> StandIn {
        let repo = RepoId(DbId(seed.repo));
        let chain = ChainId::Repo(repo);
        let fold = RepoFold::new(repo, AccountId(DbId(seed.owner)));
        let reads: Vec<Format> = seed.reads.iter().map(|(f, v)| Format { family: *f, version: *v }).collect();
        let mut s = StandIn {
            chain,
            now,
            repo,
            members: Vec::new(),
            disks: Vec::new(),
            inbox: Vec::new(),
            deps: Vec::new(),
            tickets: HashMap::new(),
            reads: HashMap::new(),
            writes: Vec::new(),
            identity: Identity::new(seed, repo),
            unsupported: 0,
            boots: now.0,
        };
        let mut rows = rows;
        rows.resize_with(SITES.len(), MemberRows::default);
        let mut readies = Vec::new();
        for (site, r) in SITES.iter().zip(rows) {
            let mut disk = Stored::new(fold.clone());
            let applied = r.state.as_ref().map_or(0, |st| st.applied.0) as usize;
            disk.state = r.state;
            disk.log = r.log;
            for e in &disk.log[..applied.min(disk.log.len())] {
                disk.fold.apply(e);
            }
            s.boots += 1;
            let (m, ready) = Member::restore(config(chain, *site, &reads), &disk, now, s.boots).expect("Tri record");
            s.disks.push(disk);
            s.members.push(m);
            readies.push((*site, ready));
        }
        for (site, ready) in readies {
            s.absorb(site, ready);
        }
        s
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Every write since the last call, for the host to make durable before it replies.
    pub fn drain_writes(&mut self) -> Vec<(SiteId, Write<RepoEvent>)> {
        std::mem::take(&mut self.writes)
    }

    fn i(s: SiteId) -> usize {
        s.0 as usize
    }

    fn absorb(&mut self, s: SiteId, ready: graftyard_raft::Ready<Push, ()>) {
        let ((), out) = {
            let mut j = Journal { site: s, disk: &mut self.disks[Self::i(s)], out: &mut self.writes };
            ready.persist(&mut j)
        };
        self.take_out(s, out);
    }

    fn take_out(&mut self, s: SiteId, mut out: graftyard_raft::Out<Push>) {
        loop {
            self.inbox.extend(out.early.drain(..).chain(out.messages.drain(..)));
            for r in out.resolutions.drain(..) {
                match r {
                    Resolution::Ticket { ticket, result } => {
                        self.tickets.insert(ticket, outcome_wire(&result));
                    }
                    Resolution::Read { id, result } => {
                        let m = &self.members[Self::i(s)];
                        let reply = match result.and_then(|g| m.fresh(&g)) {
                            Ok(f) => ReadReply::Refs(ReadValue {
                                index: f.index().0,
                                value: f.fold().refs.iter().map(|(n, o)| (n.display(), o.hex())).collect(),
                            }),
                            Err(nl) => ReadReply::Error { error: CallErrorWire::NotLeader((&nl).into()) },
                        };
                        self.reads.insert(id, reply);
                    }
                    _ => {}
                }
            }
            self.deps.extend(out.deps.drain(..).map(|d| (s, d)));
            let Some(ack) = out.self_ack.take() else { break };
            let now = self.now;
            let ready = self.members[Self::i(s)].self_ack(now, ack);
            let ((), next) = {
                let mut j = Journal { site: s, disk: &mut self.disks[Self::i(s)], out: &mut self.writes };
                ready.persist(&mut j)
            };
            out = next;
        }
    }

    fn on<R>(
        &mut self,
        s: SiteId,
        f: impl FnOnce(&mut Member<Push>, Timestamp) -> graftyard_raft::Ready<Push, R>,
    ) -> R {
        let now = self.now;
        let ready = f(&mut self.members[Self::i(s)], now);
        let (r, out) = {
            let mut j = Journal { site: s, disk: &mut self.disks[Self::i(s)], out: &mut self.writes };
            ready.persist(&mut j)
        };
        self.take_out(s, out);
        r
    }

    /// Delivers messages and answers `deps` reads until nothing is left.
    fn settle(&mut self) {
        for _ in 0..10_000 {
            if self.inbox.is_empty() && self.deps.is_empty() {
                return;
            }
            for e in std::mem::take(&mut self.inbox) {
                let Envelope { from, to, msg } = e;
                self.on(to, |m, now| m.step(now, from, msg));
            }
            for (s, want) in std::mem::take(&mut self.deps) {
                let deps = want.chains.iter().map(|c| (*c, self.identity.read(*c))).collect();
                self.on(s, |m, now| m.deps_read(now, want.id, deps));
            }
        }
        panic!("the stand-in group never settled");
    }

    /// Ticks every member at `now`, then settles.
    pub fn advance(&mut self, now: Timestamp) {
        self.now = Timestamp(self.now.0.max(now.0));
        for s in SITES {
            self.on(s, |m, t| m.tick(t));
        }
        self.settle();
    }

    /// Makes the home member lead, bootstrapping the group on first use.
    pub fn ensure_leader(&mut self) -> bool {
        for _ in 0..4 {
            if self.members[Self::i(LEADER)].role() == Role::Leader {
                return true;
            }
            self.on(LEADER, |m, now| m.campaign(now));
            self.settle();
        }
        self.members[Self::i(LEADER)].role() == Role::Leader
    }

    /// `commit`: admits the command on the leader and answers its outcome, which with every message delivered at
    /// once resolves within the call, or its ticket.
    pub fn commit(&mut self, rid: RequestId, push: Push) -> CommitReply {
        self.ensure_leader();
        let r = self.on(LEADER, |m, now| m.commit(now, rid, push));
        match r {
            Err(e) => {
                if matches!(e, graftyard_raft::CallError::Unsupported(_)) {
                    self.unsupported += 1;
                }
                CommitReply::Error((&e).into())
            }
            Ok(a) => {
                self.settle();
                match self.tickets.get(&a.ticket) {
                    Some(o) => CommitReply::Done(o.clone()),
                    None => CommitReply::Admitted(a.ticket.into()),
                }
            }
        }
    }

    pub fn outcome(&mut self, ticket: Ticket) -> OutcomeReply {
        self.settle();
        if let Some(o) = self.tickets.get(&ticket) {
            return OutcomeReply::Done(o.clone());
        }
        match self.members[Self::i(LEADER)].outcome(ticket) {
            Some(r) => OutcomeReply::Done(outcome_wire(&r)),
            None => OutcomeReply::Pending(ticket.into()),
        }
    }

    pub fn outcome_by_request(&mut self, rid: RequestId, push: &Push) -> OutcomeReply {
        self.ensure_leader();
        match self.members[Self::i(LEADER)].outcome_by_request(rid, push) {
            Ok(ByRequest::Done(a)) => OutcomeReply::Done(outcome_wire(&Ok(a))),
            Ok(ByRequest::Pending(t)) => OutcomeReply::Pending(t.into()),
            Ok(ByRequest::Unknown) => OutcomeReply::Unknown(true),
            Err(nl) => OutcomeReply::Error(CallErrorWire::NotLeader((&nl).into())),
        }
    }

    /// The refs through `fresh` on the leader, and the applied index they are at.
    pub fn read(&mut self) -> ReadReply {
        self.ensure_leader();
        let r = self.on(LEADER, |m, now| m.read(now));
        let id = match r {
            Ok(id) => id,
            Err(nl) => return ReadReply::Error { error: CallErrorWire::NotLeader((&nl).into()) },
        };
        self.settle();
        match self.reads.remove(&id) {
            Some(r) => r,
            None => ReadReply::Error {
                error: CallErrorWire::NotLeader((&self.members[Self::i(LEADER)].not_leader()).into()),
            },
        }
    }

    fn leader_log(&self) -> &[LogEntry<RepoEvent>] {
        self.members[Self::i(LEADER)].log()
    }

    /// The committed entries `from..=to` as a shard's `ChainLog` reads them.
    pub fn entries(&self, from: u64, to: u64) -> Vec<LogPacks> {
        let commit = self.members[Self::i(LEADER)].state().commit.0;
        self.leader_log()
            .iter()
            .filter(|e| e.index.0 >= from && e.index.0 <= to.min(commit))
            .map(|e| LogPacks {
                index: e.index,
                packs: match &e.body {
                    EntryBody::Events(evs) => packs_of(evs),
                    _ => Vec::new(),
                },
                promoted: None,
                consumed: Vec::new(),
            })
            .collect()
    }

    pub fn log(&self, from: u64) -> Vec<EntryInfo> {
        self.leader_log()
            .iter()
            .filter(|e| e.index.0 >= from)
            .map(|e| EntryInfo {
                index: e.index.0,
                term: e.term.0,
                format: (e.format.family, e.format.version),
                bytes: encode_entry(e).len(),
                requests: e.requests.len(),
                ref_updates: match &e.body {
                    EntryBody::Events(evs) => evs.iter().map(|ev| ev.updates().len()).sum(),
                    _ => 0,
                },
                kind: match &e.body {
                    EntryBody::Events(_) => "events",
                    EntryBody::Config(_) => "config",
                    EntryBody::Noop => "noop",
                    _ => "other",
                },
                packs: match &e.body {
                    EntryBody::Events(evs) => packs_of(evs)
                        .iter()
                        .map(|p| match p {
                            PackRef::Stored { id, bytes, .. } => format!("stored {id} {bytes}"),
                            PackRef::Inline(b) => format!("inline {}", b.len()),
                        })
                        .collect(),
                    _ => Vec::new(),
                },
            })
            .collect()
    }

    /// Every member's (last index, commit, applied), and whether the home member leads.
    pub fn status(&self) -> (Vec<(u64, u64, u64)>, bool) {
        (
            self.members.iter().map(|m| (m.log().len() as u64, m.state().commit.0, m.state().applied.0)).collect(),
            self.members[Self::i(LEADER)].role() == Role::Leader,
        )
    }

    pub fn refs(&self) -> BTreeMap<String, String> {
        self.members[Self::i(LEADER)].group_fold().fold.refs.iter().map(|(n, o)| (n.display(), o.hex())).collect()
    }
}
