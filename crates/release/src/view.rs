//! The cached `Release` view (`design/DESIGN.md`, Shape, \"Release group\", `Pointers`, `POINTER_TTL`, `STALE_MAX`):
//! what every `graft-front`, edge, and site isolate, members included, holds of the group. Pure, so its rules are
//! tested here and every isolate runs the same code.
//!
//! - **A replica, not a copy.** An isolate holds its own `ReleaseFold`, built by replaying the group's committed
//!   entries through the same `check` and `apply` a follower runs, against the isolate's own trust anchor. A member
//!   that answers with another install's entries, or bytes that do not verify, moves nothing.
//! - **Never backward.** Pages are read from any member's applied state by position; a member behind the replica
//!   adds nothing, and a page that starts past the replica's count is a gap, refused.
//! - **Fail static.** A refresh that fails keeps the last view and is retried after `RETRY_MS`, not a full
//!   `POINTER_TTL`, and no request waits on it (`Freshness`).
//! - **The copy outside the group.** Each member writes the committed entries it applied to the `Release` chain's
//!   `MemberRegistry` object on its own site. A cold isolate whose first refresh has no answer reads the copies for COPY_MS
//!   and takes one only through `adopt`: every entry checked against its own anchor, all or nothing, and never
//!   shorter than what it holds, so a stale copy cannot roll a view back and the group's view replaces it.
//! - **Uncommitted `StaleMode`.** A flag the group could not commit reaches isolates through members' `pending`
//!   rows. Each isolate verifies it again against its own cached `Keys` and honors it only while `until` is in the
//!   future and at most `STALE_MAX` after the flag's first arrival.

use std::collections::BTreeMap;

use graftyard_types::{AccountId, ChainId, Digest, Timestamp};

use crate::group::{Accepted, STALE_MAX};
use crate::types::{KeyId, ReleaseEntry, Scope, Script, VersionId};
use crate::{Anchor, Refusal, ReleaseFold, SignedEntry, formats_enable_message};

/// How long an isolate serves its view before it refreshes it (`design/DESIGN.md`, Shape, `POINTER_TTL`) [guess].
pub const POINTER_TTL_MS: u64 = 5_000;
/// How soon a failed refresh is tried again: 1 s rather than a full POINTER_TTL [inference].
pub const RETRY_MS: u64 = 1_000;
/// The longest a request waits for a view, and one member call's deadline in a refresh [guess].
pub const VIEW_CALL_MS: u64 = 1_000;
/// A refresh beside requests still running after this long is replaced: three members at VIEW_CALL_MS each.
pub const ABANDON_MS: u64 = 4 * VIEW_CALL_MS;
/// The deadline of a cold isolate's read of the copies of the group's entries that the members keep, one per site, in
/// the `Release` chain's own `MemberRegistry` object on that site (`design/DESIGN.md`, Shape, `POINTER_TTL`). The read
/// asks every site at once and takes what answered by then. An idle Durable Object round trip between regions is 111 to
/// 135 ms at p50 with p99 within 10 ms of it [measured, `tests/m0/results/m0-wp2.md`, "Transport"], so the near
/// site's copy, and normally all three, answer well inside it; BF-8 run 4's 277 isolates read them in 81 ms at p50 and
/// 139 ms at most [measured, `tests/m5/results/m5-wp2.md`]. It is half of VIEW_CALL_MS, the wait a cold request already
/// had, so the read adds no wait [inference].
pub const COPY_MS: u64 = 500;

/// The `Cloudflare-Workers-Version-Overrides` value that pins `script` to `v`: a Structured Fields dictionary whose
/// value is a String (https://developers.cloudflare.com/workers/versions-and-deployments/version-overrides/).
pub fn version_override(script: &str, v: VersionId) -> String {
    format!("{script}=\"{v}\"")
}

/// The one `Cloudflare-Workers-Version-Overrides` value `graft-front` sends for the scripts behind it: a dictionary
/// member per pinned script, by script name, and nothing for a `None` pointer, so the deployment's split applies to it
/// (`design/AGENT-LAYER.md`, Shape, "graft-mcp", release control). `None` when no script is pinned. One header carries
/// both pointers because `graft-mcp` forwards the header it got on each edge call, so a pinned edge stays pinned
/// behind it.
pub fn overrides(pins: &[(&str, Option<VersionId>)]) -> Option<String> {
    let members: Vec<String> = pins.iter().filter_map(|(s, p)| p.map(|v| version_override(s, v))).collect();
    (!members.is_empty()).then(|| members.join(", "))
}

