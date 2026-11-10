//! The `Audit` consumer on real `graftyard_raft::Member`s (`design/SHARED.md`, "Multi-chain operations", Finisher):
//! it reads a `Names` or `Tokens` shard's log from its committed cursor, waits until an entry is a minute old,
//! finishes what that entry's commit point left unfinished, and only then moves its cursor on that shard. Every fold
//! is read through `fresh` on its own group, as the site's consumer reads it.

mod common;

use common::world::{FreshLeader, OPS, World};
use common::{A, acct, login};
use graftyard_identity::audit::{self, AuditRead, Finisher, Owed, Want};
use graftyard_identity::finish::Step;
use graftyard_identity::{
    AccountCapture, AccountKind, AppKey, AppRecord, AuthzCmd, AuthzOp, AuthzRead, ClientId, CredentialRef, Fingerprint,
    IdentityCmd, IdentityFold, IdentityOp, NameKey, NamesCmd, NamesFold, NamesOp, NamesOut, NamesRead, PermissionSet,
    RsaPublic, ScopeSet, SecretHash, Slug, TokenKind, TokensCmd, TokensFold, TokensOp, account_shard, name_shard,
    shard_of_hash,
};
use graftyard_raft::{CommitError, Record};
use graftyard_types::{AccountId, ChainId, CredentialShard, DbId, Index, Timestamp};
use smallvec::smallvec;

const MINUTE: u64 = 60_000;

fn as_user(u: AccountId) -> CredentialShard {
    CredentialShard { account: Some(u), key: None }
}

fn created(w: &mut World, u: AccountId, l: &str) {
    let op = NamesOp::PutAccount { id: u, suspended: false, sessions_not_before: Timestamp(0) };
    w.names_cmd(ChainId::Names(account_shard(u)), NamesCmd { cred: OPS, op }).unwrap();
    let c = AccountCapture { login: login(l), kind: AccountKind::User, created: Timestamp(1) };
    w.identity_cmd(u, IdentityCmd { cred: OPS, op: IdentityOp::Create(c) }).unwrap();
}

/// A fold for `chain` through `fresh` on its group, or its empty fold when no group exists yet, as the site's
/// consumer reads a chain whose group nobody created.
fn read(w: &mut World, c: ChainId) -> AuditRead {
    match c {
        ChainId::Names(_) => AuditRead::Names(w.names(c).fresh_leader()),
        ChainId::Tokens(s) => AuditRead::Tokens(w.tokens(s).fresh_leader()),
        ChainId::Authz(o) => AuditRead::Authz(w.authz(o).fresh_leader()),
        ChainId::Identity(a) => AuditRead::Identity(w.identity(a).fresh_leader()),
        other => panic!("audit reads no {other}"),
    }
}

/// Submits a step, and says whether it was refused.
fn submit(w: &mut World, s: Step) -> bool {
    match s {
        Step::Names(chain, cmd) => w.names_cmd(chain, cmd).is_err(),
        Step::Tokens(_, cmd) => w.tokens_cmd(*cmd).is_err(),
        Step::Authz(ChainId::Authz(o), cmd) => w.authz_cmd(o, *cmd).is_err(),
        Step::Identity(ChainId::Identity(a), cmd) => w.identity_cmd(a, cmd).is_err(),
        other => panic!("{other:?}"),
    }
}

/// The site's consumer for one shard, in memory: the shard leader's committed entries from the cursor, a minute
/// behind `now`, then one `Finisher` batch driven to its end. Returns the report, or `None` when nothing was due.
fn run_batch(w: &mut World, chain: ChainId, now: Timestamp) -> Option<audit::Report> {
    let (cursor, entries) = match chain {
        ChainId::Names(_) => {
            let g = w.names(chain);
            let s = g.leader().unwrap();
            let m = g.m(s);
            let commit = m.state().commit.0 as usize;
            (m.group_fold().fold.audit_cursor(), Ok(m.log()[..commit].to_vec()))
        }
        ChainId::Tokens(sh) => {
            let g = w.tokens(sh);
            let s = g.leader().unwrap();
            let m = g.m(s);
            let commit = m.state().commit.0 as usize;
            (m.group_fold().fold.audit_cursor(), Err(m.log()[..commit].to_vec()))
        }
        _ => unreachable!(),
    };
    let mut owed = Vec::new();
    let scanned = match entries {
        Ok(log) => {
            let applied = Index(log.len() as u64);
            audit::scan(
                cursor,
                applied,
                now,
                2_000,
                |i| log.get(i.0 as usize - 1).cloned(),
                |_, e| owed.extend(audit::names_owed(e)),
            )
        }
        Err(log) => {
            let ChainId::Tokens(sh) = chain else { unreachable!() };
            let fold: TokensFold = w.tokens(sh).fresh_leader();
            let applied = Index(log.len() as u64);
            audit::scan(
                cursor,
                applied,
                now,
                2_000,
                |i| log.get(i.0 as usize - 1).cloned(),
                |_, e| owed.extend(audit::tokens_owed(&fold, e)),
            )
        }
    }
    .expect("every entry is in memory");
    if scanned.through <= cursor {
        return None;
    }
    let mut f = Finisher::new(chain, now, cursor, scanned.through, owed);
    for _ in 0..200 {
        match f.want() {
            Want::Read(cs) => {
                for c in cs {
                    let fold = read(w, c);
                    f.read(c, fold);
                }
            }
            Want::Submit(s) => {
                let refused = submit(w, s);
                f.submitted(refused);
            }
            Want::Done(r) => return Some(r),
            Want::Stuck(o) => panic!("stuck on {o:?}"),
        }
    }
    panic!("the batch never finished");
}

