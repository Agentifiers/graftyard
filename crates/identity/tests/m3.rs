//! Milestone 3's identity on real `graftyard_raft::Member`s (`build/BUILD-PLAN.md`, M3-WP1 and M3-WP2, and F50):
//! App registration with its slug claim as the commit point and the `Audit` finisher, OAuth apps, every token kind on
//! the shard that is its uniqueness point, paired tokens, codes that are used once and lapse, and each account's login
//! on its `account_shard`. Dependencies are answered from folds the test reads through `fresh` on their own groups,
//! as the leader's host does once per entry.

mod common;

use common::{A, B, Group, SITES, acct, key, login};
use graftyard_identity::finish;
use graftyard_identity::{
    AccountKind, AppId, AppKey, AppRecord, AuthzCmd, AuthzErr, AuthzEvent, AuthzFold, AuthzOp, AuthzRead, ClientId,
    ClientRoute, CodeKind, DEVICE_CODE_TTL, Fingerprint, Jti, LoginRow, NAMES_FORMAT, NAMES_FORMAT_V2, NameKey,
    NamesCmd, NamesErr, NamesEvent, NamesFold, NamesOp, NamesOut, NamesRead, OAuthAppRecord, OpaqueToken,
    PermissionSet, RsaPublic, ScopeSet, SecretHash, Slug, SlugRow, TokenFields, TokenKind, TokensCmd, TokensErr,
    TokensEvent, TokensFold, TokensOp, TokensRead, UserId, account_shard, code_hash, draw_device_code, draw_secret_on,
    fingerprint_shard, jti_shard, name_shard, shard_of_hash, token_shard,
};
use graftyard_raft::{CommitError, Fold, Record, Scratch, codec};
use graftyard_types::{AccountId, ChainId, CredentialShard, DbId, Timestamp, TokenShard, Ulid};
use smallvec::smallvec;

use common::world::{FreshLeader, OPS, World};

