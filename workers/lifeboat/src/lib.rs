//! The lifeboat bundle's puller (`design/DESIGN.md`, "Install operations", Lifeboat; Shape, "Lifeboat feed";
//! `decisions/DECISIONS.md`, "`lifeboat-bundle`"). One Durable Object, `Puller`, named `PULLER_OBJECT`, runs
//! `graftyard_lifeboat::pass` on its own alarm, since workerd has no cron trigger:
//!
//! - **Feed.** `FEED_URL` and `FEED_BUCKET` with the bucket-scoped R2 read token (`FEED_KEY_ID`, `FEED_SECRET`), over
//!   R2's S3 API (`graftyard_lifeboat::feed`).
//! - **Edge.** `EDGE_URL`, each request signed with the host's key (`LIFEBOAT_KEY`, the seed `graftyard lifeboat init`
//!   wrote) as `LIFEBOAT_HOST` (`graftyard_identity::lifeboat::sign`).
//! - **Copies.** Each chain's rows to the site build's `Member` on `MEMBER` (`POST /import`), one learner per chain.
//! - **Packs.** To `graft-blob` on `BLOB_URL`, which checks the checksum again before its durable put.
//! - **Scrub.** Weekly, `graft-blob`'s `POST /_scrub`, which checks every key against its recorded SHA-256 and
//!   repairs from a sibling. A key it cannot repair alarms with the key and the copy whose entry named it, on every
//!   pass until a later scrub finds it whole (`pass::scrubbed`).
//! - **Decoder.** Daily, or when the release changes, a `LifeboatDecoder` for `LIFEBOAT_RELEASE` signed with the
//!   host's key, bound to the last digest of the edge's group, which a 409 names.
//!
//! The front reads `GET /status`: the decoder and when the copy was last current. `POST /pass` runs one pass now, for
//! a bed, and `POST /scrub` one scrub now, as after an operator pulls a lost key again. Each pass logs one line; a halt or an alarming lag logs it as an error.

use std::cell::RefCell;

use graftyard_git::export::EXPORT_PREFIX;
use graftyard_identity::lifeboat;
use graftyard_lifeboat::feed::{S3, Signed, listed};
use graftyard_lifeboat::pass::{self, Copied, Host, Imported, Owed, Page, ScrubReport, State};
use graftyard_lifeboat::status::LifeboatStatus;
use graftyard_release::{Bound, ReleaseEntry, SignedEntry};
use graftyard_types::{ChainId, Digest, Index, ReleaseId, Timestamp};
use serde_json::{Value, json};
use worker::*;

