//! An install in memory for `graft-mcp`'s tests: real `Identity`, `Tokens`, `Authz`, and `Names` members
//! (`crates/identity/tests/common`, included by path), the host's `graftyard_api::mcp` handlers over them, and a stand-in
//! for the edge's OAuth routes (`workers/edge/src/ident.rs` and `usertok.rs`) that commits the same `StorePair` and
//! `Revoke` commands. `graft-mcp`'s `flow::Env` calls this bed where the Worker calls the `EDGE` binding.

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use base64::Engine as _;
use graftyard_api::mcp::{self, Call};
use graftyard_identity::{
    AccountCapture, AccountKind, AgentApp, AgentKind, AppKey, AppRecord, AuthzCmd, AuthzOp, AuthzRead as _, ClientId,
    CredentialRef, IdentityCmd, IdentityFold, IdentityOp, IdentityOut, NameKey, NamesCmd, NamesOp, NamesOut,
    OpaqueToken, PermissionSet, RsaPublic, ScopeSet, Slug, TokenFields, TokenKind, TokenRow, TokensCmd, TokensOp,
    TokensRead as _, account_shard, install_account,
};
use graftyard_mcp::flow::{self, Req, Resp};
use graftyard_mcp::{CimdDoc, Fetched, Sealer};
use graftyard_raft::CommitError;
use graftyard_types::{AccountId, ChainId, CredentialShard, DbId, RepoId, Timestamp};
use graftyard_wire::{Body, Reply};
use serde_json::{Value, json};
use smallvec::smallvec;

use crate::common::world::{FreshLeader, OPS, World};
use crate::common::{acct, login};

pub const HOST: &str = "https://git.acme.test";
pub const ORIGIN: &str = "https://mcp.git.acme.test";
pub const CLIENT_SECRET: &str = "0123456789abcdef0123456789abcdef01234567";
pub const CIMD: &str = "https://client.example/oauth/client.json";
pub const REDIRECT: &str = "http://127.0.0.1:33418/callback";
pub const OWNER_LOGIN: &str = "acme";
pub const REPO_NAME: &str = "mono";
pub const REPO: RepoId = RepoId(DbId(4_242));

/// One install's members plus the edge's stand-ins. `now` moves only when a test moves it.
pub struct Bed {
    pub w: RefCell<World>,
    pub now: Cell<u64>,
    pub app: AppRecord,
    pub owner: AccountId,
    pub users: Vec<(AccountId, String)>,
    /// The CIMD document's answer, as the client's server would give it.
    pub cimd: RefCell<Fetched>,
    pub cimd_fetches: Cell<u32>,
    pub cimd_cache: RefCell<BTreeMap<String, (CimdDoc, u64)>>,
    /// The host's browser session: who the stand-in `/login/oauth/authorize` signs in.
    pub browser: Cell<usize>,
    /// The host's authorization codes, each once: code to (user index).
    pub codes: RefCell<BTreeMap<String, usize>>,
    /// Every upstream call, for tests that check what reached the host.
    pub calls: RefCell<Vec<(String, String)>>,
    /// Each token `DELETE /applications/{client_id}/token` named, in order.
    pub deleted: RefCell<Vec<String>>,
    /// When set, the edge's `Identity` read answers this fold: a read from before a commit it raced.
    pub stale_identity: RefCell<Option<IdentityFold>>,
    rng: Cell<u64>,
    sealer: Sealer,
    deny: Vec<String>,
}

fn rand(seed: &Cell<u64>) -> u64 {
    let mut x = seed.get();
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    seed.set(x);
    x
}