/// The script `graft-front` sends a request to by its `Host`: `graft-mcp` for `mcp_host` (`mcp.<host>`), compared as
/// DNS names are, without case or port, and the edge for every other host, so an install with no `mcp_host` routes
/// everything to the edge as before.
pub fn front_route(host: &str, mcp_host: Option<&str>) -> Script {
    let name = |h: &str| h.split(':').next().unwrap_or_default().trim_end_matches('.').to_ascii_lowercase();
    match mcp_host {
        Some(m) if !m.is_empty() && name(host) == name(m) => Script::Mcp,
        _ => Script::Edge,
    }
}

/// What `graft-front` does with one request (`forward`): the script it goes to, the pin its version report is checked
/// against (`mismatch`), and the override header it carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forward {
    pub to: Script,
    pub pin: Option<VersionId>,
    pub overrides: Option<String>,
}

/// `graft-front`'s plan for a request to `host`, from its view's pointers (`pin`). `mcp` is `graft-mcp`'s host and
/// script name when the deployment puts it behind this front, else `None`, and then `Pointers.mcp` is not read. The
/// header names every pinned script whichever one the request goes to, because `graft-mcp` forwards it to the edge.
pub fn forward(
    host: &str,
    edge_script: &str,
    mcp: Option<(&str, &str)>,
    pin: impl Fn(Script) -> Option<VersionId>,
) -> Forward {
    let to = front_route(host, mcp.map(|(h, _)| h));
    let edge = pin(Script::Edge);
    let (mcp_script, mcp_pin) = match mcp {
        Some((_, s)) => (s, pin(Script::Mcp)),
        None => ("", None),
    };
    Forward {
        to,
        pin: if to == Script::Mcp { mcp_pin } else { edge },
        overrides: overrides(&[(edge_script, edge), (mcp_script, mcp_pin)]),
    }
}

/// Whether `graft-front` alarms on an answer: the script that answered is pinned to `pin` and reported another
/// version, or none. An unpinned call follows the deployment split, so any version it reports is one the deployment
/// serves.
pub fn mismatch(pin: Option<VersionId>, ran: Option<&str>) -> bool {
    match pin {
        None => false,
        Some(v) => ran.and_then(|r| r.parse::<VersionId>().ok()) != Some(v),
    }
}

/// What a request does with the isolate's view (`Freshness::need`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// Serve the view held.
    Use,
    /// Serve the view held and refresh it beside the request.
    Refresh,
    /// Refresh before serving, or join the refresh another request of this isolate is waiting for; either way for
    /// VIEW_CALL_MS at most, then serve whatever is held.
    Wait,
}

/// When an isolate's view was read, and whether its refresh is failing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Freshness {
    /// When the last refresh that reached a member started: the view is at least this new.
    pub fetched: Option<u64>,
    /// The earliest time the next refresh may start.
    pub next: u64,
    /// The last refresh reached no member.
    pub failing: bool,
    /// The refresh in flight: when it started, and whether requests wait for it.
    pub running: Option<(u64, bool)>,
}

impl Freshness {
    /// - A busy isolate refreshes beside its requests once the view is POINTER_TTL - RETRY_MS old, so a pin reaches
    ///   each of its requests within POINTER_TTL of its commit and no request waits.
    /// - A refresh beside requests that has not answered leaves its requests on the view held; one that has not
    ///   answered after ABANDON_MS is replaced, and no request waits for its replacement.
    /// - A request waits only when the view is POINTER_TTL old or missing and the last refresh did not fail: the
    ///   first request of a cold isolate, or of one idle that long. Requests that arrive while it waits wait for the
    ///   same refresh.
    /// - Once a refresh fails, requests serve the view held (or none) and the refresh is retried every RETRY_MS
    ///   beside them, so no request waits on a group that does not answer.
    pub fn need(&self, now: u64) -> Need {
        match self.running {
            Some((_, true)) => return Need::Wait,
            Some((r, false)) if now < r + ABANDON_MS => return Need::Use,
            // Hung past ABANDON_MS: a failing group, not one to wait for.
            Some((_, false)) => return Need::Refresh,
            None => {}
        }
        if now < self.next {
            return Need::Use;
        }
        if self.failing {
            return Need::Refresh;
        }
        match self.fetched {
            Some(f) if now < f + POINTER_TTL_MS => Need::Refresh,
            _ => Need::Wait,
        }
    }

    /// A refresh starts at `now`; `waited` when requests wait for it.
    pub fn start(&mut self, now: u64, waited: bool) {
        self.running = Some((now, waited));
    }

    /// A refresh that started at `start` reached a member.
    pub fn ok(&mut self, start: u64) {
        self.fetched = Some(self.fetched.map_or(start, |f| f.max(start)));
        self.next = start + POINTER_TTL_MS - RETRY_MS;
        self.failing = false;
        self.running = None;
    }