/// How often the alarm runs a pass while the last one found nothing more [guess: a feed round's ~15 s].
const EVERY_MS: i64 = 10_000;
const STATE_KEY: &str = "state";
const AGENT: &str = "graftyard-lifeboat";

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn now() -> u64 {
    Date::now().as_millis()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

struct Live {
    env: Env,
    s3: S3,
    edge: String,
    host: String,
    seed: [u8; 32],
    blob: String,
}

impl Live {
    fn load(env: &Env) -> std::result::Result<Live, String> {
        let var = |k: &str| env.var(k).map(|v| v.to_string()).map_err(|_| format!("{k} is not set"));
        let sec = |k: &str| env.secret(k).map(|v| v.to_string()).map_err(|_| format!("{k} is not set"));
        let seed = Digest::parse_hex(sec("LIFEBOAT_KEY")?.trim()).ok_or("LIFEBOAT_KEY is not 64 hex digits")?.0;
        Ok(Live {
            env: env.clone(),
            s3: S3 {
                endpoint: var("FEED_URL")?,
                bucket: var("FEED_BUCKET")?,
                key_id: sec("FEED_KEY_ID")?,
                secret: sec("FEED_SECRET")?,
                region: var("FEED_REGION").unwrap_or_else(|_| "auto".into()),
            },
            edge: var("EDGE_URL")?.trim_end_matches('/').to_string(),
            host: var("LIFEBOAT_HOST")?,
            seed,
            blob: var("BLOB_URL")?.trim_end_matches('/').to_string(),
        })
    }

    async fn s3_get(&self, s: Signed) -> std::result::Result<Response, String> {
        let h = Headers::new();
        for (k, v) in &s.headers {
            h.set(k, v).map_err(err)?;
        }
        let mut init = RequestInit::new();
        init.with_headers(h);
        Fetch::Request(Request::new_with_init(&s.url, &init).map_err(err)?).send().await.map_err(err)
    }

    /// A signed `GET` of an export path (below `EXPORT_PREFIX`, with its query).
    async fn signed(&self, rest: &str) -> std::result::Result<Response, String> {
        let path = format!("{EXPORT_PREFIX}/{rest}");
        let s = lifeboat::sign(&self.seed, &self.host, Timestamp(now()), "GET", &path);
        let h = Headers::new();
        h.set(lifeboat::HOST_HEADER, &s.host).map_err(err)?;
        h.set(lifeboat::AT_HEADER, &s.at.0.to_string()).map_err(err)?;
        h.set(lifeboat::SIG_HEADER, &s.sig).map_err(err)?;
        h.set("user-agent", AGENT).map_err(err)?;
        let mut init = RequestInit::new();
        init.with_headers(h);
        let req = Request::new_with_init(&format!("{}{path}", self.edge), &init).map_err(err)?;
        Fetch::Request(req).send().await.map_err(err)
    }
}

impl Host for Live {
    fn now(&self) -> u64 {
        now()
    }

    async fn list(&self, after: Option<&str>) -> std::result::Result<(Vec<String>, bool), String> {
        let mut r = self.s3_get(self.s3.list_after(after, 100, now())).await?;
        if r.status_code() != 200 {
            return Err(format!("feed list: {}", r.status_code()));
        }
        Ok(listed(&r.text().await.map_err(err)?))
    }

    async fn object(&self, key: &str) -> std::result::Result<Vec<u8>, String> {
        let mut r = self.s3_get(self.s3.get(Some(key), &[], now())).await?;
        if r.status_code() != 200 {
            return Err(format!("feed {key}: {}", r.status_code()));
        }
        r.bytes().await.map_err(err)
    }

    async fn entries(&self, chain: ChainId, from: Index, limit: u64) -> std::result::Result<Page, String> {
        let mut r = self.signed(&format!("chains/{chain}/entries?from={}&limit={limit}", from.0)).await?;
        if r.status_code() != 200 {
            return Ok(Page::Status(r.status_code()));
        }
        let rows = graftyard_git::export::unframe(&r.bytes().await.map_err(err)?)?;
        Ok(Page::Rows { first: from, rows })
    }

    async fn import(&self, chain: ChainId, rows: &[Vec<u8>]) -> std::result::Result<Imported, String> {
        let stub = self
            .env
            .durable_object("MEMBER")
            .and_then(|n| n.get_by_name(&graftyard_lookup::lifeboat_object(chain)))
            .map_err(err)?;
        // A repository copy's owner, for its family: the install's `RepoOwner` row, read from the copy of its
        // `Names` shard, which the feed carries before any of the repository's rows can matter to a reader.
        let body = json!({ "chain": chain, "rows": rows.iter().map(|r| hex(r)).collect::<Vec<_>>(), "owner": owner_of(&self.env, chain).await });
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_body(Some(body.to_string().into()));
        let mut r = stub
            .fetch_with_request(Request::new_with_init("https://member/import", &init).map_err(err)?)
            .await
            .map_err(|e| open_files_error(chain, &e.to_string()))?;
        let status = r.status_code();
        let text = r.text().await.map_err(err)?;
        // Not yet: a repository copy whose owner its `Names` copy does not name. The chain stays behind, so the edge
        // fills it on a later pass.
        if status == 409 {
            return Ok(Imported { held: Index(0), halt: None });
        }
        // A copy that refuses the rows (entry 1 rebuilds no record, a family no member hosts) halts its chain, as a
        // refused row does: the rows before stay, nothing is skipped, and the other chains go on.
        if (400..500).contains(&status) {
            // `held` 0: the puller's cursor never moves back (`Puller::applied`), so it stays where the copy is.
            return Ok(Imported { held: Index(0), halt: Some(format!("{status} {text}")) });
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("import {chain}: {status}: {e}"))?;
        if status != 200 {
            return Err(format!("import {chain}: {status} {v}"));
        }
        let halt = v.get("halt").filter(|h| !h.is_null()).map(|h| h.to_string());
        Ok(Imported { held: Index(v["held"].as_u64().unwrap_or(0)), halt })
    }

    async fn put_pack(&self, p: &Owed) -> std::result::Result<Copied, String> {
        let mut r = self.signed(&format!("packs/{}?network={}", p.id, p.network.0.0)).await?;
        if r.status_code() != 200 {
            return Ok(Copied::Status(r.status_code()));
        }
        let bytes = r.bytes().await.map_err(err)?;
        if bytes.len() as u64 != p.bytes || graftyard_lifeboat::blob::sha256(&bytes) != p.checksum.0 {
            return Ok(Copied::Corrupt);
        }
        let key = format!("pack/{}/{}", p.network.0.0, p.id);
        let h = Headers::new();
        h.set("x-graftyard-sha256", &p.checksum.hex()).map_err(err)?;
        let mut init = RequestInit::new();
        init.with_method(Method::Put)
            .with_headers(h)
            .with_body(Some(js_sys::Uint8Array::from(bytes.as_slice()).into()));
        let req = Request::new_with_init(&format!("{}/{key}", self.blob), &init).map_err(err)?;
        let r = Fetch::Request(req).send().await.map_err(err)?;
        Ok(match r.status_code() {
            200 | 201 => Copied::Kept,
            s => Copied::Status(s),
        })
    }
}

/// A copy's import call that failed in workerd itself. The copy cannot see why: workerd opens its SQLite file before
/// any of the member's code runs, logs `SQLITE_CANTOPEN` on a line of its own, and answers the call with only "internal
/// error; reference = …". On BF-53 every such failure was the open-file limit (34 passes at the inherited 1,024), so
/// the pass's error names it and the start script that raises it.
fn open_files_error(chain: ChainId, e: &str) -> String {
    if e.contains("SQLITE_CANTOPEN") || e.contains("internal error; reference") {
        let line = format!(
            "lifeboat: the copy of {chain} failed in workerd ({e}): if workerd logged SQLITE_CANTOPEN, the open-file \
             limit is under the bundle's floor; start it with the start.sh `graftyard lifeboat bundle` wrote"
        );
        console_error!("{line}");
        line
    } else {
        format!("import {chain}: {e}")
    }
}

/// One `graft-blob` scrub, or why it did not answer. Each key it could not repair is logged as an error naming the
/// copy to pull it from again.
async fn scrub(live: &Live) -> std::result::Result<ScrubReport, String> {
    let mut init = RequestInit::new();
    init.with_method(Method::Post);
    let req = Request::new_with_init(&format!("{}/_scrub", live.blob), &init).map_err(err)?;
    let mut r = Fetch::Request(req).send().await.map_err(err)?;
    if r.status_code() != 200 {
        return Err(format!("graft-blob scrub: {}", r.status_code()));
    }
    let report = pass::scrubbed(&r.text().await.map_err(err)?);
    for l in &report.lost {
        let from =
            l.copy.map_or_else(|| "no copy names it".to_string(), |c| format!("pull it again from {c}'s entries"));
        console_error!("lifeboat scrub {}: {} is {}; no sibling holds it whole: {from}", l.outcome, l.key, l.outcome);
    }
    Ok(report)
}

/// A repository chain's owner, from the copy of its `Names` shard: `None` for another chain, or one not copied yet.
async fn owner_of(env: &Env, chain: ChainId) -> Option<i64> {
    let (ChainId::Repo(repo) | ChainId::RepoMeta(repo)) = chain else { return None };
    let names = ChainId::Names(graftyard_identity::name_shard(&graftyard_identity::NameKey::Repo(repo)));
    let stub = env.durable_object("MEMBER").ok()?.get_by_name(&graftyard_lookup::lifeboat_object(names)).ok()?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(json!({ "q": { "repoOwner": repo.0.0 } }).to_string().into()));
    let mut r = stub.fetch_with_request(Request::new_with_init("https://member/read", &init).ok()?).await.ok()?;
    let v: Value = r.json().await.ok()?;
    v.pointer("/value/repoOwner").and_then(Value::as_i64)
}

