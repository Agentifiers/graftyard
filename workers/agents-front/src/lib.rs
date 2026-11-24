//! `graft-agents-front` (`design/AGENT-LAYER.md`, Shape, "Controller", `AgentsAnchor`, `AgentsReleaseView`, and
//! `forward`; `build/BUILD-PLAN.md`, AM3-WP1 and its LF-7 row): the stable Worker on the controller's webhook hostname
//! in the agents account. It forwards every request to `graft-controller` over the `CONTROLLER` `fetch()` service
//! binding, and while its view's `Pointers.controller` is `Some` it sets `Cloudflare-Workers-Version-Overrides` to the
//! pinned version (`plan`). A `None` pointer sets nothing while the view shows a `ControllerRoll` running, so the
//! deployment's percentage split picks the version; any other `None` pins `CONTROLLER_DEFAULT`, N while a roll's split
//! is deployed, so an isolate that has not read the roll never follows the split (DECISIONS "`controller-default`").
//!
//! - **The view.** One `Reader` per isolate: an `AgentsReleaseView` of the install's `Release` group, read over HTTPS
//!   from `GET https://<host>/api/graftyard/v1/release/entries` under a `graftyard-agent` App JWT, never through a
//!   Durable Object binding, because the agents account is another account. Each page is the edge's JSON
//!   (`graftyard_release::agents::parse_page`) and goes through `AgentsReleaseView::extend`, which checks every entry
//!   against the `AgentsAnchor` and never moves the view to a lower index.
//! - **Fail static.** A refresh that fails, or that answers an entry the view refuses, keeps the last view and its pin
//!   and counts as failed on `Freshness`, so the next request refreshes beside itself and none waits on a failing
//!   install (`design/AGENT-LAYER.md`, "Failure modes", the controller's control-plane row).
//! - **Pure here, HTTP in the host.** This file decides; `worker` (wasm32 only) reads the pages, signs the JWT, and
//!   forwards. No path here calls the Cloudflare API.

use graftyard_release::VersionId;
use graftyard_release::agents::{AgentsAnchor, AgentsReleaseView, ENTRIES_PER_PAGE, controller_pin};
use graftyard_release::view::version_override;
use graftyard_release::view::{Freshness, Need};
use graftyard_types::{Index, Timestamp};

#[cfg(target_arch = "wasm32")]
mod worker;

/// One `GET release/entries` page as `parse_page` returns it and `AgentsReleaseView::extend` takes it.
pub type Page = Vec<(Index, Vec<u8>, Timestamp)>;

/// How many pages one refresh reads at most: a cold isolate replays the install's whole history 100 entries at a time,
/// as `graft-front`'s view does (`crates/lookup/src/view.rs`, `PAGES`).
pub const PAGES: usize = 50;

/// The isolate's view of the install's `Release` group, and when to read it again.
#[derive(Clone, Debug, Default)]
pub struct Reader {
    view: AgentsReleaseView,
    fresh: Freshness,
    /// Refreshes that read every page and took them, and refreshes that failed or were refused, in this isolate.
    oks: u64,
    fails: u64,
    last_error: Option<String>,
}

impl Reader {
    pub fn view(&self) -> &AgentsReleaseView {
        &self.view
    }

    pub fn fresh(&self) -> &Freshness {
        &self.fresh
    }

    pub fn oks(&self) -> u64 {
        self.oks
    }

    pub fn fails(&self) -> u64 {
        self.fails
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// What a request arriving at `now` does with the view (`Freshness::need`).
    pub fn need(&self, now: u64) -> Need {
        self.fresh.need(now)
    }

    /// A refresh starts at `now`; `waited` when requests wait for it (`Freshness::start`).
    pub fn start(&mut self, now: u64, waited: bool) {
        self.fresh.start(now, waited);
    }

    /// The refresh requests waited for goes on beside them: the wait ran out (`Freshness::detach`).
    pub fn detach(&mut self) {
        self.fresh.detach();
    }

    /// Extends the view by `pages` in order, without ending the refresh: what `apply` does with a refresh's pages, and
    /// what the host does with the pages a refresh read before it failed, so a long first read that runs out of time
    /// keeps what it verified. The first refusal stops it (`AgentsReleaseView::extend`).
    pub fn take(&mut self, anchor: &AgentsAnchor, pages: &[Page]) -> Result<(), String> {
        pages.iter().try_for_each(|page| {
            self.view.extend(anchor, page).map(|_| ()).map_err(|e| format!("{}: {e}", e.variant()))
        })
    }

    /// The end of the refresh that started at `start`, at `now`: the pages it read in order, or why it read none.
    /// Each page goes through `extend`, so entries the view holds add nothing and the view never moves to a lower
    /// index. A failed read, or a page whose first new entry the view refuses, is a failed refresh: the view keeps what
    /// it held (with any entries `extend` took before the refusal, each of which passed the group's check), and the
    /// next one is tried after `RETRY_MS` beside requests, never before them.
    pub fn apply(&mut self, anchor: &AgentsAnchor, pages: Result<Vec<Page>, String>, start: u64, now: u64) {
        match pages.and_then(|pages| self.take(anchor, &pages)) {
            Ok(()) => {
                self.fresh.ok(start);
                self.oks += 1;
            }
            Err(e) => {
                self.fresh.failed(now);
                self.fails += 1;
                self.last_error = Some(e);
            }
        }
    }
}

/// The `after` of the next page of a refresh that read `page` after `after`: `None` once a page is shorter than
/// `ENTRIES_PER_PAGE`, the edge's last, or when it does not move past `after`, so a refresh never reads the same page
/// twice.
pub fn next_after(after: Index, page: &Page) -> Option<Index> {
    let last = page.last()?.0;
    (page.len() >= ENTRIES_PER_PAGE && last > after).then_some(last)
}

/// The pin and `Cloudflare-Workers-Version-Overrides` value of a forwarded request (`graftyard_release::agents::
/// controller_pin`): `Pointers.controller` while it is set; the deployment's split while a verified view shows a
/// `ControllerRoll` running; otherwise `default`, the script's `CONTROLLER_DEFAULT`, which the agents `graft-ops` sets
/// to N while a roll's split is deployed, so an isolate that has not read the roll yet never reaches N+1.
pub fn plan(
    view: &AgentsReleaseView,
    default: Option<VersionId>,
    controller_script: &str,
) -> (Option<VersionId>, Option<String>) {
    let pin = controller_pin(view, default);
    (pin, pin.map(|v| version_override(controller_script, v)))
}
