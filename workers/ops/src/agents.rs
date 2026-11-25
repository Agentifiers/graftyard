//! The agents account's `graft-ops` (`design/AGENT-LAYER.md`, Shape, "Controller"; `build/BUILD-PLAN.md`, AM3-WP1):
//! the same executor as the install's (`graftyard_release::ops::run`) under `Account::Agents`, so it runs only
//! `ControllerRoll`, uploads only `graft-agents-front` and `graft-controller`, and signs with `Keys.agents_executor`.
//! The install's `graft-ops` sits idle while a `ControllerRoll` is current, and this one while anything else is.
//!
//! - **The view** is an `AgentsReleaseView`, read over HTTPS from `GET https://<GRAFTYARD_HOST>/api/graftyard/v1/
//!   release/entries` under a `graftyard-agent` App JWT (`AGENT_APP_ID`, `AGENT_APP_KEY`), checked against the
//!   `AgentsAnchor` (`GRAFTYARD_INSTALL`, `GRAFTYARD_CUSTOMER_KEY`), and kept by the isolate between runs. No Durable
//!   Object binding reaches the install: the agents account is another account.
//! - **Commits** are `POST` to the same path, as `graftyard release post` sends them: 201, 409, or 422.
//! - **Keys.** `AGENTS_EXECUTOR_KEY`, the Ed25519 seed `graftyard agents init` stored. No drain key: this account
//!   drains nothing.
//! - **Scripts** are `OPS_SCRIPTS`' `agents-front` and `controller`, deployed with `DEPLOY_TOKEN`, the agents
//!   account's deploy token, through `exec::Api`; `RELEASES` is the agents account's release bucket.
//! - **Gate.** A `ControllerRoll` is judged on `GATE_ERRORS` (`gate::judge_roll`) from `graft-agents-front`'s counts of
//!   `graft-controller`'s answers, which the install's edge stored (`POST release/errors`, `gate::accept_flush`) and
//!   answers summed over its sites (`GET release/errors?since=`), over the step's window and from the intent's start.
//!   No site answering is `Pending`. A test install's `GATE` still forces `pass`, `pending`, or `breach:<why>`.
//! - **The front's default.** A `graft-controller` deploy whose split serves no version 100% first sets
//!   `graft-agents-front`'s `CONTROLLER_DEFAULT` to N, and one that serves a version 100% clears it after; a controller
//!   deployment read while the front's value differs sets it and fails the run (`agents::controller_default`).
//! - Pages are `PAGE graft-agents-ops` lines.
//!
//! Not built: `graft-agents-front`'s own 1%-for-a-day roll under the same intent (OPEN-QUESTIONS A52), and `Rotate`
//! of the agents secrets, which pages by name.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::Duration;

use graftyard_identity::AppId;
use graftyard_identity::appauth::{app_key, sign_app_jwt};
use graftyard_lookup::within;
use graftyard_release::agents::{
    AgentsAnchor, AgentsReleaseView, CONTROLLER_DEFAULT, ENTRIES_PER_PAGE, controller_default, front_bindings,
    front_default, parse_page,
};
use graftyard_release::gate::{self, Verdict};
use graftyard_release::ops::{
    self, Account, Deployment, Exec, Gate, MoveRun, MoveSpec, Ran, RawBundle, Signer, Split, Submit,
};
use graftyard_release::view::VIEW_CALL_MS;
use graftyard_release::{Anchor, IntentId, ReleaseFold, RotateStep, Script, SecretName, SignedEntry, VersionId};
use graftyard_types::{DbId, Index, Timestamp};
use serde_json::Value;
use worker::*;

use crate::exec::{Api, bundle, dwell, secret, seed, var};

const ENTRIES_PATH: &str = "/api/graftyard/v1/release/entries";
const ERRORS_PATH: &str = "/api/graftyard/v1/release/errors";
/// How long an App JWT is reused: well inside the 9 minutes `sign_app_jwt` gives it [inference].
const JWT_REUSE_MS: u64 = 300_000;
/// Pages one view read takes at most: a cold isolate replays the install's history 100 entries at a time.
const PAGES: usize = 50;
/// Reads of one page a run tries before it gives up on the view, each VIEW_CALL_MS at most.
const VIEW_TRIES: usize = 4;
/// How long one commit waits for the edge's answer.
const COMMIT_MS: u64 = 30_000;

thread_local! {
    static VIEW: RefCell<AgentsReleaseView> = RefCell::new(AgentsReleaseView::default());
    static JWT: RefCell<Option<(u64, String)>> = const { RefCell::new(None) };
}

fn now_ms() -> u64 {
    Date::now().as_millis()
}

pub struct AgentsLive<'a> {
    env: &'a Env,
    anchor: AgentsAnchor,
    api: Api,
    executor: [u8; 32],
}