impl Bed {
    /// The install account's `Authz` with `Graftyard MCP` registered as `graftyard apps init` plans it, two users, and
    /// one org with one repository.
    pub fn new() -> Bed {
        let mut w = World::default();
        let install = install_account();
        let open = |k| AuthzCmd { cred: OPS, op: AuthzOp::Open { kind: k, enterprise: None } };
        w.authz_cmd(install, open(AccountKind::Org)).unwrap();
        let slug = Slug::parse(mcp::MCP_SLUG).unwrap();
        let app = match w
            .names_on(NameKey::AppSlug(slug.clone()), NamesOp::ClaimSlug { slug: slug.clone(), owner: install })
        {
            Ok(a) => match a.out {
                NamesOut::App(id) => id,
                o => panic!("{o:?}"),
            },
            e => panic!("{e:?}"),
        };
        let record = AppRecord {
            id: app,
            slug,
            name: "Graftyard MCP".into(),
            client: ClientId::for_app(app),
            keys: smallvec![AppKey { id: DbId(1), public: RsaPublic(vec![0x30, 0x82].into()), created: Timestamp(1) }],
            permissions: PermissionSet(
                [("contents", graftyard_identity::Access::Write), ("metadata", graftyard_identity::Access::Read)]
                    .into_iter()
                    .map(|(k, a)| (k.into(), a))
                    .collect(),
            ),
            events: smallvec![],
            webhook: None,
            webhook_secret: None,
            client_secrets: smallvec![graftyard_identity::appauth::secret_hash(CLIENT_SECRET)],
            callbacks: smallvec![graftyard_identity::Url(format!("{ORIGIN}/callback").into())],
            public: true,
        };
        w.authz_cmd(install, AuthzCmd { cred: OPS, op: AuthzOp::PutApp { record: Box::new(record.clone()) } }).unwrap();
        let row = AgentApp { app, kind: AgentKind::Mcp, confine: None };
        w.authz_cmd(install, AuthzCmd { cred: OPS, op: AuthzOp::PutAgentApp(row) }).unwrap();
        let mut users = Vec::new();
        for (i, l) in ["alice", "bob"].iter().enumerate() {
            let u = acct(0, 3, i as u16 + 1);
            let op = NamesOp::PutAccount { id: u, suspended: false, sessions_not_before: Timestamp(0) };
            w.names_cmd(ChainId::Names(account_shard(u)), NamesCmd { cred: OPS, op }).unwrap();
            let c = AccountCapture { login: login(l), kind: AccountKind::User, created: Timestamp(1) };
            w.identity_cmd(u, IdentityCmd { cred: OPS, op: IdentityOp::Create(c) }).unwrap();
            users.push((u, (*l).to_owned()));
        }
        let owner = acct(0, 4, 1);
        w.authz_cmd(owner, open(AccountKind::Org)).unwrap();
        let doc = json!({ "client_id": CIMD, "client_name": "Example agent", "redirect_uris": [REDIRECT] });
        Bed {
            w: RefCell::new(w),
            now: Cell::new(1_800_000_000_000),
            app: record,
            owner,
            users,
            cimd: RefCell::new(Fetched {
                status: 200,
                cache_control: Some("max-age=300".into()),
                body: doc.to_string().into_bytes(),
            }),
            cimd_fetches: Cell::new(0),
            cimd_cache: RefCell::new(BTreeMap::new()),
            browser: Cell::new(0),
            codes: RefCell::new(BTreeMap::new()),
            calls: RefCell::new(Vec::new()),
            deleted: RefCell::new(Vec::new()),
            stale_identity: RefCell::new(None),
            rng: Cell::new(0x9e37_79b9_7f4a_7c15),
            sealer: Sealer::new(vec![[0x5a; 32]]).unwrap(),
            deny: vec!["git.acme.test".into(), "acme.test".into()],
        }
    }

    pub fn user(&self, i: usize) -> AccountId {
        self.users[i].0
    }

    pub fn client_id(&self) -> String {
        self.app.client.as_str().to_owned()
    }

    pub fn basic(&self, secret: &str) -> String {
        format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{}:{secret}", self.client_id())))
    }

    fn draw(&self) -> OpaqueToken {
        loop {
            let r = || rand(&self.rng);
            if let Ok((s, _)) = graftyard_identity::draw_secret(r, graftyard_identity::Family::Na)
                && let Some(t) = OpaqueToken::new(TokenKind::UserToServer, s)
            {
                return t;
            }
        }
    }

    /// The edge's `issue_pair`: a `ghu_` of `Graftyard MCP` and its `ghr_`, one `StorePair` entry.
    pub fn issue_pair(&self, u: AccountId) -> (String, String) {
        let ghu = self.draw();
        let (rs, _) =
            graftyard_identity::draw_secret_on(|| rand(&self.rng), ghu.shard(), 200_000).expect("a ghr_ on its shard");
        let ghr = OpaqueToken::new(TokenKind::Refresh, rs).unwrap();
        let now = self.now.get();
        let fields = TokenFields { app: Some(self.app.id), client: Some(self.app.client), ..TokenFields::default() };
        let op = TokensOp::StorePair {
            user: ghu.hash(),
            refresh: ghr.hash(),
            account: u,
            scopes: ScopeSet(1),
            user_expires: Some(Timestamp(now + 8 * 3_600_000)),
            refresh_expires: Some(Timestamp(now + 184 * 86_400_000)),
            fields,
        };
        self.w.borrow_mut().tokens_cmd(TokensCmd { cred: OPS, op }).unwrap();
        (ghu.text(), ghr.text())
    }