/// A minute passes on every group: each member ticks, and whatever elections follow settle.
fn advance_all(w: &mut World, ms: u64) {
    fn tick<C: graftyard_raft::Command>(g: &mut common::Group<C>, ms: u64) {
        g.advance(ms);
        g.deliver_all();
        if g.leader().is_none() {
            g.elect(A);
        }
    }
    w.names.values_mut().for_each(|g| tick(g, ms));
    w.tokens.values_mut().for_each(|g| tick(g, ms));
    w.identity.values_mut().for_each(|g| tick(g, ms));
    w.authz.values_mut().for_each(|g| tick(g, ms));
}

fn now_of_tokens(w: &mut World, h: &[u8; 32]) -> Timestamp {
    w.tokens(shard_of_hash(h)).now
}

/// BF-35's other half: a PAT whose edge died between its `Tokens` commit and its `Identity` side is listed by the
/// consumer once a minute passed, and not before; the cursor moves past it only after the listing committed, and a
/// second run owes nothing. A PAT the edge finished costs the consumer no step.
#[test]
fn audit_lists_a_credential_its_edge_left_unlisted_once_a_minute_passed() {
    let mut w = World::default();
    let u = acct(0, 2, 1);
    created(&mut w, u, "alice");
    let (dead, done) = ([0x11; 32], [0x12; 32]);
    assert_eq!(shard_of_hash(&dead), shard_of_hash(&dead));
    for h in [dead, done] {
        let op = TokensOp::Store { hash: h, kind: TokenKind::Pat, account: u, scopes: ScopeSet(1), expires: None };
        w.tokens_cmd(TokensCmd { cred: as_user(u), op }).unwrap();
    }
    // The edge finished `done` itself.
    let add = IdentityOp::AddCredential {
        credential: CredentialRef::Token { hash: done, kind: TokenKind::Pat },
        title: "laptop".into(),
    };
    w.identity_cmd(u, IdentityCmd { cred: as_user(u), op: add }).unwrap();
    let chain = ChainId::Tokens(shard_of_hash(&dead));
    let listed = |w: &mut World| -> IdentityFold { w.identity(u).fresh_leader() };
    let pat = CredentialRef::Token { hash: dead, kind: TokenKind::Pat };
    // Inside the minute: nothing is due, nothing is listed, the cursor stays.
    let now = now_of_tokens(&mut w, &dead);
    assert_eq!(run_batch(&mut w, chain, now), None);
    assert!(!listed(&mut w).credentials.contains_key(&pat));
    // A minute later the consumer lists it, then moves its cursor.
    advance_all(&mut w, MINUTE + 1);
    let now = now_of_tokens(&mut w, &dead);
    if shard_of_hash(&dead) == shard_of_hash(&done) {
        let r = run_batch(&mut w, chain, now).expect("due");
        assert_eq!((r.owed, r.steps, r.refused), (2, 1, 0), "one listing; the finished PAT costs none");
    } else {
        let r = run_batch(&mut w, chain, now).expect("due");
        assert_eq!((r.owed, r.steps, r.refused), (1, 1, 0), "one listing");
    }
    assert!(listed(&mut w).credentials.contains_key(&pat));
    let t: TokensFold = w.tokens(shard_of_hash(&dead)).fresh_leader();
    assert!(t.audit_cursor() > Index(0));
    // The cursor's own entry is not yet a minute old; once it is, it owes nothing, and the cursor does not move past
    // it, or every cursor entry would wake the alarm for another, forever.
    let at = t.audit_cursor();
    assert_eq!(run_batch(&mut w, chain, now), None);
    advance_all(&mut w, MINUTE + 1);
    let now = now_of_tokens(&mut w, &dead);
    let r = run_batch(&mut w, chain, now).expect("the cursor's entry is due");
    assert_eq!((r.owed, r.steps), (0, 0), "nothing owed");
    let t: TokensFold = w.tokens(shard_of_hash(&dead)).fresh_leader();
    assert_eq!(t.audit_cursor(), at, "and no cursor entry");
    // A revocation's entry owes the drop, which the consumer makes.
    w.tokens_cmd(TokensCmd { cred: as_user(u), op: TokensOp::Revoke { hash: dead } }).unwrap();
    advance_all(&mut w, MINUTE + 1);
    let now = now_of_tokens(&mut w, &dead);
    run_batch(&mut w, chain, now).expect("due");
    assert!(!listed(&mut w).credentials.contains_key(&pat), "a revoked PAT is dropped");
}

