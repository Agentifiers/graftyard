//! One site script (`design/DESIGN.md`, Shape, "Sites, not services, are the failure unit"). The same source builds
//! `graft-site-a`, `-b`, and `-c`; `SITE_ID` and `GRAFTYARD_SITES` in each script's version say which site it is and
//! where every other site's classes live. A site script has no public route: callers reach its classes through
//! Durable Object bindings with `script_name`.

#[cfg(feature = "test-faults")]
use graftyard_types::{ChainId, PlacementRecord};
use graftyard_types::{SiteId, SiteTable};
#[cfg(feature = "test-faults")]
use serde::Deserialize;
use worker::*;

mod actions;
mod actions_trigger;
mod activity;
mod alarm_steps;
mod archive;
mod audit;
#[cfg(feature = "test-faults")]
mod b030;
mod broker;
mod callers;
mod clock;
mod export_feed;
mod family;
mod fanout;
#[cfg(feature = "test-faults")]
mod faults;
mod feed;
mod fold_audit;
mod gatecount;
mod heapcount;
mod hookseq;
mod host;
mod inbox;
mod lifeboat;
mod live;
mod load;
mod logread;
mod logrelay;
mod lookup;
mod member;
mod merge_queue;
mod openbytes;
mod prsync;
// Also in unit tests, which drive a member over its fold (`host::tests`).
#[cfg(any(test, feature = "test-faults"))]
#[cfg_attr(not(feature = "test-faults"), allow(dead_code))]
mod register;
mod registry;
mod release;
mod replace;
mod retire;
mod search;
mod shard;
mod snap;
mod split;
#[cfg(feature = "test-faults")]
mod sqlprobe;
mod sweep;
mod tier;
mod txn;

pub use broker::RunnerBroker;
pub use hookseq::HookSequencer;
pub use inbox::Inbox;
pub use logrelay::LogRelay;
pub use member::Member;
pub use registry::Registry;
pub use search::SearchShard;
pub use shard::GitShard;

/// The site this script is and the install's site table, from the script's own version. The system chains' records
/// come from the `Release` view (`crate::release::system_record`).
pub(crate) struct SiteEnv {
    pub id: SiteId,
    pub sites: SiteTable,
    #[cfg(feature = "test-faults")]
    pub genesis: Genesis,
}

/// Test installs only: the system chains' records from the `GRAFTYARD_GENESIS` variable, which `deploy.mjs --genesis`
/// sets from a test config (`tests/m1/wp7/genesis.json`), for installs that run no `Release` group. Product builds
/// read the records only from the genesis the `Release` group accepted.
#[cfg(feature = "test-faults")]
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Genesis {
    #[serde(default)]
    pub names: Option<PlacementRecord>,
    #[serde(default)]
    pub tokens: Option<PlacementRecord>,
    #[serde(default)]
    pub ids: Option<PlacementRecord>,
}

impl SiteEnv {
    pub fn read(env: &Env) -> Result<SiteEnv> {
        let id: u8 = env.var("SITE_ID")?.to_string().parse().map_err(|_| Error::RustError("SITE_ID".into()))?;
        let sites: SiteTable = serde_json::from_str(&env.var("GRAFTYARD_SITES")?.to_string())?;
        if sites.get(SiteId(id)).is_none() {
            return Err(Error::RustError(format!("SITE_ID {id} is not in GRAFTYARD_SITES")));
        }
        #[cfg(feature = "test-faults")]
        let genesis = match env.var("GRAFTYARD_GENESIS") {
            Ok(v) => serde_json::from_str(&v.to_string())?,
            Err(_) => Genesis::default(),
        };
        Ok(SiteEnv {
            id: SiteId(id),
            sites,
            #[cfg(feature = "test-faults")]
            genesis,
        })
    }

    /// Test installs only: a system chain's record from `GRAFTYARD_GENESIS`, if the variable is set.
    #[cfg(feature = "test-faults")]
    pub fn genesis_shortcut(&self, chain: ChainId) -> Option<PlacementRecord> {
        match chain {
            ChainId::Names(_) => self.genesis.names.clone(),
            ChainId::Tokens(_) => self.genesis.tokens.clone(),
            ChainId::Ids => self.genesis.ids.clone(),
            _ => None,
        }
    }

    /// The member-class binding for `site`: its own class here, or the other script's class bound with
    /// `script_name`. Every `get()` passes the site's hint, because only the first call respects it.
    pub fn member(&self, env: &Env, site: SiteId, do_name: &str) -> Result<Stub> {
        let region = self.sites.get(site).ok_or_else(|| Error::RustError(format!("unknown site {site:?}")))?.region;
        env.durable_object(&format!("MEMBER_{}", site.0))?.get_by_name_with_location_hint(do_name, region.hint())
    }

