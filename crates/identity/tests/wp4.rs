//! M3-WP4's gates on real `graftyard_raft::Member`s (`build/BUILD-PLAN.md`, M3-WP4): BF-27 (a `jti` revoked with
//! 1,000 commands queued under its token), BF-32 (an App JWT from Octokit's signer verified against `AppRecord.keys`,
//! an OAuth app's wrong secret, and no client secret on any `Identity` chain), key rotation with an overlap window
//! on `TokenShard(0)`, and the installation rows on the owner's `Authz` chain that `authorize` intersects with the
//! claims. Every dependency is read through `fresh` on its own group, as the leader's host does once per entry.

mod common;

use common::world::{FreshLeader, OPS, World};
use common::{A, B, Group, acct};
use graftyard_identity::appauth::{AppJwtError, client_secret_ok, secret_hash, verify_app_jwt};
use graftyard_identity::stateless::{
    self, AccessTokenRequest, Grant, INSTALLATION_TOKEN_TTL, InstallationClaims, KeyEpoch, KeyError, KeySource, Minter,
    NarrowError, RotationStep, SIGNING_KEY_CACHE, SigningSecret, StatelessError, TokenRequestError, access_token_grant,
    revoke_command, rotation_step,
};
use graftyard_identity::{
    Access, AccountCapture, AccountKind, AppId, AppKey, AppRecord, AuthzCmd, AuthzErr, AuthzFold, AuthzOp, AuthzOut,
    AuthzRead, ClientId, CredentialRefusal, Decision, IdBlock, IdentityCmd, IdentityOp, InstallationId,
    InstallationRow, Kid, Login, NameKey, NamesOp, OAuthAppRecord, PermissionSet, Principal, RepoAccess, RepoName,
    RepoPermission, RepoScope, RsaPublic, Slug, TokensCmd, TokensErr, TokensOp, TokensRead, UserId, Visibility,
    WriteAction, authorize_write, jti_shard,
};
use graftyard_raft::{CommitError, Record, codec};
use graftyard_types::{AccountId, CredentialKey, CredentialShard, DbId, RepoId, Timestamp, TokenShard};
use smallvec::smallvec;