/// 100 PATs and 100 SSH keys, each from an edge that died after the `Tokens` commit: all 200 are listed after one
/// consumer pass over their shards, 200 of 200, and a second pass owes none.
#[test]
fn audit_lists_every_credential_whose_edge_died_200_of_200() {
    let mut w = World::default();
    let u = acct(0, 2, 7);
    created(&mut w, u, "bulk");
    let mut creds = Vec::new();
    for i in 0..100u32 {
        let mut h = [0u8; 32];
        h[..4].copy_from_slice(&i.to_be_bytes());
        h[4] = 0x5a;
        let op = TokensOp::Store { hash: h, kind: TokenKind::Pat, account: u, scopes: ScopeSet(1), expires: None };
        w.tokens_cmd(TokensCmd { cred: as_user(u), op }).unwrap();
        creds.push(CredentialRef::Token { hash: h, kind: TokenKind::Pat });
        let mut f = [0u8; 32];
        f[..4].copy_from_slice(&i.to_be_bytes());
        f[4] = 0x6b;
        let op = TokensOp::AddSshKey { fingerprint: Fingerprint(f), account: u, repo: None };
        w.tokens_cmd(TokensCmd { cred: as_user(u), op }).unwrap();
        creds.push(CredentialRef::SshKey(Fingerprint(f)));
    }
    advance_all(&mut w, MINUTE + 1);
    let shards: std::collections::BTreeSet<_> = creds.iter().map(|c| c.shard()).collect();
    for s in &shards {
        let now = w.tokens(*s).now;
        run_batch(&mut w, ChainId::Tokens(*s), now).expect("due");
    }
    let f: IdentityFold = w.identity(u).fresh_leader();
    let listed = creds.iter().filter(|c| f.credentials.contains_key(c)).count();
    assert_eq!(listed, 200, "listed by the consumer");
    advance_all(&mut w, MINUTE + 1);
    for s in &shards {
        let now = w.tokens(*s).now;
        let r = run_batch(&mut w, ChainId::Tokens(*s), now).expect("the cursor entries are due");
        assert_eq!((r.owed, r.steps), (0, 0), "nothing owed twice");
    }
}