/// The release this bundle decodes: `LIFEBOAT_RELEASE`, hex.
fn release_of(env: &Env) -> Option<[u8; 32]> {
    Digest::parse_hex(env.var("LIFEBOAT_RELEASE").ok()?.to_string().trim()).map(|d| d.0)
}

/// Posts a `LifeboatDecoder` signed with the host's key, bound to `prev`; on a 409 once more to the digest it names.
/// The committed release, or why not.
async fn post_decoder(live: &Live, release: [u8; 32]) -> std::result::Result<(), String> {
    let anchor = graftyard_release::Anchor::from_vars(
        &live.env.var("GRAFTYARD_INSTALL").map_err(err)?.to_string(),
        &live.env.var("GRAFTYARD_CUSTOMER_KEY").map_err(err)?.to_string(),
    )
    .map_err(err)?;
    let mut prev = Digest([0; 32]);
    for _ in 0..2 {
        let mut e = SignedEntry::unsigned(
            Bound { install: anchor.install, group: ChainId::Release, prev: Some(prev), parent: None },
            ReleaseEntry::LifeboatDecoder { host: live.host.clone().into(), release: ReleaseId(release) },
        );
        e.sign(&live.seed);
        let h = Headers::new();
        h.set("content-type", "application/octet-stream").map_err(err)?;
        h.set("user-agent", AGENT).map_err(err)?;
        let mut init = RequestInit::new();
        init.with_method(Method::Post)
            .with_headers(h)
            .with_body(Some(js_sys::Uint8Array::from(e.encode().as_slice()).into()));
        let url = format!("{}{}", live.edge, "/api/graftyard/v1/release/entries");
        let mut r = Fetch::Request(Request::new_with_init(&url, &init).map_err(err)?).send().await.map_err(err)?;
        let status = r.status_code();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        match status {
            201 => return Ok(()),
            409 => prev = v["prev"].as_str().and_then(Digest::parse_hex).ok_or("a 409 without prev")?,
            s => return Err(format!("LifeboatDecoder: {s} {v}")),
        }
    }
    Err("LifeboatDecoder: stale prev twice".into())
}