    /// A live `ghu_` of another App for `u`.
    pub fn other_app_token(&self, u: AccountId, app: graftyard_identity::AppId, client: ClientId) -> String {
        let t = self.draw();
        let fields = TokenFields { app: Some(app), client: Some(client), ..TokenFields::default() };
        let op = TokensOp::StoreWith {
            hash: t.hash(),
            kind: TokenKind::UserToServer,
            account: u,
            scopes: ScopeSet(1),
            expires: Some(Timestamp(self.now.get() + 3_600_000)),
            fields,
        };
        self.w.borrow_mut().tokens_cmd(TokensCmd { cred: OPS, op }).unwrap();
        t.text()
    }

    /// A token's row through `fresh` on its shard.
    pub fn row(&self, text: &str) -> Option<(TokenRow, Option<TokenFields>)> {
        let t = OpaqueToken::parse(text).ok()?;
        let f = self.w.borrow_mut().tokens(t.shard()).fresh_leader();
        f.token(&t.hash()).map(|r| (r, f.fields(&t.hash()).cloned()))
    }

    pub fn revoked(&self, text: &str) -> bool {
        self.row(text).is_some_and(|(r, _)| r.revoked)
    }

    pub fn identity_fold(&self, a: AccountId) -> IdentityFold {
        self.w.borrow_mut().identity(a).fresh_leader()
    }

    /// One call to the host's grant endpoints, as the edge routes it.
    pub fn grants(&self, method: &str, path: &str, auth: Option<&str>, user: Option<&str>, body: &Value) -> Reply {
        let route = mcp::route(path).expect("a grant path");
        let call = Call { method, authorization: auth, user_token: user, body };
        futures_executor::block_on(mcp::grants(self, route, call))
    }

    /// `token/scoped` as the edge routes it: `None` is the edge's 404 for any other client.
    pub fn scoped(&self, client: &str, auth: Option<&str>, body: &Value) -> Option<Reply> {
        let call = Call { method: "POST", authorization: auth, user_token: None, body };
        futures_executor::block_on(mcp::token_scoped(self, client, call))
    }

    pub fn serve(&self, req: Req) -> Resp {
        futures_executor::block_on(flow::serve(self, &req))
    }
}

pub fn body_json(r: &Reply) -> Value {
    match &r.body {
        Body::Json(v) => v.clone(),
        _ => Value::Null,
    }
}

fn reply_bytes(r: &Reply) -> (u16, Vec<u8>) {
    let b = match &r.body {
        Body::Json(v) => v.to_string().into_bytes(),
        Body::Bytes(b) => b.clone(),
        Body::Empty => Vec::new(),
    };
    (r.status, b)
}

