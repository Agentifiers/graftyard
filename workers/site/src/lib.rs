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

mod audit;
mod family;
#[cfg(feature = "test-faults")]
mod faults;
mod host;
mod lookup;
mod member;
#[cfg(feature = "test-faults")]
mod register;
mod registry;
mod release;
mod shard;
mod snap;
mod sweep;
mod tier;

pub use member::Member;
pub use registry::Registry;
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

/// No public route. A request that reaches the script's fetch handler is refused.
#[event(fetch)]
async fn fetch(_req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    Response::error("not found", 404)
}

/// The site's Cron Trigger: the pack sweeper (`crate::sweep`), then the fold audit (`crate::audit`). Reports go to the
/// script's logs.
#[event(scheduled)]
async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    match sweep::run(&env).await {
        Ok(r) => console_log!("sweep {r}"),
        Err(e) => console_error!("sweep failed: {e}"),
    }
    match audit::run(&env).await {
        Ok(r) => console_log!("fold audit {r}"),
        Err(e) => console_error!("fold audit failed: {e}"),
    }
}

pub(crate) fn now_ms() -> u64 {
    Date::now().as_millis()
}