#[durable_object(alarm)]
pub struct Puller {
    state: State_,
    env: Env,
    busy: RefCell<bool>,
}

type State_ = worker::State;

impl Puller {
    async fn load(&self) -> State {
        self.state
            .storage()
            .get::<Vec<u8>>(STATE_KEY)
            .await
            .ok()
            .flatten()
            .and_then(|b| State::decode(&b).ok())
            .unwrap_or_default()
    }

    /// One pass and, when due, the decoder; then the state is saved. One at a time.
    async fn run(&self) -> std::result::Result<Value, String> {
        if self.busy.replace(true) {
            return Ok(json!({ "busy": true }));
        }
        let out = async {
            let live = Live::load(&self.env)?;
            let mut st = self.load().await;
            let r = pass::pass(&live, &mut st).await?;
            let mut decoder = Value::Null;
            if let Some(rel) = release_of(&self.env)
                && pass::decoder_due(&st, rel, now())
            {
                decoder = match post_decoder(&live, rel).await {
                    Ok(()) => {
                        (st.decoder_sent, st.decoder_at) = (Some(rel), now());
                        json!("committed")
                    }
                    Err(e) => json!(e),
                };
            }
            let mut scrubbed = Value::Null;
            if pass::scrub_due(&st, now()) {
                scrubbed = self.scrub_now(&live, &mut st).await;
            }
            self.state.storage().put(STATE_KEY, st.encode()).await.map_err(err)?;
            let lag = LifeboatStatus::at(ReleaseId(rel_or_zero(&self.env)), st.current_at, now());
            let line = json!({ "lifeboat_pass": r, "lag_s": lag.header(), "decoder": decoder, "halted": st.puller.halted,
                "scrub": scrubbed, "scrub_lost": st.scrub_lost });
            // A halted chain, the lag alarm, a pack unserved on a live chain past `UNSERVED_ALARM_MS`, or a key the
            // last scrub could not repair.
            if !st.puller.halted.is_empty() || lag.alarms() || r.unserved_alarm > 0 || !st.scrub_lost.is_empty() {
                console_error!("{line}");
            } else {
                console_log!("{line}");
            }
            Ok(line)
        }
        .await;
        self.busy.replace(false);
        out
    }
}

