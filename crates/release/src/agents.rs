//! The agents account's view of the install's `Release` group (`design/AGENT-LAYER.md`, Shape, "Controller",
//! `AgentsAnchor`, `AgentsReleaseView`, and `forward`; AM3-WP1), pure, so `graft-agents-front` and the agents
//! `graft-ops` run the same rules and LF-8 is tested here.
//!
//! - **The feed.** `GET /api/graftyard/v1/release/entries?after=` answers the install's committed entries in index
//!   order, as stored, each with its index and the time of the entry it committed in (`feed`). Index 1 is the genesis.
//! - **The view.** `AgentsReleaseView` replays them through the group's own `check` and `apply` against the
//!   `AgentsAnchor`: an entry is taken only at the next index, only when bound to the anchored install, and only under
//!   the `Keys` chain that starts at a genesis signed by the anchored customer key. So an entry for another install, a
//!   `Keys` not signed by the customer key of the latest accepted `Keys`, and an entry out of index order are refused,
//!   and a refused entry moves nothing (`extend`).
//! - **Never backward, fail static.** A page that starts past the view is a gap, and a page whose entries the view
//!   already holds adds nothing, so no answer moves the view to a lower index. A refresh that fails keeps the last view;
//!   `Freshness` (from `view`) says when to try again and that no request waits on it.
//! - **The pin.** `controller_override` is the override `graft-agents-front` sets on `graft-controller`: `Some` only
//!   while `Pointers.controller` is `Some`, so a `None` pointer follows the deployment split (the base's pin-only rule).
//!
//! The view replays every entry, not only the agents' kinds, because the `Keys`, `prev`, and intent rules are the
//! group's and depend on the whole history; the endpoint therefore serves every committed entry, and the agents
//! account learns no more than the entries' signed bytes, which carry no secret.

use graftyard_types::{Index, Timestamp};

use crate::group::Accepted;
use crate::types::{IntentId, KeyId, Keys, Script, VersionId};
use crate::view::version_override;
use crate::{Anchor, InstallId, Refusal, ReleaseFold};

/// What `graftyard agents init` sets as script variables on `graft-agents-front` and the agents `graft-ops`: the
/// install's trust anchor, as the install's own scripts carry it, and the install's hostname, where the view is read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentsAnchor {
    pub install: InstallId,
    pub customer: KeyId,
    pub host: Box<str>,
}

impl AgentsAnchor {
    fn anchor(&self) -> Anchor {
        Anchor { install: self.install, customer: self.customer }
    }
}

/// The part of the install's `Release` fold the agents account acts on: the fold it replayed, from which `index`,
/// `keys`, `controller`, and `current` are read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentsReleaseView {
    fold: ReleaseFold,
}

/// How many entries one `GET release/entries` page carries at most (`design/AGENT-LAYER.md`, "API reference": lists
/// take `per_page`, default 30, at most 100).
pub const ENTRIES_PER_PAGE: usize = 100;

impl AgentsReleaseView {
    /// The index of the last entry taken: the `after` of the next read.
    pub fn index(&self) -> Index {
        Index(self.fold.count)
    }

    /// The latest accepted `Keys`, once the genesis is taken.
    pub fn keys(&self) -> Option<&Keys> {
        self.fold.keys.as_ref()
    }

    /// `Pointers.controller`: `Some` only after a pre-signed `ControllerPointer` undo, until the next `ControllerRoll`.
    pub fn controller(&self) -> Option<VersionId> {
        self.fold.pointers.get(Script::Controller)
    }

    /// The intent the group is running, if any.
    pub fn current(&self) -> Option<IntentId> {
        self.fold.current.as_deref().map(|c| c.intent.id)
    }

    /// The fold the view replayed, for the agents `graft-ops`, which runs `ops::next` on it.
    pub fn fold(&self) -> &ReleaseFold {
        &self.fold
    }

    /// Takes a page of `(index, stored bytes, entry time)` that the edge answered, in order. A page that starts at or
    /// before the view's index comes from a member behind it: the entries the view holds are skipped, never replayed
    /// over it, so no answer moves the view to a lower index. Every other entry must be the next index, bound to the
    /// anchored install, and pass the group's `check` on the view's fold, whose `Keys` chain began at a genesis signed
    /// by the anchored customer key. The first entry that fails stops the page with its refusal, and the view keeps
    /// what it held, with the entries before it. Returns how many entries it took.
    pub fn extend(&mut self, anchor: &AgentsAnchor, page: &[(Index, Vec<u8>, Timestamp)]) -> Result<u64, Refusal> {
        let a = anchor.anchor();
        let held = self.fold.count;
        let mut n = 0;
        for (index, bytes, at) in page {
            if index.0 <= held {
                continue;
            }
            if index.0 != self.fold.count + 1 {
                return Err(Refusal::BadState("a release/entries page skips an index"));
            }
            let ev = Accepted { bytes: bytes.clone().into_boxed_slice() };
            self.fold.check(&a, *at, &ev)?;
            self.fold.apply(&ev, *at);
            n += 1;
        }
        Ok(n)
    }
}

/// The page `GET release/entries?after=<after>&per_page=<per_page>` answers: `entries` are the group's committed
/// entries from index `after + 1`, as a member's `/view` pages them from position `after`, with their times; the page
/// is the first `per_page` of them (1 to `ENTRIES_PER_PAGE`), each with its index.
pub fn feed(after: u64, entries: &[(Vec<u8>, Timestamp)], per_page: usize) -> Vec<(Index, &[u8], Timestamp)> {
    entries
        .iter()
        .take(per_page.clamp(1, ENTRIES_PER_PAGE))
        .enumerate()
        .map(|(i, (b, at))| (Index(after + 1 + i as u64), b.as_slice(), *at))
        .collect()
}

/// The edge's answer to `GET release/entries` (`workers/edge/src/release.rs`, `read`): a JSON array of `{"index",
/// "entry", "at"}`, `entry` the stored bytes in standard base64, as `extend` takes them. Anything else is an error,
/// which a reader treats as a failed refresh.
pub fn parse_page(body: &[u8]) -> Result<Vec<(Index, Vec<u8>, Timestamp)>, String> {
    use base64::Engine as _;
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| format!("not JSON: {e}"))?;
    let rows = v.as_array().ok_or("not an array of entries")?;
    rows.iter()
        .map(|r| {
            let index = r["index"].as_u64().ok_or("an entry with no index")?;
            let at = r["at"].as_u64().ok_or("an entry with no at")?;
            let b = r["entry"].as_str().ok_or("an entry with no bytes")?;
            let bytes =
                base64::engine::general_purpose::STANDARD.decode(b).map_err(|e| format!("entry {index}: {e}"))?;
            Ok((Index(index), bytes, Timestamp(at)))
        })
        .collect()
}

/// The `Cloudflare-Workers-Version-Overrides` value `graft-agents-front` sets on its call to `graft-controller`:
/// `Some` only while the view's `Pointers.controller` is `Some`, so `None` follows the deployment split.
pub fn controller_override(view: &AgentsReleaseView, controller_script: &str) -> Option<String> {
    view.controller().map(|v| version_override(controller_script, v))
}