impl<'a> AgentsLive<'a> {
    pub async fn load(env: &'a Env) -> std::result::Result<AgentsLive<'a>, String> {
        let a = Anchor::from_vars(&var(env, "GRAFTYARD_INSTALL")?, &var(env, "GRAFTYARD_CUSTOMER_KEY")?)
            .map_err(|e| format!("anchor: {e}"))?;
        let anchor =
            AgentsAnchor { install: a.install, customer: a.customer, host: var(env, "GRAFTYARD_HOST")?.into() };
        let scripts = serde_json::from_str(&var(env, "OPS_SCRIPTS")?).map_err(|e| format!("OPS_SCRIPTS: {e}"))?;
        Ok(AgentsLive {
            env,
            anchor,
            api: Api { account: var(env, "CF_ACCOUNT")?, token: secret(env, "DEPLOY_TOKEN").await?, scripts },
            executor: seed(env, "AGENTS_EXECUTOR_KEY").await?,
        })
    }

    /// A `graftyard-agent` App JWT, reused for `JWT_REUSE_MS`.
    async fn jwt(&self) -> std::result::Result<String, String> {
        let now = now_ms();
        if let Some((_, j)) = JWT.with(|j| j.borrow().clone()).filter(|(at, _)| now < at + JWT_REUSE_MS) {
            return Ok(j);
        }
        let app = var(self.env, "AGENT_APP_ID")?.parse::<i64>().map_err(|_| "AGENT_APP_ID is not a number")?;
        let key = app_key(&secret(self.env, "AGENT_APP_KEY").await?)?;
        let j = sign_app_jwt(&key, AppId(DbId(app)), Timestamp(now));
        JWT.with(|x| *x.borrow_mut() = Some((now, j.clone())));
        Ok(j)
    }

    fn url(&self) -> String {
        format!("https://{}{ENTRIES_PATH}", self.anchor.host)
    }

    async fn read_page(
        &self,
        jwt: &str,
        after: Index,
    ) -> std::result::Result<Vec<(Index, Vec<u8>, Timestamp)>, String> {
        let url = format!("{}?after={}&per_page={ENTRIES_PER_PAGE}", self.url(), after.0);
        let h = Headers::new();
        h.set("authorization", &format!("Bearer {jwt}")).map_err(|e| e.to_string())?;
        h.set("accept", "application/json").map_err(|e| e.to_string())?;
        h.set("user-agent", "graft-agents-ops").map_err(|e| e.to_string())?;
        let mut init = RequestInit::new();
        init.with_method(Method::Get).with_headers(h);
        let req = Request::new_with_init(&url, &init).map_err(|e| e.to_string())?;
        let read = async {
            let mut r = Fetch::Request(req).send().await.map_err(|e| e.to_string())?;
            if r.status_code() != 200 {
                return Err(format!("after={}: HTTP {}", after.0, r.status_code()));
            }
            parse_page(&r.bytes().await.map_err(|e| e.to_string())?).map_err(|e| format!("after={}: {e}", after.0))
        };
        within(Duration::from_millis(VIEW_CALL_MS), read)
            .await
            .unwrap_or_else(|| Err(format!("after={}: no answer within {VIEW_CALL_MS} ms", after.0)))
    }

    /// The install's stored counts of `graft-controller`'s answers since `since`, summed over its sites
    /// (`GET release/errors`); `None` when the edge did not answer 200 within `VIEW_CALL_MS`.
    async fn counts(&self, jwt: &str, since: u64) -> Option<gate::ScriptErrors> {
        let url = format!("https://{}{ERRORS_PATH}?since={since}", self.anchor.host);
        let h = Headers::new();
        h.set("authorization", &format!("Bearer {jwt}")).ok()?;
        h.set("accept", "application/json").ok()?;
        h.set("user-agent", "graft-agents-ops").ok()?;
        let mut init = RequestInit::new();
        init.with_method(Method::Get).with_headers(h);
        let req = Request::new_with_init(&url, &init).ok()?;
        let read = async {
            let mut r = Fetch::Request(req).send().await.ok()?;
            (r.status_code() == 200).then_some(())?;
            let v: Value = r.json().await.ok()?;
            serde_json::from_value::<gate::ScriptErrors>(v["counts"].clone()).ok()
        };
        within(Duration::from_millis(VIEW_CALL_MS), read).await.flatten()
    }

    /// `graft-agents-front`'s name and the versions API's read of its deployed version.
    async fn front(&self) -> std::result::Result<(String, Value), String> {
        let name = self.api.name(Script::AgentsFront)?.to_string();
        let d = self.api.deployment(Script::AgentsFront).await?;
        let live = ops::upload_source(&d.split).ok_or("graft-agents-front has no deployed version")?;
        let path = format!("/workers/scripts/{name}/versions/{live}");
        let one = self.api.call(&self.api.token, Method::Get, &path, None, false).await?;
        Ok((name, one["result"].clone()))
    }

    /// Sets `graft-agents-front`'s `CONTROLLER_DEFAULT` to `want` unless its deployed version carries it already.
    /// The settings call uploads and deploys a version with the deployed one's bindings and that one change; it is
    /// refused while a version uploaded since is not deployed (Cloudflare error 10214), and the run fails then.
    async fn set_default(&self, want: Option<VersionId>) -> std::result::Result<bool, String> {
        let (name, src) = self.front().await?;
        if front_default(&src) == want {
            return Ok(false);
        }
        let settings = serde_json::json!({ "bindings": front_bindings(&src, want) }).to_string();
        let opts = web_sys::BlobPropertyBag::new();
        opts.set_type("application/json");
        let parts = js_sys::Array::of1(&settings.into());
        let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts).map_err(|e| format!("{e:?}"))?;
        let form = web_sys::FormData::new().map_err(|e| format!("{e:?}"))?;
        form.append_with_blob("settings", &blob).map_err(|e| format!("{e:?}"))?;
        let path = format!("/workers/scripts/{name}/settings");
        self.api.call(&self.api.token, Method::Patch, &path, Some(form.into()), false).await?;
        console_log!("graft-agents-ops {name} {CONTROLLER_DEFAULT} = {want:?}");
        Ok(true)
    }
}