fn refused<O: std::fmt::Debug, E: std::fmt::Debug>(r: Result<graftyard_raft::Acked<O>, CommitError<E>>) -> E {
    match r {
        Err(CommitError::Refused { err, .. }) => err,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn ms(d: std::time::Duration) -> u64 {
    d.as_millis() as u64
}

fn perms(p: &[(&str, Access)]) -> PermissionSet {
    PermissionSet(p.iter().map(|(k, a)| (Box::from(*k), *a)).collect())
}

/// The fixture `universal-github-app-jwt` 2.2.2 made, the signer `@octokit/app` uses: three App JWTs and the public
/// half of the throwaway key that signed the first and third.
struct Fixture {
    now: Timestamp,
    app: AppId,
    public: Vec<u8>,
    jwt: String,
    jwt_other_key: String,
    jwt_other_app: String,
}

fn fixture() -> Fixture {
    use base64::Engine as _;
    let v: serde_json::Value = serde_json::from_str(include_str!("fixtures/app-jwt.json")).unwrap();
    let s = |k: &str| v[k].as_str().unwrap().to_owned();
    Fixture {
        now: Timestamp(v["now"].as_u64().unwrap() * 1000),
        app: AppId(DbId(v["app_id"].as_i64().unwrap())),
        public: base64::engine::general_purpose::STANDARD.decode(s("public_der_b64")).unwrap(),
        jwt: s("jwt"),
        jwt_other_key: s("jwt_other_key"),
        jwt_other_app: s("jwt_other_app"),
    }
}

fn app_record(app: AppId, public: &[u8], permissions: PermissionSet, secret: &str) -> AppRecord {
    AppRecord {
        id: app,
        slug: Slug::parse("octo-ci").unwrap(),
        name: "Octo CI".into(),
        client: ClientId::for_app(app),
        keys: smallvec![AppKey { id: DbId(1), public: RsaPublic(public.into()), created: Timestamp(5) }],
        permissions,
        events: smallvec![],
        webhook: None,
        webhook_secret: None,
        client_secrets: smallvec![secret_hash(secret)],
        callbacks: smallvec![],
        public: true,
    }
}

/// An App registered on `owner` the way M3-WP1 does it: the slug claim, then the record, then the routes.
fn register_app(w: &mut World, owner: AccountId, record: &AppRecord) {
    let slug = record.slug.clone();
    let out = w.names_on(NameKey::AppSlug(slug.clone()), NamesOp::ClaimSlug { slug, owner }).unwrap().out;
    assert_eq!(out, graftyard_identity::NamesOut::App(record.id), "the fixture's App id is the slug shard's first id");
    let put = AuthzCmd { cred: OPS, op: AuthzOp::PutApp { record: Box::new(record.clone()) } };
    w.authz_cmd(owner, put).unwrap();
    w.names_on(NameKey::App(record.id), NamesOp::RegisterApp { app: record.id, owner }).unwrap();
}

fn open(w: &mut World, owner: AccountId, kind: AccountKind) {
    w.authz_cmd(owner, AuthzCmd { cred: OPS, op: AuthzOp::Open { kind, enterprise: None } }).unwrap();
    let g = w.authz(owner);
    let l = g.leader().unwrap();
    g.id_block(l, IdBlock::whole(0, 50 + (owner.0.0 % 7) as u32).unwrap()).unwrap();
    g.settle(&mut |c| panic!("{c}"));
}

fn repo(w: &mut World, owner: AccountId, name: &str) -> RepoId {
    let op = AuthzOp::CreateRepo { name: RepoName::parse(name).unwrap() };
    match w.authz_cmd(owner, AuthzCmd { cred: OPS, op }).unwrap().out {
        AuthzOut::Repo { repo, .. } => repo,
        o => panic!("{o:?}"),
    }
}

fn install(w: &mut World, owner: AccountId, id: InstallationId, row: InstallationRow, app_owner: AccountId) {
    let app = row.app;
    w.names_on(NameKey::Installation(id), NamesOp::RegisterInstallation { id, app, owner }).unwrap();
    w.authz_cmd(owner, AuthzCmd { cred: OPS, op: AuthzOp::Install { id, row, app_owner } }).unwrap();
}

struct Src(Kid, [u8; 32]);

impl KeySource for Src {
    fn current(&self) -> Result<(Kid, SigningSecret), KeyError> {
        Ok((self.0, SigningSecret(self.1)))
    }
}

/// Publishes `kid`'s verifying key on `TokenShard(0)`, as `graftyard mint-key rotate` does.
fn publish(w: &mut World, kid: Kid, secret: [u8; 32], retired: Option<Timestamp>) {
    let key = SigningSecret(secret).public();
    w.tokens_cmd(TokensCmd { cred: OPS, op: TokensOp::PutVerifyingKey { kid, key, retired } }).unwrap();
}

fn keys(w: &mut World) -> graftyard_identity::TokensFold {
    w.tokens(TokenShard(0)).fresh_leader()
}

struct Repo {
    id: RepoId,
    owner: AccountId,
}

impl RepoAccess for Repo {
    fn repo(&self) -> RepoId {
        self.id
    }
    fn owner(&self) -> AccountId {
        self.owner
    }
    fn visibility(&self) -> Visibility {
        Visibility::Private
    }
    fn collaborator(&self, _user: UserId) -> Option<RepoPermission> {
        None
    }
}

fn on(id: RepoId, owner: AccountId) -> Repo {
    Repo { id, owner }
}

// ---- BF-27: revoke a `jti` with 1,000 commands queued under its token ----

/// A world with an org, a user-owned App installed on it for `contents: write`, and a minted token.
struct Installed {
    w: World,
    org: AccountId,
    id: InstallationId,
    claims: InstallationClaims,
}

fn installed() -> Installed {
    let mut w = World::default();
    let dev = acct(0, 3, 1);
    let org = acct(0, 3, 2);
    open(&mut w, dev, AccountKind::User);
    open(&mut w, org, AccountKind::Org);
    let f = fixture();
    let rec = app_record(f.app, &f.public, perms(&[("contents", Access::Write), ("metadata", Access::Read)]), "s3");
    register_app(&mut w, dev, &rec);
    let id = InstallationId(DbId(77));
    let row = InstallationRow { app: f.app, repos: RepoScope::All, permissions: rec.permissions.clone() };
    install(&mut w, org, id, row, dev);
    publish(&mut w, Kid(1), [1; 32], None);
    let now = Timestamp(w.authz(org).now.0);
    let grant = Grant { app: f.app, installation: id, repos: RepoScope::All, permissions: rec.permissions };
    let (text, minted) = Minter::default().mint(&Src(Kid(1), [1; 32]), &grant, now, [3; 10]).unwrap();
    let k = keys(&mut w);
    let claims = stateless::verify(&text, &k, now).unwrap();
    assert_eq!(claims, minted);
    Installed { w, org, id, claims }
}

/// 1,000 commands queued on the org's `Authz` leader under one installation token. Its `jti` is revoked on
/// `jti_shard` while they wait; the leader reads that shard once per entry after the batch froze, so every command
/// decided afterwards is refused with 401.
#[test]
fn bf27_jti_revoked_with_1000_commands_queued() {
    let Installed { mut w, org, claims, .. } = installed();
    let cred = Principal::installation(&claims).credential_shard();
    assert_eq!(cred.account, None);
    assert_eq!(cred.key, Some(CredentialKey::Jti { shard: jti_shard(&claims.jti), jti: claims.jti.0 }));
    let mut g: Group<AuthzCmd> = w.authz.remove(&org.0.0).unwrap();
    // A command under the token before the revocation is decided on its credential: the token is not the operator,
    // so the admin-only `CreateRepo` is refused 403, not 401, which shows the `jti` row was read and passed.
    let create =
        |i: usize| AuthzCmd { cred, op: AuthzOp::CreateRepo { name: RepoName::parse(&format!("r{i}")).unwrap() } };
    let t = g.commit(A, 9_000, create(0)).unwrap().ticket;
    g.settle(&mut |c| w.read(c));
    assert!(
        matches!(g.result(t), Some(Err(CommitError::Refused { err: AuthzErr::Forbidden, .. }))),
        "{:?}",
        g.result(t)
    );
    // The next command's entry waits on its dependency read; 999 more queue behind it.
    let mut tickets = vec![g.commit(A, 10_000, create(1)).unwrap().ticket];
    for i in 2..=1_000 {
        tickets.push(g.commit(A, 10_000 + i as u128, create(i)).unwrap().ticket);
    }
    // `DELETE /installation/token`, under the token itself, while they wait.
    let ack = w.tokens_cmd(revoke_command(&claims)).unwrap();
    assert!(ack.index.0 > 0);
    assert!(w.tokens(jti_shard(&claims.jti)).fresh_leader().jti_revoked(&claims.jti, Timestamp(0)));
    let mut dep_reads = 0;
    g.settle(&mut |c| {
        dep_reads += 1;
        w.read(c)
    });
    let mut refused = 0;
    for t in &tickets {
        match g.result(*t) {
            Some(Err(CommitError::Refused { err: AuthzErr::Unauthorized(CredentialRefusal::Revoked), .. })) => {
                refused += 1
            }
            other => panic!("{t:?}: {other:?}"),
        }
    }
    assert_eq!(refused, 1_000);
    let entries = g.m(A).log().len();
    eprintln!("BF-27: {refused}/1000 refused 401; {dep_reads} dependency reads in {entries} log entries");
    assert!(dep_reads < 20, "reads are per entry: {dep_reads}");
}

/// `DELETE /installation/token` is the token's own: another token's `jti` and a stored-token user are refused, a
/// second delete is a no-op 204, and the row lapses at the token's `exp` with no command.
#[test]
fn delete_installation_token_is_the_tokens_own_and_idempotent() {
    let Installed { mut w, claims, .. } = installed();
    let other = InstallationClaims {
        jti: graftyard_identity::Jti(graftyard_types::Ulid(claims.jti.0.0 + 1)),
        ..claims.clone()
    };
    let theirs = TokensCmd {
        cred: Principal::installation(&other).credential_shard(),
        op: TokensOp::RevokeJti { jti: claims.jti, exp: claims.exp },
    };
    // Another token's `RevokeJti` lands on its own jti's shard, so send it to this jti's shard directly.
    let shard = jti_shard(&claims.jti);
    let mut g = w.tokens.remove(&shard.0).unwrap_or_else(|| {
        w.tokens(shard);
        w.tokens.remove(&shard.0).unwrap()
    });
    let r = g.submit(500, theirs, &mut |c| w.read(c));
    w.tokens.insert(shard.0, g);
    assert_eq!(refused(r), TokensErr::Forbidden);
    let user = TokensCmd {
        cred: CredentialShard { account: Some(acct(0, 3, 1)), key: None },
        op: TokensOp::RevokeJti { jti: claims.jti, exp: claims.exp },
    };
    assert_eq!(refused(w.tokens_cmd(user)), TokensErr::Unauthorized(CredentialRefusal::NoAccount(acct(0, 3, 1))));
    let first = w.tokens_cmd(revoke_command(&claims)).unwrap();
    // The second is under a revoked token: the leader refuses it 401, which the edge answers 204 as GitHub does.
    let second = w.tokens_cmd(revoke_command(&claims));
    assert_eq!(refused(second), TokensErr::Unauthorized(CredentialRefusal::Revoked));
    let ops =
        w.tokens_cmd(TokensCmd { cred: OPS, op: TokensOp::RevokeJti { jti: claims.jti, exp: claims.exp } }).unwrap();
    assert!(ops.index.0 > first.index.0);
    let f = w.tokens(shard).fresh_leader();
    assert_eq!(f.more.jtis.len(), 1, "one row, however many deletes");
    assert!(f.jti_revoked(&claims.jti, Timestamp(claims.exp.0 - 1)));
    assert!(!f.jti_revoked(&claims.jti, claims.exp), "the row lapses at exp");
}

// ---- BF-32: App JWTs, the token request, OAuth secrets, and no secret on `Identity` ----

/// The App half of BF-32 with Octokit's own signer: a JWT `universal-github-app-jwt` made verifies against the
/// record's key on the owner's `Authz` chain, read fresh; one signed by a key not in the record, one naming
/// another App, and one outside its window are refused. Then the token request narrows and mints a `ghs_` token
/// that verifies at the edge.
#[test]
fn bf32_an_octokit_app_jwt_verifies_against_app_record_keys_and_mints() {
    let f = fixture();
    let Installed { mut w, org, id, .. } = installed();
    let dev = acct(0, 3, 1);
    let a = w.authz(dev).fresh_leader();
    let rec = a.app(f.app).expect("registered").clone();
    assert_eq!(verify_app_jwt(&f.jwt, &rec, f.now), Ok(f.app));
    assert_eq!(verify_app_jwt(&f.jwt_other_key, &rec, f.now), Err(AppJwtError::Signature));
    assert_eq!(verify_app_jwt(&f.jwt_other_app, &rec, f.now), Err(AppJwtError::Issuer));
    assert_eq!(verify_app_jwt(&f.jwt, &rec, Timestamp(f.now.0 + 600_000)), Err(AppJwtError::Expired));
    assert_eq!(verify_app_jwt(&f.jwt, &rec, Timestamp(f.now.0 - 120_000)), Err(AppJwtError::NotYet));
    let mut tampered = f.jwt.clone();
    let i = tampered.find('.').unwrap() + 3;
    tampered.replace_range(i..i + 1, if &tampered[i..i + 1] == "A" { "B" } else { "A" });
    assert!(verify_app_jwt(&tampered, &rec, f.now).is_err());
    // The token request: the installation on the org's chain, read fresh.
    let r1 = repo(&mut w, org, "one");
    let r2 = repo(&mut w, org, "two");
    let inst = w.authz(org).fresh_leader();
    let names = |n: &str| match n {
        "one" => Some(r1),
        "two" => Some(r2),
        _ => None,
    };
    let req = AccessTokenRequest {
        repositories: vec!["one".into()],
        repository_ids: vec![],
        permissions: Some(perms(&[("contents", Access::Read)])),
    };
    let grant = access_token_grant(&f.jwt, &rec, id, &inst, &req, &names, f.now).unwrap();
    assert_eq!(grant.repos, RepoScope::Selected(smallvec![r1]));
    assert_eq!(grant.permissions, perms(&[("contents", Access::Read)]));
    // A permission the App lacks is refused 422, and so is a repository the installation cannot see.
    let more = AccessTokenRequest { permissions: Some(perms(&[("issues", Access::Write)])), ..Default::default() };
    assert_eq!(
        access_token_grant(&f.jwt, &rec, id, &inst, &more, &names, f.now),
        Err(TokenRequestError::Narrow(NarrowError::NotGranted("issues".into())))
    );
    let ghost = AccessTokenRequest { repositories: vec!["ghost".into()], ..Default::default() };
    assert_eq!(
        access_token_grant(&f.jwt, &rec, id, &inst, &ghost, &names, f.now),
        Err(TokenRequestError::Narrow(NarrowError::Repository("ghost".into())))
    );
    assert!(matches!(
        access_token_grant(&f.jwt_other_key, &rec, id, &inst, &req, &names, f.now),
        Err(TokenRequestError::Jwt(AppJwtError::Signature))
    ));
    assert_eq!(
        access_token_grant(&f.jwt, &rec, InstallationId(DbId(78)), &inst, &req, &names, f.now),
        Err(TokenRequestError::NoInstallation)
    );
    // Minted and verified at the edge: variable length, `ghs_<app id>_` then a JWS.
    let (text, claims) = Minter::default().mint(&Src(Kid(1), [1; 32]), &grant, f.now, [9; 10]).unwrap();
    assert!(text.starts_with(&format!("ghs_{}_", f.app.0.0)));
    assert_ne!(text.len(), 40);
    let k = keys(&mut w);
    assert_eq!(stateless::verify(&text, &k, f.now), Ok(claims.clone()));
    // `authorize` intersects the claims with the installation's row: read on `one`, nothing on `two`.
    let p = Principal::installation(&claims);
    let authz = w.authz(org).fresh_leader();
    let folds: [&dyn AuthzRead; 1] = [&authz];
    assert_eq!(graftyard_identity::permission(&p, &folds, &on(r1, org)), Some(RepoPermission::Read));
    assert_eq!(graftyard_identity::permission(&p, &folds, &on(r2, org)), None);
    assert_eq!(authorize_write(&p, WriteAction::Push, &folds, &on(r1, org)), Decision::Forbidden);
    assert_eq!(authorize_write(&p, WriteAction::Push, &folds, &on(r2, org)), Decision::Hidden);
    eprintln!("BF-32 App half: Octokit JWT verified; ghs_ token {} chars", text.len());
}

/// The OAuth half of BF-32: an OAuth app registered on the owner's chain holds only its secret's hash. The right
/// secret passes and a wrong one is the edge's 401; a dump of every `Identity` chain the flow touched, and of the
/// `Authz` chain, holds no client secret's bytes, in plain text or hex.
#[test]
fn bf32_a_wrong_oauth_secret_is_refused_and_no_identity_chain_holds_a_client_secret() {
    let mut w = World::default();
    let owner = acct(0, 4, 1);
    let user = acct(0, 4, 2);
    open(&mut w, owner, AccountKind::User);
    let secret = "9f2c1e7d3b5a46c8e0f1a2b3c4d5e6f708192a3b";
    let client = ClientId::parse("0a1b2c3d4e5f60718293").unwrap();
    w.names_on(NameKey::Client(client), NamesOp::RegisterClient { client, owner, app: None }).unwrap();
    let rec = OAuthAppRecord {
        client,
        name: "Octo Web".into(),
        callbacks: smallvec![],
        client_secrets: smallvec![secret_hash(secret)],
        device_flow: false,
    };
    w.authz_cmd(owner, AuthzCmd { cred: OPS, op: AuthzOp::PutOAuthApp { record: rec } }).unwrap();
    let held = w.authz(owner).fresh_leader().oauth_app(&client).unwrap().client_secrets.clone();
    assert!(client_secret_ok(&held, secret));
    assert!(!client_secret_ok(&held, "9f2c1e7d3b5a46c8e0f1a2b3c4d5e6f708192a3c"), "a wrong secret is 401");
    assert!(!client_secret_ok(&held, ""));
    // The user's account, created and signed in, with a credential from the flow.
    let capture =
        AccountCapture { login: Login::parse("mona").unwrap(), kind: AccountKind::User, created: Timestamp(1) };
    w.identity_cmd(user, IdentityCmd { cred: OPS, op: IdentityOp::Create(capture) }).unwrap();
    // Every `Identity` chain's committed events and fold, and the `Authz` chain's, as bytes.
    let mut dump = Vec::new();
    for g in w.identity.values_mut() {
        for e in g.committed_events(A) {
            e.encode(&mut dump);
        }
        dump.extend(codec::to_bytes(&g.fresh_leader()));
    }
    let identity_bytes = dump.len();
    let authz = w.authz(owner).fresh_leader();
    dump.extend(codec::to_bytes(&authz));
    let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).any(|x| x == needle);
    assert!(!find(&dump, secret.as_bytes()), "the secret's text");
    assert!(find(&dump, &secret_hash(secret).0), "the Authz chain holds its hash");
    assert!(!find(&dump[..identity_bytes], &secret_hash(secret).0), "no Identity chain holds even the hash");
    assert!(identity_bytes > 0);
    eprintln!("BF-32 OAuth half: wrong secret refused; {identity_bytes} Identity bytes hold no client secret");
}

// ---- rotation with an overlap window on `TokenShard(0)` ----

/// `graftyard mint-key rotate`: kid 2 is published while kid 1 signs, the source switches after one token
/// lifetime, an isolate still caching kid 1 keeps signing with it for up to `SIGNING_KEY_CACHE`, and kid 1 retires
/// only once every token it could have signed has expired. Through the overlap both verify; after `retired`,
/// kid 1's tokens fail and kid 2's pass. Every step is read through `fresh` on `TokenShard(0)`.
#[test]
fn key_rotation_keeps_both_keys_valid_through_the_overlap() {
    let mut w = World::default();
    let h = ms(INSTALLATION_TOKEN_TTL);
    let t0 = Timestamp(1_800_000_000_000);
    publish(&mut w, Kid(1), [1; 32], None);
    let grant = Grant {
        app: AppId(DbId(5)),
        installation: InstallationId(DbId(6)),
        repos: RepoScope::All,
        permissions: perms(&[("contents", Access::Read)]),
    };
    let old_src = Src(Kid(1), [1; 32]);
    let new_src = Src(Kid(2), [2; 32]);
    // Kid 2 is published at t0 while kid 1 signs.
    publish(&mut w, Kid(2), [2; 32], None);
    let mut epoch1 = KeyEpoch { kid: Kid(1), published: Timestamp(0), switched: None };
    let epoch2 = KeyEpoch { kid: Kid(2), published: t0, switched: None };
    assert_eq!(rotation_step(&epoch1, &epoch2, Timestamp(t0.0 + h - 1)), RotationStep::Wait);
    // An isolate fetched kid 1 just before the switch and caches it.
    let mut cached = Minter::default();
    let switch = Timestamp(t0.0 + h);
    cached.mint(&old_src, &grant, Timestamp(switch.0 - 1), [1; 10]).unwrap();
    assert_eq!(rotation_step(&epoch1, &epoch2, switch), RotationStep::SignWithNew);
    epoch1.switched = Some(switch);
    // After the switch, a fresh isolate signs with kid 2 and the cached one with kid 1 until its cache lapses.
    let mut fresh = Minter::default();
    let late = Timestamp(switch.0 + ms(SIGNING_KEY_CACHE) - 2_000);
    let (old_tok, old_claims) = cached.mint(&new_src, &grant, late, [2; 10]).unwrap();
    assert_eq!(old_claims.kid, Kid(1), "still cached");
    let (new_tok, new_claims) = fresh.mint(&new_src, &grant, late, [3; 10]).unwrap();
    assert_eq!(new_claims.kid, Kid(2));
    let lapsed = Timestamp(switch.0 + ms(SIGNING_KEY_CACHE));
    assert_eq!(cached.mint(&new_src, &grant, lapsed, [4; 10]).unwrap().1.kid, Kid(2), "the cache lapsed");
    assert_eq!(cached.fetches, 2);
    // Retire kid 1 at the step's time; both verify until then.
    let RotationStep::Retire { kid, at } = rotation_step(&epoch1, &epoch2, lapsed) else { panic!() };
    assert_eq!(kid, Kid(1));
    assert!(at.0 >= old_claims.exp.0, "kid 1 retires after its last token's exp");
    w.tokens_cmd(TokensCmd {
        cred: OPS,
        op: TokensOp::PutVerifyingKey { kid, key: SigningSecret([1; 32]).public(), retired: Some(at) },
    })
    .unwrap();
    let k = keys(&mut w);
    let during = Timestamp(old_claims.exp.0 - 1);
    assert_eq!(stateless::verify(&old_tok, &k, during), Ok(old_claims.clone()), "kid 1 inside the overlap");
    assert_eq!(stateless::verify(&new_tok, &k, during), Ok(new_claims.clone()), "kid 2 inside the overlap");
    // A kid 1 token re-dated past `retired` (as a stolen key would sign one) fails; kid 2's still pass.
    let after = Timestamp(at.0 + 1);
    let forged = InstallationClaims {
        iat: Timestamp(after.0 / 1000 * 1000),
        exp: Timestamp(after.0 / 1000 * 1000 + h),
        ..old_claims
    };
    let forged = stateless::sign_with(&SigningSecret([1; 32]), &forged);
    assert_eq!(stateless::verify(&forged, &k, after), Err(StatelessError::Retired));
    let (later_tok, _) = fresh.mint(&new_src, &grant, after, [5; 10]).unwrap();
    assert!(stateless::verify(&later_tok, &k, after).is_ok());
    // A verifying key cannot be swapped under its kid.
    let swap = TokensOp::PutVerifyingKey { kid: Kid(2), key: SigningSecret([9; 32]).public(), retired: None };
    assert_eq!(refused(w.tokens_cmd(TokensCmd { cred: OPS, op: swap })), TokensErr::KeyChanged);
    eprintln!(
        "rotation: overlap {} s from publish to retire; old key verified for {} s after the switch",
        (at.0 - t0.0) / 1000,
        (at.0 - switch.0) / 1000
    );
}

// ---- installations on the owner's `Authz` chain ----

/// `Install` commits only after its `Names` route and within the App's permissions; uninstalling, or narrowing the
/// selection, moves `not_before`, so a token minted before it is refused by `authorize` at once.
#[test]
fn an_installation_is_bounded_by_its_app_and_uninstall_fails_its_tokens_at_once() {
    let Installed { mut w, org, id, claims, .. } = installed();
    let f = fixture();
    let dev = acct(0, 3, 1);
    let r1 = repo(&mut w, org, "one");
    let p = Principal::installation(&claims);
    let read = |w: &mut World| w.authz(org).fresh_leader();
    let perm = |a: &AuthzFold, r| {
        let folds: [&dyn AuthzRead; 1] = [a];
        graftyard_identity::permission(&p, &folds, &on(r, org))
    };
    assert_eq!(perm(&read(&mut w), r1), Some(RepoPermission::Write));
    // Beyond the App's permissions, or with no route, is refused.
    let wide = InstallationRow {
        app: f.app,
        repos: RepoScope::All,
        permissions: perms(&[("contents", Access::Write), ("administration", Access::Write)]),
    };
    let cmd = |row: InstallationRow, id| AuthzCmd { cred: OPS, op: AuthzOp::Install { id, row, app_owner: dev } };
    assert_eq!(refused(w.authz_cmd(org, cmd(wide, id))), AuthzErr::BadRecord);
    let unrouted = InstallationRow { app: f.app, repos: RepoScope::All, permissions: perms(&[]) };
    assert_eq!(refused(w.authz_cmd(org, cmd(unrouted, InstallationId(DbId(99))))), AuthzErr::NotClaimed);
    // A token from another installation of the same App, or naming another App, gets nothing here.
    let stray = InstallationClaims { installation: InstallationId(DbId(99)), ..claims.clone() };
    let folds = read(&mut w);
    let fs: [&dyn AuthzRead; 1] = [&folds];
    assert_eq!(graftyard_identity::permission(&Principal::installation(&stray), &fs, &on(r1, org)), None);
    // Narrowing to another repository moves `not_before`: the old token fails at once, on every repository.
    w.authz(org).advance(5_000);
    let r2 = repo(&mut w, org, "two");
    let narrow = InstallationRow {
        app: f.app,
        repos: RepoScope::Selected(smallvec![r2]),
        permissions: perms(&[("contents", Access::Write)]),
    };
    w.authz_cmd(org, cmd(narrow, id)).unwrap();
    let a = read(&mut w);
    assert!(a.installation(id).unwrap().not_before > claims.iat, "a narrowing moves not_before");
    assert_eq!(perm(&a, r1), None);
    assert_eq!(perm(&a, r2), None, "minted before not_before");
    let fresh_claims =
        InstallationClaims { iat: Timestamp(a.installation(id).unwrap().not_before.0 + 1_000), ..claims.clone() };
    let fp = Principal::installation(&fresh_claims);
    let fs: [&dyn AuthzRead; 1] = [&a];
    assert_eq!(graftyard_identity::permission(&fp, &fs, &on(r2, org)), Some(RepoPermission::Write));
    // Uninstall: every token fails, a second uninstall is a no-op, and an unknown id is refused.
    w.authz(org).advance(5_000);
    w.authz_cmd(org, AuthzCmd { cred: OPS, op: AuthzOp::Uninstall { id } }).unwrap();
    let a = read(&mut w);
    let fs: [&dyn AuthzRead; 1] = [&a];
    assert_eq!(graftyard_identity::permission(&fp, &fs, &on(r2, org)), None);
    w.authz_cmd(org, AuthzCmd { cred: OPS, op: AuthzOp::Uninstall { id } }).unwrap();
    assert_eq!(
        refused(w.authz_cmd(org, AuthzCmd { cred: OPS, op: AuthzOp::Uninstall { id: InstallationId(DbId(5)) } })),
        AuthzErr::NoInstallation
    );
    // The token itself is not the operator: it cannot install or uninstall.
    let as_token = AuthzCmd { cred: p.credential_shard(), op: AuthzOp::Uninstall { id } };
    assert_eq!(refused(w.authz_cmd(org, as_token)), AuthzErr::Forbidden);
    // Every member holds the same rows.
    let g = w.authz(org);
    g.settle(&mut |c| panic!("{c}"));
    let mine = g.m(A).group_fold().fold.more.installations.clone();
    assert_eq!(g.m(B).group_fold().fold.more.installations, mine);
}