impl mcp::Host for Bed {
    fn now(&self) -> Timestamp {
        Timestamp(self.now.get())
    }
    fn web(&self) -> String {
        HOST.into()
    }
    fn enabled(&self) -> bool {
        true
    }
    fn draw(&self) -> OpaqueToken {
        Bed::draw(self)
    }
    async fn mcp_app(&self) -> Result<Option<AppRecord>, Reply> {
        let f = self.w.borrow_mut().authz(install_account()).fresh_leader();
        let mcp = f.more.more.agent_apps.values().find(|a| a.kind == AgentKind::Mcp).and_then(|a| f.app(a.app));
        Ok(mcp.filter(|r| r.slug.as_str() == mcp::MCP_SLUG).cloned())
    }
    async fn token(&self, text: &str) -> Result<Option<(TokenRow, Option<TokenFields>)>, Reply> {
        Ok(self.row(text))
    }
    async fn identity(&self, a: AccountId) -> Result<Option<IdentityFold>, Reply> {
        if let Some(f) = self.stale_identity.borrow().clone() {
            return Ok(Some(f));
        }
        let mut w = self.w.borrow_mut();
        Ok(w.identity.contains_key(&a.0.0).then(|| w.identity(a).fresh_leader()))
    }
    async fn commit_identity(&self, a: AccountId, cmd: IdentityCmd) -> Result<Result<IdentityOut, String>, Reply> {
        Ok(match self.w.borrow_mut().identity_cmd(a, cmd) {
            Ok(acked) => Ok(acked.out),
            Err(CommitError::Refused { err, .. }) => {
                Err(format!("{err:?}").split(['(', ' ', '{']).next().unwrap().into())
            }
            Err(e) => panic!("{e:?}"),
        })
    }
    async fn commit_tokens(&self, cmd: TokensCmd) -> Result<Result<(), String>, Reply> {
        Ok(match self.w.borrow_mut().tokens_cmd(cmd) {
            Ok(_) => Ok(()),
            Err(CommitError::Refused { err, .. }) => {
                Err(format!("{err:?}").split(['(', ' ', '{']).next().unwrap().into())
            }
            Err(e) => panic!("{e:?}"),
        })
    }
    async fn list_credential(&self, a: AccountId, c: CredentialRef, title: &str) {
        let op = IdentityOp::AddCredential { credential: c, title: title.into() };
        let _ = self.w.borrow_mut().identity_cmd(a, IdentityCmd { cred: CredentialShard::NONE, op });
    }
    async fn account_of(&self, l: &str) -> Result<Option<AccountId>, Reply> {
        Ok((l == OWNER_LOGIN).then_some(self.owner))
    }
    async fn repo(&self, owner: AccountId, name: &str) -> Result<Option<RepoId>, Reply> {
        Ok((owner == self.owner && name == REPO_NAME).then_some(REPO))
    }
    async fn user_json(&self, a: AccountId) -> Value {
        let l = self.users.iter().find(|(u, _)| *u == a).map(|(_, l)| l.clone()).unwrap_or_default();
        json!({
            "login": l, "id": a.0.0, "node_id": "U_x", "avatar_url": format!("{HOST}/identicons/{l}.png"),
            "gravatar_id": "", "url": format!("{HOST}/api/v3/users/{l}"), "html_url": format!("{HOST}/{l}"),
            "followers_url": "", "following_url": "", "gists_url": "", "starred_url": "", "subscriptions_url": "",
            "organizations_url": "", "repos_url": "", "events_url": "", "received_events_url": "", "type": "User",
            "site_admin": false,
        })
    }
}

impl flow::Env for Bed {
    fn now(&self) -> u64 {
        self.now.get()
    }
    fn origin(&self) -> &str {
        ORIGIN
    }
    fn host_origin(&self) -> &str {
        HOST
    }
    fn client(&self) -> (&str, &str) {
        (self.app.client.as_str(), CLIENT_SECRET)
    }
    fn sealer(&self) -> &Sealer {
        &self.sealer
    }
    fn deny(&self) -> &[String] {
        &self.deny
    }
    fn random(&self) -> [u8; 16] {
        let (a, b) = (rand(&self.rng), rand(&self.rng));
        let mut o = [0u8; 16];
        o[..8].copy_from_slice(&a.to_le_bytes());
        o[8..].copy_from_slice(&b.to_le_bytes());
        o
    }
    fn allow_authorize(&self, _ip: &str) -> bool {
        true
    }
    fn cimd_cached(&self, url: &str) -> Option<CimdDoc> {
        let now = self.now.get();
        self.cimd_cache.borrow().get(url).filter(|(_, until)| *until > now).map(|(d, _)| d.clone())
    }
    fn cimd_keep(&self, url: &str, doc: &CimdDoc, secs: u64) {
        self.cimd_cache.borrow_mut().insert(url.into(), (doc.clone(), self.now.get() + secs * 1000));
    }
    async fn fetch(&self, _url: &url::Url) -> Result<Fetched, String> {
        self.cimd_fetches.set(self.cimd_fetches.get() + 1);
        let mut f = self.cimd.borrow().clone();
        f.body.truncate(graftyard_mcp::CIMD_MAX_BYTES + 1);
        Ok(f)
    }
    async fn edge(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: Option<Vec<u8>>,
    ) -> Result<(u16, Vec<u8>), String> {
        self.calls.borrow_mut().push((method.into(), path.into()));
        let h = |k: &str| headers.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str());
        let body = body.unwrap_or_default();
        let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let path_only = path.split('?').next().unwrap();
        let client = self.client_id();
        Ok(match (method, path_only) {
            ("POST", "/login/oauth/access_token") => self.host_access_token(&body),
            ("GET", "/api/v3/user") => {
                let tok = h("authorization").and_then(|a| a.strip_prefix("Bearer ")).unwrap_or_default();
                match self.row(tok) {
                    Some((r, _)) if !r.revoked => (200, json!({ "id": r.account.0.0 }).to_string().into_bytes()),
                    _ => (401, b"{}".to_vec()),
                }
            }
            (m, p) if p.starts_with("/api/graftyard/v1/mcp/grants") => {
                let route = mcp::route(p.strip_prefix("/api/graftyard/v1").unwrap()).expect("a grant path");
                let call = Call {
                    method: m,
                    authorization: h("authorization"),
                    user_token: h("x-graftyard-user-token"),
                    body: &json,
                };
                reply_bytes(&mcp::grants(self, route, call).await)
            }
            ("POST", p) if p == format!("/api/v3/applications/{client}/token/scoped") => {
                let call = Call { method: "POST", authorization: h("authorization"), user_token: None, body: &json };
                reply_bytes(&mcp::token_scoped(self, &client, call).await.unwrap_or_else(Reply::not_found))
            }
            ("DELETE", p) if p == format!("/api/v3/applications/{client}/token") => {
                if h("authorization") != Some(&self.basic(CLIENT_SECRET)) {
                    return Ok((404, Vec::new()));
                }
                self.deleted.borrow_mut().extend(json["access_token"].as_str().map(str::to_owned));
                let t = json["access_token"].as_str().and_then(|t| OpaqueToken::parse(t).ok());
                match t {
                    Some(t) => {
                        let op = TokensOp::Revoke { hash: t.hash() };
                        let _ = self.w.borrow_mut().tokens_cmd(TokensCmd { cred: OPS, op });
                        (204, Vec::new())
                    }
                    None => (422, Vec::new()),
                }
            }
            _ => (404, Vec::new()),
        })
    }
}