impl Puller {
    /// A scrub now: its report, or why it did not answer, which tries again after `pass::SCRUB_RETRY_MS`.
    async fn scrub_now(&self, live: &Live, st: &mut State) -> Value {
        st.scrub_tried = now();
        match scrub(live).await {
            Ok(rep) => {
                (st.scrub_at, st.scrub_lost) = (now(), rep.lost.clone());
                json!(rep)
            }
            Err(e) => {
                console_error!("lifeboat {e}");
                json!(e)
            }
        }
    }
}

fn rel_or_zero(env: &Env) -> [u8; 32] {
    release_of(env).unwrap_or([0; 32])
}

impl DurableObject for Puller {
    fn new(state: worker::State, env: Env) -> Self {
        Self { state, env, busy: RefCell::new(false) }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        // Any call arms the alarm, so the bundle's first request starts the puller.
        if self.state.storage().get_alarm().await?.is_none() {
            self.state.storage().set_alarm(EVERY_MS).await?;
        }
        match (req.method(), req.path().as_str()) {
            (Method::Get, "/status") => {
                let st = self.load().await;
                Response::from_json(&json!({
                    "decoder": hex(&rel_or_zero(&self.env)),
                    "current_at": st.current_at,
                    "halted": st.puller.halted,
                    "held": st.puller.held,
                    "owed": st.owed.len(),
                    "unserved": st.unserved.len(),
                    "unserved_alarm": pass::unserved_alarm(&st, now()),
                    "gone": st.puller.gone,
                    "packs": st.packs,
                    "pages": st.pages,
                    "objects": st.objects,
                    "decoder_at": st.decoder_at,
                    "scrub_at": st.scrub_at,
                    "scrub_lost": st.scrub_lost,
                }))
            }
            (Method::Post, "/pass") => match self.run().await {
                Ok(v) => Response::from_json(&v),
                Err(e) => Response::error(e, 503),
            },
            (Method::Post, "/scrub") => {
                if self.busy.replace(true) {
                    return Response::from_json(&json!({ "busy": true }));
                }
                let out = async {
                    let live = Live::load(&self.env)?;
                    let mut st = self.load().await;
                    let v = self.scrub_now(&live, &mut st).await;
                    self.state.storage().put(STATE_KEY, st.encode()).await.map_err(err)?;
                    Ok::<Value, String>(json!({ "scrub": v, "scrub_lost": st.scrub_lost }))
                }
                .await;
                self.busy.replace(false);
                match out {
                    Ok(v) => Response::from_json(&v),
                    Err(e) => Response::error(e, 503),
                }
            }
            _ => Response::error("not found", 404),
        }
    }

    async fn alarm(&self) -> Result<Response> {
        if let Err(e) = self.run().await {
            console_error!("lifeboat pass failed: {e}");
        }
        self.state.storage().set_alarm(EVERY_MS).await?;
        Response::empty()
    }
}

/// The bundle's front door for the puller: `/_lifeboat/*` reaches `Puller`; everything else is the edge's.
#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let stub = env.durable_object("PULLER")?.get_by_name(graftyard_lifeboat::PULLER_OBJECT)?;
    let rest = url.path().strip_prefix("/_lifeboat").unwrap_or(url.path()).to_string();
    let mut init = RequestInit::new();
    init.with_method(req.method());
    stub.fetch_with_request(Request::new_with_init(&format!("https://puller{rest}"), &init)?).await
}