fn refused<O: std::fmt::Debug, E: std::fmt::Debug>(r: Result<graftyard_raft::Acked<O>, CommitError<E>>) -> E {
    match r {
        Err(CommitError::Refused { err, .. }) => err,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn org() -> AccountId {
    acct(0, 9, 1)
}

/// The owner's `Authz` chain, opened as an org.
fn opened(w: &mut World, owner: AccountId) {
    let open = AuthzCmd { cred: OPS, op: AuthzOp::Open { kind: AccountKind::Org, enterprise: None } };
    w.authz_cmd(owner, open).unwrap();
}

fn record(app: AppId, slug: &Slug) -> AppRecord {
    AppRecord {
        id: app,
        slug: slug.clone(),
        name: "Octo CI".into(),
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

fn claim_slug(w: &mut World, slug: &Slug, owner: AccountId) -> AppId {
    match w.names_on(NameKey::AppSlug(slug.clone()), NamesOp::ClaimSlug { slug: slug.clone(), owner }) {
        Ok(a) => match a.out {
            NamesOut::App(app) => app,
            o => panic!("{o:?}"),
        },
        e => panic!("{e:?}"),
    }
}

/// Runs the `Audit` finisher's plan for a slug claim until it owes nothing.
fn finish_app(w: &mut World, slug: &Slug, record: Option<&AppRecord>) -> usize {
    let mut steps = 0;
    loop {
        let names = w.names_fold(NameKey::AppSlug(slug.clone()));
        let Some(claim) = names.slug(slug) else { return steps };
        let authz = w.authz(claim.owner).fresh_leader();
        let app_route = w.names_fold(NameKey::App(claim.app));
        let client_route = w.names_fold(NameKey::Client(ClientId::for_app(claim.app)));
        match finish::app_registration(slug, claim, &authz, &app_route, &client_route, record) {
            Some(s) => w.step(s),
            None => return steps,
        }
        steps += 1;
        assert!(steps < 10, "the finisher does not converge");
    }
}

// ---- M3-WP1: App registration ----

/// The slug claim is the commit point: a record for an unclaimed slug, or for a slug another owner claimed, is
/// refused on the owner's `Authz` leader, which reads the slug's shard through `fresh`.
#[test]
fn an_app_record_commits_only_after_its_slug_claim() {
    let mut w = World::default();
    let owner = org();
    opened(&mut w, owner);
    let slug = Slug::parse("Octo-CI").unwrap();
    assert_eq!(slug.as_str(), "octo-ci");
    // Planted bug 1's shape: the record before the claim.
    let early = record(AppId(DbId::pack(0, 5, 0).unwrap()), &slug);
    assert_eq!(
        refused(w.authz_cmd(owner, AuthzCmd { cred: OPS, op: AuthzOp::PutApp { record: Box::new(early) } })),
        AuthzErr::NotClaimed
    );
    let app = claim_slug(&mut w, &slug, owner);
    // Another owner's claim of the same slug, in any spelling, is refused, and so is its record.
    let rival = acct(0, 9, 2);
    opened(&mut w, rival);
    let again = Slug::parse("OCTO-ci").unwrap();
    assert_eq!(
        refused(w.names_on(NameKey::AppSlug(again.clone()), NamesOp::ClaimSlug { slug: again, owner: rival })),
        NamesErr::Taken { by: owner }
    );
    let theirs = AuthzCmd { cred: OPS, op: AuthzOp::PutApp { record: Box::new(record(app, &slug)) } };
    assert_eq!(refused(w.authz_cmd(rival, theirs)), AuthzErr::NotClaimed);
    // The owner's record commits, and routes follow; a route to an App with no record is refused.
    assert_eq!(refused(w.names_on(NameKey::App(app), NamesOp::RegisterApp { app, owner })), NamesErr::NotRegistered);
    w.authz_cmd(owner, AuthzCmd { cred: OPS, op: AuthzOp::PutApp { record: Box::new(record(app, &slug)) } }).unwrap();
    assert_eq!(finish_app(&mut w, &slug, None), 2, "RegisterApp and RegisterClient were owed");
    let a = w.authz(owner).fresh_leader();
    assert_eq!(a.app(app).map(|r| r.client), Some(ClientId::for_app(app)));
    assert_eq!(w.names_fold(NameKey::App(app)).app_owner(app), Some(owner));
    let client = ClientId::for_app(app);
    assert_eq!(w.names_fold(NameKey::Client(client)).client(&client), Some(ClientRoute { owner, app: Some(app) }));
    // Every member of the slug's group holds the claim.
    let g = w.names(ChainId::Names(name_shard(&NameKey::AppSlug(slug.clone()))));
    g.settle(&mut |c| panic!("{c}"));
    for s in SITES {
        assert_eq!(g.m(s).group_fold().fold.slug(&slug), Some(SlugRow { app, owner }), "{s:?}");
    }
}

/// An edge that claimed the slug and died before the record: `Audit` finishes it from the record if it has one, and
/// otherwise closes the App and releases the slug, so the slug is free again and the id is never reused.
#[test]
fn audit_finishes_or_aborts_a_registration_nobody_finished() {
    let mut w = World::default();
    let owner = org();
    opened(&mut w, owner);
    // Finished: the claim and the record committed; the routes did not.
    let s1 = Slug::parse("builder").unwrap();
    let a1 = claim_slug(&mut w, &s1, owner);
    w.authz_cmd(owner, AuthzCmd { cred: OPS, op: AuthzOp::PutApp { record: Box::new(record(a1, &s1)) } }).unwrap();
    assert_eq!(finish_app(&mut w, &s1, None), 2);
    assert_eq!(finish_app(&mut w, &s1, None), 0, "a second pass owes nothing");
    // Finished from the record: only the claim committed, and the finisher holds the record the edge sent. It
    // writes the record first; a route before it would lead to no App.
    let s0 = Slug::parse("from-record").unwrap();
    let a0 = claim_slug(&mut w, &s0, owner);
    assert_eq!(finish_app(&mut w, &s0, Some(&record(a0, &s0))), 3, "PutApp, RegisterApp, RegisterClient");
    assert_eq!(w.authz(owner).fresh_leader().app(a0), Some(&record(a0, &s0)));
    // A record for another slug's App is not this claim's: the finisher aborts instead.
    let sx = Slug::parse("mismatch").unwrap();
    let ax = claim_slug(&mut w, &sx, owner);
    assert_eq!(finish_app(&mut w, &sx, Some(&record(a0, &s0))), 2, "CloseApp, then ReleaseSlug");
    assert!(w.authz(owner).fresh_leader().app_closed(ax));
    // Aborted: only the claim committed, and the finisher has no record.
    let s2 = Slug::parse("orphan").unwrap();
    let a2 = claim_slug(&mut w, &s2, owner);
    assert_eq!(finish_app(&mut w, &s2, None), 2, "CloseApp, then ReleaseSlug");
    assert_eq!(w.names_fold(NameKey::AppSlug(s2.clone())).slug(&s2), None, "the slug is free");
    assert!(w.authz(owner).fresh_leader().app_closed(a2));
    // The closed id is never registered again, even by an edge that wakes up with its record.
    let late = AuthzCmd { cred: OPS, op: AuthzOp::PutApp { record: Box::new(record(a2, &s2)) } };
    assert_eq!(refused(w.authz_cmd(owner, late)), AuthzErr::NotClaimed);
    // A new claim of the freed slug takes a new id.
    let a3 = claim_slug(&mut w, &s2, owner);
    assert_ne!(a3, a2);
    // A live registration's slug is never released.
    let rel = NamesOp::ReleaseSlug { slug: s1.clone(), app: a1, owner };
    assert_eq!(refused(w.names_on(NameKey::AppSlug(s1.clone()), rel)), NamesErr::Registered);
}

/// A user registers only for an owner it administers: the leader reads the owner's `Authz` through `fresh`.
#[test]
fn only_an_admin_of_the_owner_claims_a_slug() {
    let mut w = World::default();
    let owner = org();
    opened(&mut w, owner);
    let admin = acct(0, 9, 3);
    let member = acct(0, 9, 4);
    for (u, role) in [(admin, graftyard_identity::OrgRole::Admin), (member, graftyard_identity::OrgRole::Member)] {
        let op = AuthzOp::SetMember { user: UserId(u), role: Some(role) };
        w.authz_cmd(owner, AuthzCmd { cred: OPS, op }).unwrap();
    }
    for u in [admin, member] {
        let chain = ChainId::Names(account_shard(u));
        w.names_cmd(
            chain,
            NamesCmd {
                cred: OPS,
                op: NamesOp::PutAccount { id: u, suspended: false, sessions_not_before: Timestamp(0) },
            },
        )
        .unwrap();
    }
    let slug = Slug::parse("team-bot").unwrap();
    let chain = ChainId::Names(name_shard(&NameKey::AppSlug(slug.clone())));
    let by = |u: AccountId| NamesCmd {
        cred: CredentialShard { account: Some(u), key: None },
        op: NamesOp::ClaimSlug { slug: slug.clone(), owner },
    };
    assert_eq!(refused(w.names_cmd(chain, by(member))), NamesErr::Forbidden);
    assert!(w.names_cmd(chain, by(admin)).is_ok());
}

/// An OAuth app's commit point is its client id's route; its record follows, and secrets are stored only as hashes.
#[test]
fn an_oauth_app_registers_through_its_client_id() {
    let mut w = World::default();
    let owner = org();
    opened(&mut w, owner);
    let client = ClientId::parse("0123456789abcdef0123").unwrap();
    assert!(ClientId::parse("0123456789abcdef01234").is_err(), "21 characters");
    let rec = OAuthAppRecord {
        client,
        name: "deploy".into(),
        callbacks: smallvec![],
        client_secrets: smallvec![SecretHash([1; 32])],
        device_flow: true,
    };
    let put = || AuthzCmd { cred: OPS, op: AuthzOp::PutOAuthApp { record: rec.clone() } };
    assert_eq!(refused(w.authz_cmd(owner, put())), AuthzErr::NotClaimed);
    w.names_on(NameKey::Client(client), NamesOp::RegisterClient { client, owner, app: None }).unwrap();
    w.authz_cmd(owner, put()).unwrap();
    assert_eq!(w.authz(owner).fresh_leader().oauth_app(&client), Some(&rec));
    // An App-form client id is never an OAuth app's, and an OAuth app's never names an App.
    let app_form = ClientId::for_app(AppId(DbId(5)));
    assert_eq!(
        refused(w.names_on(NameKey::Client(app_form), NamesOp::RegisterClient { client: app_form, owner, app: None })),
        NamesErr::BadClient
    );
}

// ---- M3-WP2: token kinds and their shards ----

fn fp(n: u8) -> Fingerprint {
    Fingerprint([n; 32])
}

/// Each credential row lands on the shard that is its uniqueness point, and a row routed to another shard is refused
/// by the leader and by a follower's check.
#[test]
fn every_credential_kind_is_refused_off_its_shard() {
    let mut w = World::default();
    let user = acct(0, 2, 5);
    let jti = Jti(Ulid(0x0123_4567_89ab_cdef));
    assert_eq!(TokensCmd { cred: OPS, op: TokensOp::RevokeJti { jti, exp: Timestamp(1) } }.shard(), jti_shard(&jti));
    assert_ne!(jti_shard(&jti), shard_of_hash(&[0; 32]), "jti and fingerprint hash under their own tags");
    assert_ne!(fingerprint_shard(&fp(3)), shard_of_hash(&fp(3).0));
    // An SSH key lands on its fingerprint's shard; a fold for another shard refuses it.
    let add = TokensCmd { cred: OPS, op: TokensOp::AddSshKey { fingerprint: fp(3), account: user, repo: None } };
    assert_eq!(add.shard(), fingerprint_shard(&fp(3)));
    let wrong = TokenShard((fingerprint_shard(&fp(3)).0 + 1) % 3072);
    let mut f = TokensFold::new(wrong);
    let r = common::entry(&mut f, std::slice::from_ref(&add), &[]);
    assert_eq!(r, [Err(TokensErr::WrongShard { want: fingerprint_shard(&fp(3)) })]);
    let ev = TokensEvent::SshKey { fingerprint: fp(3), account: user, repo: None };
    assert!(f.scratch().check(&ev).is_err(), "a follower refuses a fingerprint off its shard");
    let ev = TokensEvent::JtiRevoked { jti, exp: Timestamp(9) };
    assert!(TokensFold::new(TokenShard((jti_shard(&jti).0 + 1) % 3072)).scratch().check(&ev).is_err());
    // On its shard, one key has one owner: two edges adding it at once, in one entry, get one owner.
    w.tokens_cmd(add.clone()).unwrap();
    let other =
        TokensCmd { cred: OPS, op: TokensOp::AddSshKey { fingerprint: fp(3), account: acct(0, 2, 6), repo: None } };
    assert_eq!(refused(w.tokens_cmd(other)), TokensErr::KeyTaken { by: user });
    // Verifying keys live on shard 0 only.
    let vk = TokensCmd {
        cred: OPS,
        op: TokensOp::PutVerifyingKey {
            kid: graftyard_identity::Kid(1),
            key: graftyard_identity::Ed25519Public([2; 32]),
            retired: None,
        },
    };
    assert_eq!(vk.shard(), TokenShard(0));
}

/// A `ghu_` and its `ghr_` commit in one entry on one shard, and revoking either revokes both.
#[test]
fn a_paired_token_revokes_with_its_partner() {
    let mut w = World::default();
    let user = acct(0, 2, 5);
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    let mut rand = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let app = AppId(DbId(77));
    let fields = TokenFields { app: Some(app), client: Some(ClientId::for_app(app)), ..TokenFields::default() };
    // Two pairs: the first is revoked through its `ghr_`, the second through its `ghu_`.
    let mut last = None;
    for revoke_refresh in [true, false] {
        let u = OpaqueToken::new(TokenKind::UserToServer, draw_one_secret(&mut rand)).unwrap();
        let (rs, draws) = draw_secret_on(&mut rand, u.shard(), 100_000).expect("lands");
        assert!(draws > 1, "{draws} draws");
        let r = OpaqueToken::new(TokenKind::Refresh, rs).unwrap();
        assert_eq!(token_shard(&rs), u.shard());
        let pair = TokensOp::StorePair {
            user: u.hash(),
            refresh: r.hash(),
            account: user,
            scopes: ScopeSet(0),
            user_expires: Some(Timestamp(8 * 3_600_000)),
            refresh_expires: None,
            fields: fields.clone(),
        };
        w.tokens_cmd(TokensCmd { cred: OPS, op: pair }).unwrap();
        let (revoke, other) = if revoke_refresh { (r.hash(), u.hash()) } else { (u.hash(), r.hash()) };
        let f = w.tokens(u.shard()).fresh_leader();
        assert!(!f.token(&other).unwrap().revoked);
        assert_eq!(f.token(&r.hash()).unwrap().kind, TokenKind::Refresh);
        w.tokens_cmd(TokensCmd { cred: OPS, op: TokensOp::Revoke { hash: revoke } }).unwrap();
        let f = w.tokens(u.shard()).fresh_leader();
        assert!(f.token(&other).unwrap().revoked, "revoking either revokes both");
        assert_eq!(f.fields(&other).and_then(|x| x.paired), Some(revoke));
        // Revoking the other one now is refused as already revoked.
        assert_eq!(
            refused(w.tokens_cmd(TokensCmd { cred: OPS, op: TokensOp::Revoke { hash: other } })),
            TokensErr::AlreadyRevoked
        );
        last = Some(u);
    }
    let u = last.unwrap();
    // A `ghr_` whose hash is on another shard than its `ghu_` is refused.
    let far = OpaqueToken::new(TokenKind::Refresh, *b"ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ").unwrap();
    if far.shard() != u.shard() {
        let pair = TokensOp::StorePair {
            user: OpaqueToken::new(TokenKind::UserToServer, *b"YYYYYYYYYYYYYYYYYYYYYYYYYYYYYY").unwrap().hash(),
            refresh: far.hash(),
            account: user,
            scopes: ScopeSet(0),
            user_expires: None,
            refresh_expires: None,
            fields,
        };
        assert!(matches!(refused(w.tokens_cmd(TokensCmd { cred: OPS, op: pair })), TokensErr::WrongShard { .. }));
    }
}

fn draw_one_secret(rand: &mut impl FnMut() -> u64) -> [u8; 30] {
    let (s, _) = graftyard_identity::draw_secret(rand, graftyard_identity::Family::Na).unwrap();
    s
}

/// A fine-grained PAT carries its resource owner and repositories; fields a kind may not carry are refused.
#[test]
fn optional_fields_fit_their_kind() {
    let mut f = TokensFold::default();
    let user = acct(0, 2, 5);
    let store = |kind, fields| TokensCmd {
        cred: OPS,
        op: TokensOp::StoreWith {
            hash: [kind as u8 + 1; 32],
            kind,
            account: user,
            scopes: ScopeSet(0),
            expires: None,
            fields,
        },
    };
    let pat = TokenFields {
        resource_owner: Some(org()),
        repos: Some(graftyard_identity::RepoScope::Selected(smallvec![graftyard_types::RepoId(DbId(9))])),
        permissions: Some(PermissionSet::default()),
        ..TokenFields::default()
    };
    let r = common::entry(
        &mut f,
        &[
            store(TokenKind::FineGrained, pat.clone()),
            store(TokenKind::Pat, pat.clone()),
            store(TokenKind::FineGrained, TokenFields::default()),
            store(TokenKind::OAuth, TokenFields { app: Some(AppId(DbId(1))), ..TokenFields::default() }),
            store(TokenKind::Refresh, TokenFields::default()),
        ],
        &[],
    );
    assert_eq!(
        r,
        [
            Ok(()),
            Err(TokensErr::BadFields),
            Err(TokensErr::BadFields),
            Err(TokensErr::BadFields),
            Err(TokensErr::BadFields)
        ]
    );
    assert_eq!(f.fields(&[TokenKind::FineGrained as u8 + 1; 32]), Some(&pat));
}

// ---- M3-WP2: codes ----

/// A device flow on real members: the device code is redrawn onto its user code's shard, polling is pending until
/// approval, the exchange spends the code once, and a second exchange, a replay after a leader change, and an
/// expired code all get nothing.
#[test]
fn a_device_code_yields_one_token_and_lapses_at_900_s() {
    let mut w = World::default();
    let user = acct(0, 2, 5);
    let client = ClientId::parse("178c6fc778ccc68e1d6a").unwrap();
    let user_code = b"WDJB-MJHT";
    let mut x = 7u64;
    let rand = move || {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        x
    };
    let (device, _) = draw_device_code(rand, user_code, 100_000).expect("lands");
    let (dh, uh) = (code_hash(&device), code_hash(user_code));
    assert_eq!(shard_of_hash(&dh), shard_of_hash(&uh));
    let put = TokensOp::PutDeviceCode { device: dh, user: uh, client, scopes: ScopeSet(1), interval: 5 };
    w.tokens_cmd(TokensCmd { cred: OPS, op: put }).unwrap();
    let use_code = || TokensCmd { cred: OPS, op: TokensOp::UseCode { hash: dh, client: Some(client) } };
    assert_eq!(refused(w.tokens_cmd(use_code())), TokensErr::Pending, "authorization_pending");
    // The user code is not a device code.
    let wrong = TokensCmd { cred: OPS, op: TokensOp::UseCode { hash: uh, client: Some(client) } };
    assert_eq!(refused(w.tokens_cmd(wrong)), TokensErr::WrongCode);
    let approve = TokensCmd { cred: OPS, op: TokensOp::ApproveCode { user: uh, account: user } };
    w.tokens_cmd(approve.clone()).unwrap();
    assert_eq!(refused(w.tokens_cmd(approve)), TokensErr::AlreadyApproved);
    // The exchange spends the code; a second one, in the same entry or later, gets nothing.
    let shard = shard_of_hash(&dh);
    let g = w.tokens(shard);
    let leader = g.leader().unwrap();
    let t1 = g.commit(leader, 501, use_code()).unwrap().ticket;
    let t2 = g.commit(leader, 502, use_code()).unwrap().ticket;
    g.settle(&mut |c| panic!("{c}"));
    assert!(matches!(g.result(t1), Some(Ok(_))));
    assert!(matches!(g.result(t2), Some(Err(CommitError::Refused { err: TokensErr::CodeUsed, .. }))));
    // A leader change: the new leader still refuses a replay under a new request id.
    let old = g.leader().unwrap();
    g.crash(old);
    g.elect_after_promises(if old == A { B } else { A });
    g.restart(old);
    assert_eq!(refused(w.tokens_cmd(use_code())), TokensErr::CodeUsed);
    // The original request id re-sent gets its stored outcome, not a second token.
    let g = w.tokens(shard);
    let l = g.leader().unwrap();
    let t = g.commit(l, 501, use_code()).unwrap().ticket;
    g.settle(&mut |c| panic!("{c}"));
    assert!(matches!(g.result(t), Some(Ok(_))), "a duplicate replays its first outcome");
    // A fresh device code, approved, but used after 900 s: expired.
    let (dev2, uc2) = (code_hash(b"dev-2"), code_hash(b"user-2"));
    let (dev2, uc2) = if shard_of_hash(&dev2) == shard_of_hash(&uc2) { (dev2, uc2) } else { pick_pair() };
    let put = TokensOp::PutDeviceCode { device: dev2, user: uc2, client, scopes: ScopeSet(1), interval: 5 };
    w.tokens_cmd(TokensCmd { cred: OPS, op: put }).unwrap();
    w.tokens_cmd(TokensCmd { cred: OPS, op: TokensOp::ApproveCode { user: uc2, account: user } }).unwrap();
    let g = w.tokens(shard_of_hash(&dev2));
    g.advance(DEVICE_CODE_TTL.as_millis() as u64 + 1);
    let late = TokensCmd { cred: OPS, op: TokensOp::UseCode { hash: dev2, client: Some(client) } };
    assert_eq!(refused(w.tokens_cmd(late)), TokensErr::NoCode, "expired_token");
    // The lapsed rows are pruned on every member once an entry past their time applies.
    let g = w.tokens(shard_of_hash(&dev2));
    g.settle(&mut |c| panic!("{c}"));
    let l = g.leader().unwrap();
    assert!(!g.m(l).group_fold().fold.more.codes.contains_key(&dev2));
}

/// Two code hashes on one shard.
fn pick_pair() -> ([u8; 32], [u8; 32]) {
    let base = code_hash(b"user-2");
    let s = shard_of_hash(&base);
    let dev =
        (0..1_000_000u32).map(|i| code_hash(format!("dev-{i}").as_bytes())).find(|h| shard_of_hash(h) == s).unwrap();
    (dev, base)
}

/// A follower refuses a second `CodeUsed` for one code, so a leader bug cannot spend a code twice.
#[test]
fn a_follower_refuses_a_code_used_twice() {
    let client = ClientId::parse("abc").unwrap();
    let h = code_hash(b"auth");
    let mut f = TokensFold::new(shard_of_hash(&h));
    let put = TokensCmd {
        cred: OPS,
        op: TokensOp::PutCode {
            hash: h,
            kind: CodeKind::Authorization { redirect: graftyard_identity::Url("http://localhost/".into()), pkce: None },
            client: Some(client),
            scopes: ScopeSet(0),
            account: Some(acct(0, 2, 5)),
        },
    };
    // `entry` decides on `scratch()`, whose time is 0, so a code it stores lapses at the TTL.
    common::entry(&mut f, &[put], &[]);
    let at = Timestamp(1);
    let mut s = f.scratch_at(at, graftyard_raft::TierSet::CORE);
    assert!(s.check(&TokensEvent::CodeUsed { hash: h }).is_ok());
    s.apply(&TokensEvent::CodeUsed { hash: h });
    assert!(s.check(&TokensEvent::CodeUsed { hash: h }).is_err());
    // Past its time it is refused too.
    let s = f.scratch_at(Timestamp(600_001), graftyard_raft::TierSet::CORE);
    assert!(s.check(&TokensEvent::CodeUsed { hash: h }).is_err());
}

/// A revoked `jti` binds until its `exp` and then lapses with no command.
#[test]
fn a_revoked_jti_lapses_at_exp() {
    let mut w = World::default();
    let jti = Jti(Ulid(42));
    let g = w.tokens(jti_shard(&jti));
    let exp = Timestamp(g.now.0 + 60_000);
    w.tokens_cmd(TokensCmd { cred: OPS, op: TokensOp::RevokeJti { jti, exp } }).unwrap();
    let g = w.tokens(jti_shard(&jti));
    let f = g.fresh_leader();
    assert!(f.jti_revoked(&jti, Timestamp(exp.0 - 1)));
    assert!(!f.jti_revoked(&jti, exp));
    // Only an operator (the edge, after it verified the token) records one; a signed-in user's command is refused.
    let user = acct(0, 2, 5);
    let put = NamesOp::PutAccount { id: user, suspended: false, sessions_not_before: Timestamp(0) };
    w.names_cmd(ChainId::Names(account_shard(user)), NamesCmd { cred: OPS, op: put }).unwrap();
    let by_user = TokensCmd {
        cred: CredentialShard { account: Some(user), key: None },
        op: TokensOp::RevokeJti { jti: Jti(Ulid(43)), exp },
    };
    assert_eq!(refused(w.tokens_cmd(by_user)), TokensErr::Forbidden);
}

// ---- F50: an account's login on its `account_shard` ----

fn signup(w: &mut World, l: &str) -> AccountId {
    let k = NameKey::Login(key(l));
    match w.names_on(k, NamesOp::ClaimLogin { login: login(l), kind: AccountKind::User, account: None }) {
        Ok(a) => match a.out {
            NamesOut::Account(id) => id,
            o => panic!("{o:?}"),
        },
        e => panic!("{e:?}"),
    }
}

/// A signup's login reaches its `account_shard` (the edge's write or the finisher's), the edge's read by id renders
/// it, and a rename moves it; a stale write of the old login is refused because the leader reads the login's shard.
#[test]
fn an_accounts_login_follows_its_claim_and_its_rename() {
    let mut w = World::default();
    let a = signup(&mut w, "Mona-Lisa");
    let home = ChainId::Names(account_shard(a));
    let set = |l: &str| NamesCmd { cred: OPS, op: NamesOp::SetAccountLogin { account: a, login: login(l) } };
    // Before the row, the finisher owes it.
    let login_shard = w.names_fold(NameKey::Login(key("mona-lisa")));
    let acct_fold = w.names(home).fresh_leader();
    let step = finish::account_login(a, &login("Mona-Lisa"), &login_shard, &acct_fold).expect("owed");
    w.step(step);
    let acct_fold = w.names(home).fresh_leader();
    assert_eq!(acct_fold.account_login(a).map(|l| l.as_str()), Some("Mona-Lisa"), "the owner's spelling");
    assert!(finish::account_login(a, &login("Mona-Lisa"), &login_shard, &acct_fold).is_none());
    // A login the account does not hold is refused.
    assert_eq!(refused(w.names_cmd(home, set("someone-else"))), NamesErr::NotHolder);
    // Rename: claim the new login, release the old, then record the new.
    let k2 = NameKey::Login(key("mona"));
    w.names_on(k2, NamesOp::ClaimLogin { login: login("mona"), kind: AccountKind::User, account: Some(a) }).unwrap();
    w.names_on(NameKey::Login(key("mona-lisa")), NamesOp::ReleaseLogin { key: key("mona-lisa"), account: a }).unwrap();
    w.names_cmd(home, set("mona")).unwrap();
    // A delayed write of the old login, after the rename, is refused: the old login is a former name now.
    assert_eq!(refused(w.names_cmd(home, set("Mona-Lisa"))), NamesErr::NotHolder);
    let f = w.names(home).fresh_leader();
    assert_eq!(f.account_login(a).map(|l| l.as_str()), Some("mona"));
    // The row is in Milestone 3's trailing part, which an older release's fold blob never had.
    assert!(!f.more.account_logins.is_empty());
    let bytes = codec::to_bytes(&f);
    assert_eq!(codec::from_bytes::<NamesFold>(&bytes).unwrap(), f);
}

/// A fold blob from Milestone 1 (no trailing part) still decodes, and one with no Milestone 3 row is byte for byte
/// what Milestone 1 wrote, so a release that reads both formats and a release that reads one share every blob until
/// Milestone 3's formats are enabled. The pinned bytes were printed by `main` at 5e364cf.
#[test]
fn fold_blobs_without_milestone_3_rows_keep_milestone_1_bytes() {
    let names_m1 = hex(
        "010002046f63746f0080801800036f6c64018080180180801800050180801800030001020001000101128080180180801880800401",
    );
    let f: NamesFold = codec::from_bytes(&names_m1).unwrap();
    assert_eq!(codec::to_bytes(&f), names_m1);
    assert_eq!(f.login(&key("octo")), Some(LoginRow::Claimed { account: acct(0, 3, 0), kind: AccountKind::User }));
    let authz_m1 = hex("010180801801010182801801011282801802010172011201010000");
    let a: AuthzFold = codec::from_bytes(&authz_m1).unwrap();
    assert_eq!(codec::to_bytes(&a), authz_m1);
    let tokens_m1 = hex("01010b01c873d13f243f9a9664e14b10e23600710e4cea56f53e9a1089891720e1a6bb170080801803016300");
    let t: TokensFold = codec::from_bytes(&tokens_m1).unwrap();
    assert_eq!(codec::to_bytes(&t), tokens_m1);
    // With a Milestone 3 row the blob grows a marker and the part; an older blob with a bad marker is refused.
    let mut f2 = f.clone();
    f2.more.account_logins.insert(acct(0, 3, 0), login("Octo"));
    let b2 = codec::to_bytes(&f2);
    assert_eq!(&b2[..names_m1.len()], &names_m1[..]);
    assert_eq!(codec::from_bytes::<NamesFold>(&b2).unwrap(), f2);
    let mut bad = names_m1.clone();
    bad.push(2);
    assert!(codec::from_bytes::<NamesFold>(&bad).is_err());
    // And the M1 events keep their canonical bytes: the digests of stored entries do not move.
    let mut v = Vec::new();
    NamesEvent::Account { id: acct(0, 3, 0), suspended: true, sessions_not_before: Timestamp(5) }.encode(&mut v);
    assert_eq!(v, hex("010000000000030000010000000000000005"));
    let mut v = Vec::new();
    AuthzEvent::Member { user: UserId(acct(0, 3, 1)), role: None }.encode(&mut v);
    assert_eq!(v, hex("01000000000003000100"));
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// A Milestone 1 command still decides in format 1, and a release that reads only format 1 answers `Unsupported` to
/// a Milestone 3 command instead of deciding it.
#[test]
fn milestone_1_commands_keep_format_1_and_a_format_1_member_refuses_format_2() {
    let user = acct(0, 2, 5);
    let m1 = NamesCmd {
        cred: OPS,
        op: NamesOp::PutAccount { id: user, suspended: false, sessions_not_before: Timestamp(0) },
    };
    assert_eq!(graftyard_raft::Command::format(&m1), NAMES_FORMAT);
    let m3 = NamesCmd { cred: OPS, op: NamesOp::SetAccountLogin { account: user, login: login("x") } };
    assert_eq!(graftyard_raft::Command::format(&m3), NAMES_FORMAT_V2);
    let shard = account_shard(user);
    let mut g: Group<NamesCmd> = Group::new(ChainId::Names(shard), NAMES_FORMAT, NamesFold::new(shard));
    g.elect(A);
    assert!(matches!(g.commit(A, 1, m3), Err(graftyard_raft::CallError::Unsupported(_))));
    assert!(g.commit(A, 2, m1).is_ok());
}