    /// The refresh requests wait for goes on beside them instead: a cold isolate took a site's copy, or the wait ran
    /// out. Requests serve what is held, and the group's view replaces it once a member answers.
    pub fn detach(&mut self) {
        if let Some((r, true)) = self.running {
            self.running = Some((r, false));
        }
    }

    /// A refresh reached no member by `now`: the view held stays.
    pub fn failed(&mut self, now: u64) {
        self.next = now + RETRY_MS;
        self.failing = true;
        self.running = None;
    }
}

/// A `StaleMode` flag that has not committed, as a member's `pending` row carried it and this replica verified it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingFlag {
    pub scope: Scope,
    pub until: Timestamp,
    /// The earliest arrival any member recorded.
    pub arrived: Timestamp,
}

impl PendingFlag {
    /// The flag is honored before this time: `until`, and at most `stale_max` (STALE_MAX) after its arrival.
    pub fn honored_until(&self, stale_max: u64) -> Timestamp {
        Timestamp(self.until.0.min(self.arrived.0.saturating_add(stale_max)))
    }
}

/// An isolate's replica of the `Release` group.
#[derive(Clone, Debug)]
pub struct Replica {
    pub fold: ReleaseFold,
    /// Uncommitted `StaleMode` flags, by the digest of their stored form.
    pub pending: BTreeMap<Digest, PendingFlag>,
    /// STALE_MAX. Only a test install lowers it (`GRAFTYARD_STALE_MAX_MS`, test-faults site builds), so a hosted run
    /// sees the bound end a flag within the run.
    pub stale_max: u64,
}

impl Default for Replica {
    fn default() -> Replica {
        Replica { fold: ReleaseFold::default(), pending: BTreeMap::new(), stale_max: STALE_MAX }
    }
}

/// Whether `scope` covers `chain`. `owner` is the chain's owner where the caller knows it (a member knows its
/// repository's); `None` covers every repository chain under an `Owner` scope, so a caller that does not know the
/// owner asks the member, which does.
pub fn covers(scope: Scope, chain: ChainId, owner: Option<AccountId>) -> bool {
    match (scope, chain) {
        (_, ChainId::Release) => false,
        (Scope::Install, _) => true,
        (Scope::Repo(r), ChainId::Repo(c) | ChainId::RepoMeta(c)) => r == c,
        (Scope::Repo(_), _) => false,
        (Scope::Owner(a), ChainId::Authz(o) | ChainId::Identity(o) | ChainId::Actions(o, _)) => a == o,
        (Scope::Owner(a), ChainId::Repo(_) | ChainId::RepoMeta(_) | ChainId::Network(_)) => {
            owner.is_none_or(|o| o == a)
        }
        (Scope::Owner(_), _) => false,
    }
}

impl Replica {
    /// Signed entries applied, genesis included: the position the next page starts at.
    pub fn count(&self) -> u64 {
        self.fold.count
    }

    /// Applies the committed entries of a page that starts at position `start`, each with the time of the entry it
    /// committed in, after `check` accepts it on this replica's fold. Entries the replica already holds are skipped,
    /// so a member behind the replica moves nothing. Returns how many were applied; an entry `check` refuses stops the
    /// page and keeps what was applied before it.
    pub fn extend(&mut self, anchor: &Anchor, start: u64, entries: &[(Vec<u8>, Timestamp)]) -> Result<u64, Refusal> {
        if start > self.fold.count {
            return Err(Refusal::BadState("a view page starts past the replica"));
        }
        let skip = (self.fold.count - start) as usize;
        let mut n = 0;
        for (bytes, at) in entries.iter().skip(skip) {
            let ev = Accepted { bytes: bytes.clone().into_boxed_slice() };
            self.fold.check(anchor, *at, &ev)?;
            self.fold.apply(&ev, *at);
            n += 1;
        }
        Ok(n)
    }

    /// Takes a site's copy of the group's committed entries from position 0, as the members wrote it: only when every
    /// entry the replica lacks passes `check`, so a copy with a bad or unsigned entry moves nothing, and a copy no
    /// longer than the replica adds nothing, so an older copy never rolls a newer view back. Returns how many entries
    /// it added.
    pub fn adopt(&mut self, anchor: &Anchor, copy: &[(Vec<u8>, Timestamp)]) -> Result<u64, Refusal> {
        let mut next = self.clone();
        let n = next.extend(anchor, 0, copy)?;
        if n > 0 {
            *self = next;
        }
        Ok(n)
    }

    /// The pinned version of `script`, or `None` to follow the deployment split.
    pub fn pin(&self, script: Script) -> Option<VersionId> {
        self.fold.pointers.get(script)
    }