/// A batch whose attempt died after its step committed but before the cursor moved runs again from the committed
/// cursor under a new leader, and finishes with nothing done twice.
#[test]
fn a_batch_lost_before_its_cursor_runs_again_from_the_cursor_under_a_new_leader() {
    let mut w = World::default();
    let u = acct(0, 2, 9);
    created(&mut w, u, "carol");
    let h = [0x33; 32];
    let op = TokensOp::Store { hash: h, kind: TokenKind::Pat, account: u, scopes: ScopeSet(1), expires: None };
    w.tokens_cmd(TokensCmd { cred: as_user(u), op }).unwrap();
    advance_all(&mut w, MINUTE + 1);
    let chain = ChainId::Tokens(shard_of_hash(&h));
    // The first attempt is lost right after its first step committed: the listing, so the cursor has not moved.
    let (log, cursor) = {
        let g = w.tokens(shard_of_hash(&h));
        let m = g.m(g.leader().unwrap());
        (m.log()[..m.state().commit.0 as usize].to_vec(), m.group_fold().fold.audit_cursor())
    };
    let fold: TokensFold = w.tokens(shard_of_hash(&h)).fresh_leader();
    let now = w.tokens(shard_of_hash(&h)).now;
    let mut owed = Vec::new();
    let scanned = audit::scan(
        cursor,
        Index(log.len() as u64),
        now,
        2_000,
        |i| log.get(i.0 as usize - 1).cloned(),
        |_, e| owed.extend(audit::tokens_owed(&fold, e)),
    )
    .unwrap();
    let mut f = Finisher::new(chain, now, cursor, scanned.through, owed);
    loop {
        match f.want() {
            Want::Read(cs) => {
                for c in cs {
                    let fold = read(&mut w, c);
                    f.read(c, fold);
                }
            }
            Want::Submit(s) => {
                submit(&mut w, s);
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    let t: TokensFold = w.tokens(shard_of_hash(&h)).fresh_leader();
    assert_eq!(t.audit_cursor(), Index(0), "the cursor moves only after the work it covers");
    // A new leader on the shard: its consumer starts at the committed cursor and owes nothing but the cursor.
    let g = w.tokens(shard_of_hash(&h));
    let old = g.leader().unwrap();
    g.crash(old);
    g.elect_after_promises(common::SITES[(old.0 as usize + 1) % 3]);
    g.restart(old);
    let now = w.tokens(shard_of_hash(&h)).now;
    let r = run_batch(&mut w, chain, now).expect("due again");
    assert_eq!((r.owed, r.steps, r.refused), (1, 0, 0), "the listing is planned to nothing; only the cursor moves");
    let id: IdentityFold = w.identity(u).fresh_leader();
    assert_eq!(id.credentials.len(), 1);
    let t: TokensFold = w.tokens(shard_of_hash(&h)).fresh_leader();
    assert_eq!(t.audit_cursor(), scanned.through.max(t.audit_cursor()));
    assert!(t.audit_cursor() >= scanned.through);
}

fn app_record(app: graftyard_identity::AppId, slug: &Slug) -> AppRecord {
    AppRecord {
        id: app,
        slug: slug.clone(),
        name: "Builder".into(),
        client: ClientId::for_app(app),
        keys: smallvec![AppKey { id: DbId(1), public: RsaPublic(vec![0x30, 0x82].into()), created: Timestamp(5) }],
        permissions: PermissionSet::default(),
        events: smallvec![],
        webhook: None,
        webhook_secret: None,
        client_secrets: smallvec![SecretHash([7; 32])],
        callbacks: smallvec![],
        public: false,
    }
}

/// An App registration whose edge died after `PutApp` is finished by the consumer from the slug shard's log, and one
/// whose edge died right after the claim is aborted: its App is closed and its slug released. A login claimed by a
/// signup whose edge died is given its `AccountLogin` row.
#[test]
fn audit_finishes_app_registrations_and_login_rows_from_the_names_log() {
    let mut w = World::default();
    let owner = acct(0, 9, 1);
    w.authz_cmd(owner, AuthzCmd { cred: OPS, op: AuthzOp::Open { kind: AccountKind::Org, enterprise: None } }).unwrap();
    let claim = |w: &mut World, s: &Slug| match w
        .names_on(NameKey::AppSlug(s.clone()), NamesOp::ClaimSlug { slug: s.clone(), owner })
        .unwrap()
        .out
    {
        NamesOut::App(a) => a,
        o => panic!("{o:?}"),
    };
    let built = Slug::parse("builder").unwrap();
    let app = claim(&mut w, &built);
    w.authz_cmd(owner, AuthzCmd { cred: OPS, op: AuthzOp::PutApp { record: Box::new(app_record(app, &built)) } })
        .unwrap();
    let dead = Slug::parse("orphan").unwrap();
    let orphan = claim(&mut w, &dead);
    // A signup whose edge died after `ClaimLogin`.
    let k = NameKey::Login(login("Dora").key());
    let a = match w
        .names_on(k.clone(), NamesOp::ClaimLogin { login: login("Dora"), kind: AccountKind::User, account: None })
        .unwrap()
        .out
    {
        NamesOut::Account(a) => a,
        o => panic!("{o:?}"),
    };
    advance_all(&mut w, MINUTE + 1);
    let mut chains = vec![
        ChainId::Names(name_shard(&NameKey::AppSlug(built.clone()))),
        ChainId::Names(name_shard(&NameKey::AppSlug(dead.clone()))),
        ChainId::Names(name_shard(&k)),
    ];
    chains.dedup();
    for c in &chains {
        let now = w.names(*c).now;
        run_batch(&mut w, *c, now);
    }
    let f = w.names_fold(NameKey::App(app));
    assert_eq!(f.app_owner(app), Some(owner), "RegisterApp finished");
    let f = w.names_fold(NameKey::Client(ClientId::for_app(app)));
    assert!(f.client(&ClientId::for_app(app)).is_some(), "RegisterClient finished");
    let az = w.authz(owner).fresh_leader();
    assert!(az.app_closed(orphan), "the orphan claim's App is closed");
    assert_eq!(w.names_fold(NameKey::AppSlug(dead.clone())).slug(&dead), None, "and its slug released");
    let home: NamesFold = w.names(ChainId::Names(account_shard(a))).fresh_leader();
    assert_eq!(home.account_login(a).map(|l| l.key()), Some(login("Dora").key()), "the login row");
}

/// Only the operator moves the cursor, it never moves back, and a release that reads only formats 1 and 2 answers
/// `Unsupported` to the cursor command instead of deciding it. The new events keep stable tags.
#[test]
fn the_audit_cursor_is_operator_only_monotone_and_format_3() {
    let mut w = World::default();
    let u = acct(0, 2, 3);
    let shard = shard_of_hash(&[1; 32]);
    let op = |through| TokensOp::AuditCursor { shard, through: Index(through) };
    let put = NamesOp::PutAccount { id: u, suspended: false, sessions_not_before: Timestamp(0) };
    w.names_cmd(ChainId::Names(account_shard(u)), NamesCmd { cred: OPS, op: put }).unwrap();
    assert!(matches!(
        w.tokens_cmd(TokensCmd { cred: as_user(u), op: op(5) }),
        Err(CommitError::Refused { err: graftyard_identity::TokensErr::Forbidden, .. })
    ));
    w.tokens_cmd(TokensCmd { cred: OPS, op: op(5) }).unwrap();
    let before = w.tokens(shard).committed_events(A).len();
    w.tokens_cmd(TokensCmd { cred: OPS, op: op(3) }).unwrap();
    assert_eq!(w.tokens(shard).committed_events(A).len(), before, "behind the cursor: no event");
    let t: TokensFold = w.tokens(shard).fresh_leader();
    assert_eq!(t.audit_cursor(), Index(5));
    let ns = account_shard(u);
    let nop = |through| NamesCmd { cred: OPS, op: NamesOp::AuditCursor { shard: ns, through: Index(through) } };
    w.names_cmd(ChainId::Names(ns), nop(7)).unwrap();
    let user = NamesCmd { cred: as_user(u), op: NamesOp::AuditCursor { shard: ns, through: Index(9) } };
    assert!(matches!(
        w.names_cmd(ChainId::Names(ns), user),
        Err(CommitError::Refused { err: graftyard_identity::NamesErr::Forbidden, .. })
    ));
    let f: NamesFold = w.names(ChainId::Names(ns)).fresh_leader();
    assert_eq!(f.audit_cursor(), Index(7));
    // A member of the previous release reads no format 3.
    let mut old: common::Group<TokensCmd> = common::Group::reading(
        ChainId::Tokens(shard),
        vec![graftyard_identity::TOKENS_FORMAT, graftyard_identity::TOKENS_FORMAT_V2],
        TokensFold::new(shard),
    );
    old.elect(A);
    assert!(matches!(
        old.commit(A, 1, TokensCmd { cred: OPS, op: op(1) }),
        Err(graftyard_raft::CallError::Unsupported(_))
    ));
    let mut v = Vec::new();
    graftyard_identity::TokensEvent::AuditCursor { through: Index(5) }.encode(&mut v);
    assert_eq!(v, [12, 0, 0, 0, 0, 0, 0, 0, 5]);
    let mut v = Vec::new();
    graftyard_identity::NamesEvent::AuditCursor { through: Index(7) }.encode(&mut v);
    assert_eq!(v, [13, 0, 0, 0, 0, 0, 0, 0, 7]);
}

/// A fold with M3 rows and no cursor keeps the bytes `main` (0e7456a) wrote, so a release with the cursor reads the
/// previous release's blobs and writes the same ones until the cursor first moves; a moved cursor round-trips.
#[test]
fn fold_blobs_without_a_cursor_keep_the_previous_releases_bytes() {
    use graftyard_raft::codec;
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let mut t = TokensFold::new(graftyard_types::TokenShard(0));
    t.more.verifying.insert(
        graftyard_identity::Kid(1),
        graftyard_identity::VerifyingKeyRow { key: graftyard_identity::Ed25519Public([3; 32]), retired: None },
    );
    t.more.jtis.insert(graftyard_identity::Jti(graftyard_types::Ulid(9)), Timestamp(77));
    let mut n = NamesFold::new(graftyard_types::NameShard(5));
    n.more.account_logins.insert(acct(0, 3, 0), login("Octo"));
    assert_eq!(
        hex(&codec::to_bytes(&t)),
        "0101000001000000011a30303030303030303030303030303030303030303030303030394d000101030303030303030303030303030303\
         03030303030303030303030303030303030000"
    );
    assert_eq!(hex(&codec::to_bytes(&n)), "0101050000000000000101808018044f63746f00000000000000");
    t.more.more = Index(41);
    n.more.more = Index(42);
    assert_eq!(codec::from_bytes::<TokensFold>(&codec::to_bytes(&t)).unwrap(), t);
    assert_eq!(codec::from_bytes::<NamesFold>(&codec::to_bytes(&n)).unwrap(), n);
    let _ = Owed::Login { account: acct(0, 3, 0), key: login("Octo").key() };
}

// ---- transfers and enterprise changes, from the destination's `Authz` log (format 0x12.4) ----

use graftyard_identity::audit::RepoRead;
use graftyard_identity::finish::{RepoLife, RepoStep};
use graftyard_identity::{AuthzFold, OrgRole, RepoEntry, RepoName, UserId};
use graftyard_types::{PlacementName, PlacementRecord, RepoId, SiteId};

/// The repository chain as the consumer sees it: its settings owner and enterprise and its group's sites, which
/// `Step::Repo` changes the way the site's move route and the transfer `Settings` (32.10) do.
#[derive(Clone)]
struct RepoChain {
    read: RepoRead,
    /// Every step performed, in order.
    steps: Vec<RepoStep>,
}

fn placed(w: &mut World, owner: AccountId, sites: [u8; 3]) -> PlacementRecord {
    let record = PlacementRecord {
        name: PlacementName::Tri,
        sites: sites.iter().map(|s| SiteId(*s)).collect(),
        home: SiteId(sites[0]),
        generation: 1,
        moving_from: None,
        at: Timestamp(1),
    };
    let op = NamesOp::InitPlacement { owner, record: record.clone() };
    w.names_cmd(ChainId::Names(account_shard(owner)), NamesCmd { cred: OPS, op }).unwrap();
    record
}

fn open_org(w: &mut World, o: AccountId, admin: AccountId, enterprise: Option<AccountId>) {
    use graftyard_identity::{AccountKind, IdBlock};
    w.authz_cmd(o, AuthzCmd { cred: OPS, op: AuthzOp::Open { kind: AccountKind::Org, enterprise } }).unwrap();
    let g = w.authz(o);
    let l = g.leader().unwrap();
    g.id_block(l, IdBlock::whole(0, 40 + o.0.0 as u32 % 50).unwrap()).unwrap();
    g.settle(&mut |c| panic!("{c}"));
    let set = AuthzOp::SetMember { user: UserId(admin), role: Some(OrgRole::Admin) };
    w.authz_cmd(o, AuthzCmd { cred: OPS, op: set }).unwrap();
}

/// One consumer batch over `owner`'s `Authz` log, the way the site runs it: entries a minute old from the chain's
/// cursor, each planned on fresh reads, `Step::Repo` performed on `repo`.
fn authz_batch(w: &mut World, owner: AccountId, repo: &mut RepoChain, skip_cursor: bool) -> Option<audit::Report> {
    let g = w.authz(owner);
    let l = g.leader().unwrap();
    let now = g.now;
    let m = g.m(l);
    let commit = m.state().commit.0 as usize;
    let log = m.log()[..commit].to_vec();
    let cursor = if skip_cursor { Index(0) } else { m.group_fold().fold.audit_cursor() };
    let mut owed = Vec::new();
    let scanned = audit::scan(
        cursor,
        Index(log.len() as u64),
        now,
        2_000,
        |i| log.get(i.0 as usize - 1).cloned(),
        |_, e| owed.extend(audit::authz_owed(owner, e)),
    )
    .unwrap();
    if !audit::needs_batch(cursor, scanned.through, &owed) {
        return None;
    }
    let chain = ChainId::Authz(owner);
    let mut f = Finisher::new(chain, now, cursor, scanned.through, owed);
    for _ in 0..200 {
        match f.want() {
            Want::Read(cs) => {
                for c in cs {
                    let fold = match c {
                        ChainId::Repo(_) => AuditRead::Repo(repo.read.clone()),
                        c => read(w, c),
                    };
                    f.read(c, fold);
                }
            }
            Want::Submit(Step::Repo(_, s)) => {
                match &s {
                    RepoStep::Move { to } => (repo.read.voters, repo.read.home) = (to.sites.to_vec(), to.home),
                    RepoStep::Settings { owner, enterprise } => {
                        (repo.read.owner, repo.read.enterprise) = (*owner, *enterprise)
                    }
                    RepoStep::Init { .. } | RepoStep::Delete { .. } => panic!("a transfer owes no {s:?}"),
                }
                repo.steps.push(s);
                f.submitted(false);
            }
            Want::Submit(s) => {
                let refused = submit(w, s);
                f.submitted(refused);
            }
            Want::Done(r) => return Some(r),
            Want::Stuck(o) => panic!("stuck on {o:?}"),
        }
    }
    panic!("the batch never finished");
}

/// BF-30's finisher half: the edge commits the transfer's `Reserved` name on the destination's `Authz` chain and
/// dies. A minute later the consumer, scanning only that chain's log, moves the group to the destination's
/// placement, records the transfer `Settings`, makes the name `Live`, leaves the old name an `Alias`, and moves
/// `RepoOwner`, in that order; then its cursor passes the entry, and a second batch owes nothing. Within and across
/// placements.
#[test]
fn audit_alone_finishes_a_transfer_whose_finisher_died_after_reserved() {
    for other_place in [true, false] {
        let mut w = World::default();
        let (alpha, beta, admin) = (acct(0, 9, 1), acct(0, 9, 2), acct(0, 2, 1));
        let (e1, e2) = (acct(0, 9, 5), acct(0, 9, 6));
        let put = NamesOp::PutAccount { id: admin, suspended: false, sessions_not_before: Timestamp(0) };
        w.names_cmd(ChainId::Names(account_shard(admin)), NamesCmd { cred: OPS, op: put }).unwrap();
        open_org(&mut w, alpha, admin, Some(e1));
        open_org(&mut w, beta, admin, Some(e2));
        let pa = placed(&mut w, alpha, [0, 1, 2]);
        let pb = placed(&mut w, beta, if other_place { [3, 1, 2] } else { [0, 1, 2] });
        let name = RepoName::parse("tool").unwrap();
        let create = AuthzCmd { cred: as_user(admin), op: AuthzOp::CreateRepo { name: name.clone() } };
        let graftyard_identity::AuthzOut::Repo { repo, .. } = w.authz_cmd(alpha, create).unwrap().out else { panic!() };
        w.names_cmd(
            ChainId::Names(name_shard(&NameKey::Repo(repo))),
            NamesCmd { cred: OPS, op: NamesOp::RepoOwner { repo, owner: alpha } },
        )
        .unwrap();
        let mut chain = RepoChain {
            read: RepoRead {
                owner: alpha,
                enterprise: Some(e1),
                voters: pa.sites.to_vec(),
                home: pa.home,
                life: RepoLife::Created,
            },
            steps: Vec::new(),
        };
        // The edge's commit point, then nothing: its isolate died.
        let reserve = AuthzOp::ReserveTransfer { name: name.clone(), repo, from: alpha };
        w.authz_cmd(beta, AuthzCmd { cred: as_user(admin), op: reserve }).unwrap();
        // Inside the minute nothing is due.
        assert_eq!(authz_batch(&mut w, beta, &mut chain, false), None);
        advance_all(&mut w, MINUTE + 1_000);
        let r = authz_batch(&mut w, beta, &mut chain, false).expect("a batch");
        assert_eq!(r.owed, 1);
        let moves = chain.steps.iter().filter(|s| matches!(s, RepoStep::Move { .. })).count();
        assert_eq!(moves, usize::from(other_place), "{:?}", chain.steps);
        assert_eq!(chain.steps.last(), Some(&RepoStep::Settings { owner: beta, enterprise: Some(e2) }));
        assert_eq!((chain.read.voters.clone(), chain.read.home), (pb.sites.to_vec(), pb.home));
        let fa: AuthzFold = w.authz(alpha).fresh_leader();
        let fb: AuthzFold = w.authz(beta).fresh_leader();
        assert!(matches!(fb.name(&name.key()), Some(RepoEntry::Live { repo: r, .. }) if r == repo));
        assert!(matches!(fa.name(&name.key()), Some(RepoEntry::Alias { repo: r, .. }) if r == repo));
        // `/repositories/{id}` renders under the owner `RepoOwner` names. Before its last step that is still the
        // source, which holds only the `Alias` by then: the id route still finds a name there (hosted, it answered
        // 404 in that window before `id_name`), and under the destination's `Live` name after.
        assert_eq!(fa.id_name(repo), Some(&name.key()));
        assert_eq!(fb.id_name(repo), Some(&name.key()));
        let ro: NamesFold = w.names(ChainId::Names(name_shard(&NameKey::Repo(repo)))).fresh_leader();
        assert_eq!(ro.repo_owner(repo), Some(beta));
        assert!(fb.audit_cursor() > Index(0), "the cursor moved past the reservation");
        // Run again: nothing is owed, nothing is submitted.
        let n = chain.steps.len();
        advance_all(&mut w, MINUTE + 1_000);
        assert_eq!(authz_batch(&mut w, beta, &mut chain, false).map(|r| r.steps).unwrap_or(0), 0);
        assert_eq!(chain.steps.len(), n);
    }
}

/// An org changing enterprise: the consumer gives each repository the org names `Live` the transfer `Settings` with
/// the new enterprise, and none twice.
#[test]
fn audit_finishes_an_enterprise_change_on_every_repository() {
    let mut w = World::default();
    let (org, admin, e1, e2) = (acct(0, 9, 1), acct(0, 2, 1), acct(0, 9, 5), acct(0, 9, 6));
    let put = NamesOp::PutAccount { id: admin, suspended: false, sessions_not_before: Timestamp(0) };
    w.names_cmd(ChainId::Names(account_shard(admin)), NamesCmd { cred: OPS, op: put }).unwrap();
    open_org(&mut w, org, admin, Some(e1));
    placed(&mut w, org, [0, 1, 2]);
    let mut repos = Vec::new();
    for n in ["a", "b"] {
        let create = AuthzCmd { cred: as_user(admin), op: AuthzOp::CreateRepo { name: RepoName::parse(n).unwrap() } };
        let graftyard_identity::AuthzOut::Repo { repo, .. } = w.authz_cmd(org, create).unwrap().out else { panic!() };
        repos.push(repo);
    }
    let set = AuthzOp::SetEnterprise { enterprise: Some(e2) };
    w.authz_cmd(org, AuthzCmd { cred: OPS, op: set }).unwrap();
    advance_all(&mut w, MINUTE + 1_000);
    // Both repositories share one fake chain here: each is read as its own, its `Settings` recorded by repository.
    let mut chains: std::collections::BTreeMap<RepoId, RepoRead> = repos
        .iter()
        .map(|r| {
            let life = RepoLife::Created;
            (*r, RepoRead { owner: org, enterprise: Some(e1), voters: vec![SiteId(0)], home: SiteId(0), life })
        })
        .collect();
    let g = w.authz(org);
    let l = g.leader().unwrap();
    let (now, log) = (g.now, g.m(l).log()[..g.m(l).state().commit.0 as usize].to_vec());
    let mut owed = Vec::new();
    let s = audit::scan(
        Index(0),
        Index(log.len() as u64),
        now,
        2_000,
        |i| log.get(i.0 as usize - 1).cloned(),
        |_, e| owed.extend(audit::authz_owed(org, e)),
    )
    .unwrap();
    assert_eq!(owed, vec![Owed::Enterprise { org }]);
    let mut f = Finisher::new(ChainId::Authz(org), now, Index(0), s.through, owed);
    let mut settings = 0;
    loop {
        match f.want() {
            Want::Read(cs) => {
                for c in cs {
                    let fold = match c {
                        ChainId::Repo(r) => AuditRead::Repo(chains[&r].clone()),
                        c => read(&mut w, c),
                    };
                    f.read(c, fold);
                }
            }
            Want::Submit(Step::Repo(r, RepoStep::Settings { owner, enterprise })) => {
                let c = chains.get_mut(&r).unwrap();
                (c.owner, c.enterprise) = (owner, enterprise);
                settings += 1;
                f.submitted(false);
            }
            Want::Submit(s) => {
                let refused = submit(&mut w, s);
                f.submitted(refused);
            }
            Want::Done(_) => break,
            Want::Stuck(o) => panic!("stuck on {o:?}"),
        }
    }
    assert_eq!(settings, 2);
    assert!(chains.values().all(|c| c.enterprise == Some(e2)));
}

/// The `Authz` cursor: only the operator moves it, never back, in format 0x12.4, which a release reading only 0x12.3
/// refuses; a fold whose cursor never moved keeps AM1-WP2's bytes.
#[test]
fn the_authz_audit_cursor_is_operator_only_monotone_and_format_4() {
    use graftyard_identity::{AUTHZ_FORMAT, AUTHZ_FORMAT_V2, AUTHZ_FORMAT_V3, AuthzEvent};
    let mut w = World::default();
    let (o, admin) = (acct(0, 9, 1), acct(0, 2, 1));
    let put = NamesOp::PutAccount { id: admin, suspended: false, sessions_not_before: Timestamp(0) };
    w.names_cmd(ChainId::Names(account_shard(admin)), NamesCmd { cred: OPS, op: put }).unwrap();
    open_org(&mut w, o, admin, None);
    let before: AuthzFold = w.authz(o).fresh_leader();
    let cur = |through| AuthzCmd { cred: OPS, op: AuthzOp::AuditCursor { through: Index(through) } };
    assert!(matches!(
        w.authz_cmd(o, AuthzCmd { cred: as_user(admin), op: AuthzOp::AuditCursor { through: Index(5) } }),
        Err(CommitError::Refused { err: graftyard_identity::AuthzErr::Forbidden, .. })
    ));
    w.authz_cmd(o, cur(5)).unwrap();
    let n = w.authz(o).committed_events(A).len();
    w.authz_cmd(o, cur(3)).unwrap();
    assert_eq!(w.authz(o).committed_events(A).len(), n, "behind the cursor: no event");
    let f: AuthzFold = w.authz(o).fresh_leader();
    assert_eq!(f.audit_cursor(), Index(5));
    // The cursor is a trailing part: without it the blob is the previous release's; with it, it round-trips.
    let mut no_cursor = f.clone();
    no_cursor.more.more.more = Index(0);
    assert_eq!(graftyard_raft::codec::to_bytes(&no_cursor), graftyard_raft::codec::to_bytes(&before));
    assert_eq!(graftyard_raft::codec::from_bytes::<AuthzFold>(&graftyard_raft::codec::to_bytes(&f)).unwrap(), f);
    let mut old: common::Group<AuthzCmd> = common::Group::reading(
        ChainId::Authz(o),
        vec![AUTHZ_FORMAT, AUTHZ_FORMAT_V2, AUTHZ_FORMAT_V3],
        AuthzFold::new(o),
    );
    old.elect(A);
    assert!(matches!(old.commit(A, 1, cur(1)), Err(graftyard_raft::CallError::Unsupported(_))));
    let mut v = Vec::new();
    AuthzEvent::AuditCursor { through: Index(5) }.encode(&mut v);
    assert_eq!(v, [16, 0, 0, 0, 0, 0, 0, 0, 5]);
    let mut v = Vec::new();
    cur(5).encode(&mut v);
    assert_eq!(v[v.len() - 9], 29, "AuthzOp tag 29");
}