    pub fn registry(&self, env: &Env, site: SiteId, do_name: &str) -> Result<Stub> {
        let region = self.sites.get(site).ok_or_else(|| Error::RustError(format!("unknown site {site:?}")))?.region;
        env.durable_object(&format!("REGISTRY_{}", site.0))?.get_by_name_with_location_hint(do_name, region.hint())
    }
}

/// No public route: `workers_dev` and preview URLs are off, so only this script's own `SITE` binding reaches the handler,
/// with a `PrSync` first attempt (`prsync::in_band`) or an archive promotion (`archive::in_band`). Anything else is
/// refused.
#[event(fetch)]
async fn fetch(req: Request, env: Env, ctx: Context) -> Result<Response> {
    if site_down(&env) {
        return Response::error("site down", 503);
    }
    if req.method() == Method::Post && req.path() == prsync::FIRST_PATH {
        return prsync::first_handled(req, env, &ctx).await;
    }
    if req.method() == Method::Post && req.path() == archive::PROMOTE_PATH {
        return archive::promote_handled(req, env, &ctx).await;
    }
    Response::error("not found", 404)
}

/// The site's Cron Trigger: the pack sweeper (`crate::sweep`), then the fold audit (`crate::fold_audit`). Reports go to the
/// script's logs.
#[event(scheduled)]
async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    if site_down(&env) {
        return;
    }
    match sweep::run(&env).await {
        Ok(r) => console_log!("sweep {r}"),
        Err(e) => console_error!("sweep failed: {e}"),
    }
    match fold_audit::run(&env, None, false).await {
        Ok(r) => console_log!("fold audit {r}"),
        Err(e) => console_error!("fold audit failed: {e}"),
    }
    // The change feed's kick: a round, which keeps itself going every FEED_EVERY_MS through the queue. Only the site
    // that binds the export bucket runs it.
    export_feed::kick(&env).await;
}

/// The site's queue: `PrSync` and `MergeQueue` batches its repository leaders sent again (`crate::prsync`,
/// `crate::merge_queue`). A `MergeQueue` message carries `mergeQueue`, the change feed's next round `exportFeed`
/// (`crate::export_feed`); every other is a `PrSync` batch.
#[event(queue)]
async fn queue(batch: MessageBatch<serde_json::Value>, env: Env, _ctx: Context) -> Result<()> {
    if site_down(&env) {
        batch.retry_all();
        return Ok(());
    }
    prsync::consume(batch, env).await
}

/// `GRAFTYARD_SITE_DOWN=1`: the operator's whole-site stop. Every class of this script answers 503 to every call and
/// does no alarm work, and the script's own fetch, queue, and cron handlers do nothing, as if the site were lost; the
/// objects keep their storage. It is set and cleared only by a site day (`workers/site/deploy.mjs --vars`), the
/// production deploy path, so no request path reaches it. BF-58 kills the site holding a `HookSequencer` with it.
pub(crate) fn site_down(env: &Env) -> bool {
    env.var("GRAFTYARD_SITE_DOWN").is_ok_and(|v| v.to_string() == "1")
}