    /// Verifies an uncommitted `StaleMode` against this replica's `Keys` and keeps it, with the earliest arrival seen
    /// for the same bytes. Flags past `until` are refused, so they are never kept.
    pub fn offer_pending(&mut self, bytes: &[u8], arrived: Timestamp, now: Timestamp) -> Result<PendingFlag, Refusal> {
        let (scope, until) = verify_stale(&self.fold, bytes, now)?;
        let d = SignedEntry::decode(bytes).map_err(Refusal::Malformed)?.digest();
        let f = self.pending.entry(d).or_insert(PendingFlag { scope, until, arrived });
        f.arrived = Timestamp(f.arrived.0.min(arrived.0));
        Ok(*f)
    }

    /// Drops pending flags no longer honored at `now`.
    pub fn prune(&mut self, now: Timestamp) {
        let max = self.stale_max;
        self.pending.retain(|_, f| now < f.honored_until(max));
    }

    /// Until when `chain` is in stale mode at `now`: the latest honored time of any flag that covers it, committed
    /// (until `until`) or pending (until `honored_until`). `None` when no flag covers it.
    pub fn stale_until(&self, chain: ChainId, owner: Option<AccountId>, now: Timestamp) -> Option<Timestamp> {
        let committed = self.fold.stale.iter().map(|(s, u)| (*s, *u));
        let pending = self.pending.values().map(|f| (f.scope, f.honored_until(self.stale_max)));
        committed.chain(pending).filter(|(s, u)| now < *u && covers(*s, chain, owner)).map(|(_, u)| u).max()
    }

    /// The edge's check of a submitted entry against its cached `Keys` before it commits (`design/DESIGN.md`, Shape,
    /// the release endpoint): bound to this install and group, and every signature verifies. A forward entry's
    /// signers are keys of the latest cached `Keys`, the new customer key of a `Keys`, or a registered lifeboat's key
    /// for its `LifeboatDecoder`; a pre-signed undo's key is judged by the group against the `Keys` of its parent's
    /// index, so only its signature is checked here. `decide` runs every other rule.
    pub fn precheck(&self, e: &SignedEntry) -> Result<(), Refusal> {
        let Some(g) = &self.fold.genesis else { return Ok(()) };
        if e.bound.group != ChainId::Release {
            return Err(Refusal::WrongGroup(e.bound.group));
        }
        if e.bound.install != g.install {
            return Err(Refusal::WrongInstall { want: g.install, got: e.bound.install });
        }
        let keys = self.fold.keys.as_ref().expect("an installed fold holds Keys");
        let known = |k: &KeyId| {
            [keys.customer, keys.release, keys.drain, keys.executor].contains(k)
                || keys.vendor == Some(*k)
                || keys.agents_executor == Some(*k)
        };
        for (k, sig) in &e.sigs {
            let extra = match &e.body {
                ReleaseEntry::Keys(new) => new.customer == *k,
                ReleaseEntry::LifeboatDecoder { host, .. } => self.fold.lifeboats.get(host) == Some(k),
                _ => false,
            };
            if !e.body.undo_only() && !known(k) && !extra {
                return Err(Refusal::UnknownKey(*k));
            }
            let msg = match &e.body {
                ReleaseEntry::FormatsEnable { format } if *k == keys.release => formats_enable_message(*format),
                _ => e.signed_bytes(),
            };
            crate::group::verify_raw(k, &msg, sig)?;
        }
        Ok(())
    }
}

/// An uncommitted `StaleMode`, checked against `fold`'s install and latest `Keys`: bound to the `Release` group of
/// this install and to no parent, signed by the customer key alone, and `until` still ahead of `now`. Returns the
/// flag's scope and `until`.
pub fn verify_stale(fold: &ReleaseFold, bytes: &[u8], now: Timestamp) -> Result<(Scope, Timestamp), Refusal> {
    let e = SignedEntry::decode(bytes).map_err(Refusal::Malformed)?;
    let ReleaseEntry::StaleMode { scope, until } = e.body else {
        return Err(Refusal::Rejected(format!("a {} is not a StaleMode", e.body.kind()).into()));
    };
    let (Some(g), Some(keys)) = (&fold.genesis, &fold.keys) else {
        return Err(Refusal::BadState("no Release view yet"));
    };
    if e.bound.group != ChainId::Release {
        return Err(Refusal::WrongGroup(e.bound.group));
    }
    if e.bound.install != g.install {
        return Err(Refusal::WrongInstall { want: g.install, got: e.bound.install });
    }
    if e.bound.parent.is_some() {
        return Err(Refusal::BadBound("a StaleMode binds no parent intent"));
    }
    let [(key, _)] = e.sigs.as_slice() else { return Err(Refusal::WrongSigner("StaleMode")) };
    if *key != keys.customer {
        return Err(Refusal::WrongSigner("StaleMode"));
    }
    e.verify(key)?;
    if until <= now {
        return Err(Refusal::Rejected("a StaleMode flag whose until has passed".into()));
    }
    Ok((scope, until))
}