impl Bed {
    /// The browser at the host's OAuth: the signed-in user approves `Graftyard MCP`, and the host redirects to
    /// `graft-mcp`'s callback with a code and the `state` it was given.
    pub fn host_authorize(&self, location: &str) -> String {
        let u = url::Url::parse(location).unwrap();
        assert_eq!(u.origin().ascii_serialization(), HOST, "the consent redirects to the host's OAuth");
        assert_eq!(u.path(), "/login/oauth/authorize");
        let q: BTreeMap<String, String> = u.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], self.client_id());
        assert_eq!(q["redirect_uri"], format!("{ORIGIN}/callback"));
        let code = format!("hc{}", rand(&self.rng));
        self.codes.borrow_mut().insert(code.clone(), self.browser.get());
        format!("/callback?code={code}&state={}", q["state"])
    }

    /// `POST /login/oauth/access_token` for `Graftyard MCP`: a code once, or a refresh that spends the `ghr_` by
    /// revoking its pair and issues a new one (`usertok::refresh`).
    fn host_access_token(&self, body: &[u8]) -> (u16, Vec<u8>) {
        let f: BTreeMap<String, String> = url::form_urlencoded::parse(body).into_owned().collect();
        if f.get("client_id") != Some(&self.client_id())
            || f.get("client_secret").map(String::as_str) != Some(CLIENT_SECRET)
        {
            return (401, json!({ "error": "incorrect_client_credentials" }).to_string().into_bytes());
        }
        let pair = |u| {
            let (a, r) = self.issue_pair(u);
            (200, json!({ "access_token": a, "refresh_token": r, "token_type": "bearer" }).to_string().into_bytes())
        };
        if f.get("grant_type").map(String::as_str) == Some("refresh_token") {
            let text = f.get("refresh_token").cloned().unwrap_or_default();
            let bad = (200, json!({ "error": "bad_refresh_token" }).to_string().into_bytes());
            let Some((row, _)) = self.row(&text).filter(|(r, _)| r.kind == TokenKind::Refresh && !r.revoked) else {
                return bad;
            };
            let t = OpaqueToken::parse(&text).unwrap();
            if self.w.borrow_mut().tokens_cmd(TokensCmd { cred: OPS, op: TokensOp::Revoke { hash: t.hash() } }).is_err()
            {
                return bad;
            }
            return pair(row.account);
        }
        match f.get("code").and_then(|c| self.codes.borrow_mut().remove(c)) {
            Some(i) => pair(self.user(i)),
            None => (200, json!({ "error": "bad_verification_code" }).to_string().into_bytes()),
        }
    }
}