pub(crate) fn now_ms() -> u64 {
    #[cfg(target_arch = "wasm32")]
    return Date::now().as_millis();
    // Unit tests run `Family::stepped` and other input-time code natively, where `Date` panics.
    #[cfg(not(target_arch = "wasm32"))]
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod isolate_state {
    /// `lookup-poll` and `feed-per-member`: a Workers isolate hosts many requests and many Durable Objects, and runs a
    /// woken future in the context of whatever wakes it. Isolate-wide state (`thread_local!`) that holds a waker, a
    /// future, a JS object, or a `oneshot::Sender` (directly or through a struct that holds one) carries one object's
    /// I/O into another's, as `FEEDS` keyed by repository did on B0-29 m3fp3. Such state may exist only keyed by one
    /// object instance: each entry below names its static and the start of its declared type, which holds that key.
    /// A new static of this kind fails here until it is keyed per object and added.
    const PER_OBJECT: &[(&str, &str)] = &[
        // Member object name and boot (`crate::feed`).
        ("FEEDS", "RefCell<BTreeMap<Box<str>, Kept>>"),
        // Shard object instance (`GitShard::run_key`) and network.
        ("RUNS", "std::cell::RefCell<std::collections::HashMap<(String, i64), Run>>"),
        ("COMPOSES", "std::cell::RefCell<std::collections::HashMap<(String, i64), ComposeRun>>"),
    ];
    const BANNED: &[&str] = &["Waker", "Future", "Promise", "Stream", "Closure", "Request", "Response", "Stub", "Pin<"];

    fn sources() -> Vec<(String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(d).expect("src").flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push((p.display().to_string(), std::fs::read_to_string(&p).expect("a source file")));
                }
            }
        }
        out
    }

    /// Every struct whose fields hold a `oneshot::Sender`, with its file: a static's type names a struct of its own
    /// file or a `crate::` path (`crate::feed::Feed`).
    fn sender_holders(srcs: &[(String, String)]) -> Vec<(String, String)> {
        let mut names = Vec::new();
        for (f, s) in srcs {
            let mut rest = s.as_str();
            while let Some(i) = rest.find("struct ") {
                let after = &rest[i + 7..];
                let name: String = after.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                let body = after.find("\n}\n").map_or(after, |end| &after[..end]);
                if !name.is_empty() && body.find('{').is_some_and(|b| body[b..].contains("Sender<")) {
                    names.push((f.clone(), name));
                }
                rest = &after[1..];
            }
        }
        // A `type` alias over such a struct holds one too (`crate::feed`'s `Kept`).
        for (f, s) in srcs {
            for line in s.lines() {
                let Some(def) = line.trim_start().strip_prefix("type ") else { continue };
                let Some((name, rhs)) = def.split_once(" = ") else { continue };
                if rhs.contains("Sender<") || names.iter().any(|(nf, n)| nf == f && holds(rhs, n)) {
                    names.push((f.clone(), name.trim().to_owned()));
                }
            }
        }
        names
    }

    /// Every `static NAME: TYPE =` inside a `thread_local!` block, with its file.
    fn statics(srcs: &[(String, String)]) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for (f, s) in srcs {
            let mut rest = s.as_str();
            while let Some(i) = rest.find("thread_local!") {
                let body = &rest[i..];
                let end = body.find("\n}\n").or_else(|| body.find(");\n")).unwrap_or(body.len());
                let block = &body[..end];
                let mut b = block;
                while let Some(j) = b.find("static ") {
                    let decl = &b[j + 7..];
                    let (Some(colon), Some(eq)) = (decl.find(':'), decl.find(" =")) else { break };
                    if colon < eq {
                        let name = decl[..colon].trim().to_owned();
                        let ty = decl[colon + 1..eq].split_whitespace().collect::<Vec<_>>().join(" ");
                        out.push((f.clone(), name, ty));
                    }
                    b = &decl[eq.max(1)..];
                }
                rest = &body[end.max(1)..];
            }
        }
        out
    }

    /// The module of a `crate::<module>::<word>` path in `ty`.
    fn ty_module<'a>(ty: &'a str, word: &str) -> Option<&'a str> {
        let i = ty.find(&format!("::{word}"))?;
        let path = &ty[..i];
        let start = path.rfind("crate::")? + 7;
        Some(&path[start..]).filter(|m| !m.contains("::") && !m.is_empty())
    }

    fn holds(ty: &str, word: &str) -> bool {
        ty.match_indices(word).any(|(i, _)| {
            let before = ty[..i].chars().last();
            let after = ty[i + word.len()..].chars().next();
            !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
                && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
    }

    #[test]
    fn no_isolate_state_carries_one_objects_io_into_another() {
        let srcs = sources();
        let holders = sender_holders(&srcs);
        assert!(holders.iter().any(|(_, h)| h == "Feed"), "the scan finds `Feed`'s waiters: {holders:?}");
        let found = statics(&srcs);
        assert!(found.len() >= 40, "found {} isolate-wide statics", found.len());
        let mut seen = Vec::new();
        for (file, name, ty) in &found {
            let carries = BANNED.iter().find(|b| ty.contains(**b)).copied().or_else(|| {
                let named = |(f, h): &(String, String)| {
                    (f == file && holds(ty, h)) || f.ends_with(&format!("{}.rs", ty_module(ty, h).unwrap_or("-")))
                };
                (ty.contains("Sender<") || holders.iter().any(named)).then_some("oneshot::Sender")
            });
            let Some(what) = carries else { continue };
            match PER_OBJECT.iter().find(|(n, _)| n == name) {
                Some((_, want)) => {
                    assert!(ty.starts_with(want), "{file}: {name} holds a {what} but is not keyed per object: {ty}");
                    seen.push(name.as_str());
                }
                None => panic!("{file}: isolate-wide {name} holds a {what}, keyed by no object instance: {ty}"),
            }
        }
        for (n, _) in PER_OBJECT {
            assert!(seen.contains(n), "{n} is listed but was not found holding a sender");
        }
    }
}