impl Exec for AgentsLive<'_> {
    fn account(&self) -> Account {
        Account::Agents
    }

    fn now(&self) -> Timestamp {
        Timestamp(now_ms())
    }

    fn scripts(&self) -> Vec<Script> {
        self.api
            .scripts
            .keys()
            .filter_map(|k| match k.as_str() {
                "agents-front" => Some(Script::AgentsFront),
                "controller" => Some(Script::Controller),
                _ => None,
            })
            .collect()
    }

    /// Reads pages from the view's index until a short one. A refused entry, or a page that fails `VIEW_TRIES` reads,
    /// fails the run's view, and the isolate keeps what it took: no lower index, and the next run reads on from there.
    /// A cron run starts on a cold isolate, whose first read of the edge can pass VIEW_CALL_MS (LF-7 r1: every run's
    /// first page stopped at 1,000 ms, while the edge answered it in 800 to 880 ms), so a timed-out page is read again,
    /// as the install's `graft-ops` refreshes again (`exec.rs`, `VIEW_TRIES`).
    async fn view(&mut self) -> std::result::Result<ReleaseFold, String> {
        let jwt = self.jwt().await?;
        for _ in 0..PAGES {
            let after = VIEW.with(|v| v.borrow().index());
            let mut read = Err(String::new());
            for _ in 0..VIEW_TRIES {
                read = self.read_page(&jwt, after).await;
                if read.is_ok() {
                    break;
                }
            }
            let page = read?;
            VIEW.with(|v| v.borrow_mut().extend(&self.anchor, &page)).map_err(|e| {
                let why = format!("release/entries after {}: {}: {e}", after.0, e.variant());
                if e.pages() {
                    self.page(&why);
                }
                why
            })?;
            if page.len() < ENTRIES_PER_PAGE {
                return Ok(VIEW.with(|v| v.borrow().fold().clone()));
            }
        }
        Err(format!("{PAGES} pages read, more remain"))
    }

    /// A `graft-controller` deployment counts only while `graft-agents-front` carries the `CONTROLLER_DEFAULT` it
    /// needs: a front that differs is set, and the read fails this run, so no step commits until the next.
    async fn deployment(&mut self, s: Script) -> std::result::Result<Deployment, String> {
        let d = self.api.deployment(s).await?;
        if s == Script::Controller && self.set_default(controller_default(&d.split)).await? {
            return Err(format!("{CONTROLLER_DEFAULT} differed from the controller deployment's; set"));
        }
        Ok(d)
    }

    async fn gate(&mut self, fold: &ReleaseFold) -> Gate {
        match self.env.var("GATE").map(|v| v.to_string()).as_deref() {
            Ok("pass") => return Gate::Pass,
            Ok("pending") => return Gate::Pending,
            Ok(b) if b.starts_with("breach:") => return Gate::Breach(b["breach:".len()..].into()),
            _ => {}
        }
        let (Some((_, n1, n)), Some(c)) = (gate::roll_versions(fold), fold.current.as_deref()) else {
            return Gate::Pending;
        };
        let Ok(jwt) = self.jwt().await else { return Gate::Pending };
        let (since, c_start) = (gate::window_start(fold, c).0, c.intent.issued_at.0);
        let (Some(window), Some(whole)) = (self.counts(&jwt, since).await, self.counts(&jwt, c_start).await) else {
            return Gate::Pending;
        };
        if window.unversioned.answers > 0 {
            console_error!(
                "ALARM graft-agents-ops gate controller: {} answers ({} 5xx) through graft-agents-front reported no version",
                window.unversioned.answers,
                window.unversioned.fivexx
            );
        }
        let g = gate::judge_roll(&window, &whole, n1, n);
        console_log!(
            "graft-agents-ops gate controller: N+1 {:?} thrown {} N {:?} (whole {:?}) -> {g:?}",
            window.of(n1),
            window.threw,
            n.map(|n| window.of(n)),
            n.map(|n| whole.of(n))
        );
        g
    }

    async fn health(&mut self, _fold: &ReleaseFold) -> BTreeMap<u8, Verdict> {
        BTreeMap::new()
    }

    /// A controller split that needs `CONTROLLER_DEFAULT` gets it before the deploy, and one that ends it loses it
    /// after, so no cold `graft-agents-front` isolate follows a split to N+1 (DECISIONS "`controller-default`").
    async fn deploy(&mut self, s: Script, split: &Split) -> std::result::Result<(), String> {
        let want = if s == Script::Controller { controller_default(split) } else { None };
        if want.is_some() {
            self.set_default(want).await?;
        }
        self.api.deploy(s, split).await?;
        if s == Script::Controller && want.is_none() {
            self.set_default(None).await?;
        }
        Ok(())
    }

    async fn submit(&mut self, bytes: &[u8]) -> Submit {
        let h = Headers::new();
        let _ = h.set("content-type", "application/octet-stream");
        let _ = h.set("accept", "application/json");
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_headers(h).with_body(Some(js_sys::Uint8Array::from(bytes).into()));
        let req = match Request::new_with_init(&self.url(), &init) {
            Ok(r) => r,
            Err(e) => return Submit::Failed(e.to_string().into()),
        };
        let sent = async {
            let mut r = Fetch::Request(req).send().await.map_err(|e| e.to_string())?;
            let v: Value = r.json().await.unwrap_or(Value::Null);
            Ok::<_, String>((r.status_code(), v))
        };
        let (status, v) = match within(Duration::from_millis(COMMIT_MS), sent).await {
            Some(Ok(a)) => a,
            Some(Err(e)) => return Submit::Failed(e.into()),
            None => return Submit::Failed(format!("no answer within {COMMIT_MS} ms").into()),
        };
        let s = |k: &str| v[k].as_str().unwrap_or("").into();
        match status {
            201 => Submit::Committed,
            409 => Submit::Stale,
            422 => Submit::Refused { variant: s("refusal"), message: s("message") },
            _ => Submit::Failed(format!("HTTP {status}: {}", v["message"].as_str().unwrap_or("")).into()),
        }
    }

    fn sign(&self, signer: Signer, e: &mut SignedEntry) -> std::result::Result<(), String> {
        match signer {
            Signer::Executor => {
                e.sign(&self.executor);
                Ok(())
            }
            Signer::Drain => Err("the agents graft-ops holds no drain key".into()),
        }
    }

    fn page(&mut self, why: &str) {
        console_error!("PAGE graft-agents-ops: {why}");
    }

    async fn bundle(&mut self, fold: &ReleaseFold) -> std::result::Result<Option<RawBundle>, String> {
        bundle(self.env, fold).await
    }

    async fn tagged(
        &mut self,
        tag: &str,
        scripts: &[Script],
    ) -> std::result::Result<BTreeMap<Script, VersionId>, String> {
        self.api.tagged(tag, scripts).await
    }

    async fn upload(&mut self, s: Script, tag: &str, modules: &[(String, Vec<u8>)]) -> std::result::Result<(), String> {
        self.api.upload(s, tag, modules).await
    }

    async fn rotate(
        &mut self,
        _fold: &ReleaseFold,
        secret: SecretName,
        _slot: u8,
        _step: RotateStep,
    ) -> std::result::Result<(), String> {
        Err(format!("the agents graft-ops does not rotate {secret:?} yet"))
    }

    async fn move_run(&mut self, _intent: IntentId) -> MoveRun {
        MoveRun::Absent
    }

    async fn start_move(&mut self, _intent: IntentId, _spec: MoveSpec) -> std::result::Result<(), String> {
        Err("the agents graft-ops moves no group".into())
    }
}

/// One cron run of the agents `graft-ops`. Each action is one log line.
pub async fn run(env: &Env) -> Vec<Ran> {
    let mut live = match AgentsLive::load(env).await {
        Ok(l) => l,
        Err(e) => {
            console_error!("PAGE graft-agents-ops: cannot start: {e}");
            return vec![Ran::Error(e.into())];
        }
    };
    let log = ops::run(&mut live, dwell(env)).await;
    for r in &log {
        console_log!("graft-agents-ops {r:?}");
    }
    log
}
