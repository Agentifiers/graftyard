//! The vault Worker (`design/DESIGN.md`, "Install operations", Vault; Shape, "Lifeboat feed"; `build/BUILD-PLAN.md`,
//! M5-WP5). It runs in the vault account, which none of the install's tokens reach, and pulls the install's export
//! endpoints with the install-scoped export token `graftyard vault init` gave it, into its own bucket `VAULT`.
//!
//! - **Layout.** Everything it keeps is under a locked prefix (`LOCKED`, 35 days by `graftyard vault init`'s bucket
//!   lock rules) and put create-only, so nothing it wrote is ever overwritten: each `ExportObject` as served under
//!   `feed/`, entries the feed never carried under `entries/`, snapshot objects under their install keys
//!   (`snapshot/`), and packs under theirs (`pack/`). Its own cursor, `VaultState`, is the one object it overwrites,
//!   under `state/`, which no rule locks; it is written compare-and-swap on its etag, so two passes that meet never
//!   move it back.
//! - **The feed.** A pass reads `export/batches` after its position, checks every object against its own cursors
//!   with the feed's rule (`export::check_object`: each chain contiguous, never a skipped index), stores it, and
//!   only then moves its cursors. A chain whose first carry starts past index 1 (entries archived before the feed
//!   existed) is filled from `chains/{chain}/entries` from 1, or, if those are past retention (410), from the newest
//!   snapshot below the carry. Every `ChainId` the feed carries is kept, `Release`, `Ids`, `Names`, `Tokens`,
//!   `Identity`, and user-owned repositories included.
//! - **Packs and snapshots.** Each repository entry is decoded (`absorb`): every stored pack it names is copied from
//!   `packs/{id}?network=` and checked against the entry's `PackRef.checksum` before the object exists
//!   (`PackCheck`); every snapshot an `Archived` names is copied from `snapshots?at=` and each object checked against
//!   its frame's SHA-256 before it exists (`SnapshotReader`), so a bad copy is never locked in.
//! - **GC.** The vault deletes a pack no sooner than `RESTORE_WINDOW` after the install's GC deletes it. The install
//!   deletes a stored pack only after a roll consumed it (`Compacted.consumed`, after `CONSUMED_KEEP`) or a purge of
//!   its deleted repository (after `retain_until`), so only those packs are probed, at most daily, and a pack is
//!   deleted only `RESTORE_WINDOW` after the install first answered 404 for it. A 410 (marked and in no bucket) is a
//!   loss at the install, never a deletion.

use std::collections::{BTreeMap, BTreeSet};

use graftyard_git::export::{self, ExportObject, FeedPosition, FeedState};
use graftyard_git::pack::{PackId, PackRef};
use graftyard_git::repo::{REPO_FORMAT_V1, RepoEvent, packs_of};
use graftyard_git::{CONSUMED_KEEP, R2Key};
use graftyard_raft::{EntryBody, Format, LogEntry, Term, codec};
use graftyard_types::{ChainId, Digest, Index, NetworkId, Timestamp};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

#[cfg(target_arch = "wasm32")]
mod worker;

/// Days each object stays locked (`design/DESIGN.md`, "Install operations", Vault: "bucket lock rules of 35 days").
pub const LOCK_DAYS: u64 = 35;
/// The prefixes `graftyard vault init` locks for `LOCK_DAYS`. `state/` is not among them.
pub const LOCKED: [&str; 4] = ["feed/", "entries/", "snapshot/", "pack/"];
pub const STATE_KEY: &str = "state/vault";
/// The vault's Cron Trigger: a pass each minute, which polls the feed for about `POLL_FOR_MS` [guess].
pub const PULL_CRON: &str = "* * * * *";
/// How long one invocation keeps polling, and how often: a feed round lands about every 15 s (`export-period`), so the
/// vault trails it by at most one poll [guess].
pub const POLL_FOR_MS: u64 = 45_000;
pub const POLL_EVERY_MS: u64 = 10_000;
/// Bounds on one pass, so one invocation stays inside its subrequest limit [guess]: batch pages, entry pages filled
/// before the feed, pack and snapshot copies, and GC probes.
pub const PAGES_PER_PASS: usize = 20;
pub const BACKFILL_PER_PASS: usize = 20;
pub const COPIES_PER_PASS: usize = 40;
pub const PROBES_PER_PASS: usize = 50;
/// A GC candidate is probed at most this often.
pub const PROBE_EVERY_MS: u64 = 86_400_000;
/// A pass that finds the feed this far behind alarms [guess].
pub const LAG_ALARM_MS: u64 = 5 * 60_000;
/// Multipart part size for copies: R2 wants equal parts of at least 5 MiB but the last.
pub const PART_BYTES: usize = 8 << 20;

/// The repository family of `LogEntry.format`: only its entries name packs and snapshots.
const REPO_FAMILY: u16 = REPO_FORMAT_V1.family;

fn restore_window_ms() -> u64 {
    graftyard_fold::RESTORE_WINDOW.as_millis() as u64
}

pub fn feed_key(p: FeedPosition) -> String {
    format!("feed/{:012}/{:010}", p.minute, p.seq)
}

pub fn entries_key(chain: ChainId, from: Index, to: Index) -> String {
    format!("entries/{chain}/{:020}-{:020}", from.0, to.0)
}

pub fn pack_key(network: NetworkId, id: PackId) -> String {
    R2Key::Pack { network, id }.to_string()
}

/// The network whose prefix holds a repository chain's packs: the repository's own id, as the edge places them
/// (`NetworkId(repo.id.0)` on every push path) [inference: forks into a shared network are not built].
fn network_of(chain: ChainId) -> Option<NetworkId> {
    match chain {
        ChainId::Repo(r) | ChainId::RepoMeta(r) => Some(NetworkId(r.0)),
        _ => None,
    }
}

/// A copy the vault owes: a pack an entry named, or the snapshot an `Archived` named.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Copy {
    Pack { network: NetworkId, id: PackId, checksum: Digest, bytes: u64 },
    Snapshot { chain: ChainId, at: Index },
}

/// A chain whose first carry in the feed started past index 1: entries `next..until` are still to fill.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backfill {
    pub next: Index,
    pub until: Index,
}

/// A pack the install's GC may delete: probed from `after`, and deletable `RESTORE_WINDOW` after `gone`, the first
/// probe that found it gone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gc {
    pub after: Timestamp,
    pub probed: Option<Timestamp>,
    pub gone: Option<Timestamp>,
}

/// `state/vault`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultState {
    /// The newest feed object stored, and each chain's highest index stored from the feed.
    pub feed: FeedState,
    pub backfill: BTreeMap<ChainId, Backfill>,
    pub copies: BTreeSet<Copy>,
    /// GC candidates by pack key (`pack_key`).
    pub gc: BTreeMap<String, Gc>,
    /// Deleted repositories and their `retain_until`, until their packs join `gc`.
    pub deleted: BTreeMap<ChainId, Timestamp>,
    /// Copies the install could no longer serve (404 or 410 before the vault reached them), and repository rows that
    /// did not decode, so no pack or snapshot they name is known. Both are alarmed.
    pub missed: u64,
    pub undecoded: u64,
}

impl VaultState {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("json")
    }
    pub fn decode(b: &[u8]) -> Result<VaultState, String> {
        serde_json::from_slice(b).map_err(|e| format!("state/vault: {e}"))
    }
}

/// An export read: the body and whether the endpoint's `Link` names a next page, or a non-2xx status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Got {
    Ok { body: Vec<u8>, more: bool },
    Status(u16),
}

/// A copy's outcome: stored now, already in the vault, bytes that failed their check (never stored), or the
/// endpoint's non-2xx status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Copied {
    Stored,
    Exists,
    Corrupt(String),
    Status(u16),
}

/// What a pass needs from its Worker: the install's export endpoints under the token, and the `VAULT` bucket.
#[allow(async_fn_in_trait)]
pub trait Host {
    fn now(&self) -> Timestamp;
    /// `GET /api/graftyard/v1/export/<path>`, `path` with its query.
    async fn get(&self, path: &str) -> Result<Got, String>;
    /// A pack from `packs/{id}?network=` into `pack_key`, stored only if its bytes hash to `checksum` (`PackCheck`).
    async fn copy_pack(&self, network: NetworkId, id: PackId, checksum: Digest, bytes: u64) -> Result<Copied, String>;
    /// The snapshot `snapshots?at=` answers into each object's own key, each stored only if it matches its frame's
    /// SHA-256 and is a snapshot of `chain` (`SnapshotReader`).
    async fn copy_snapshot(&self, chain: ChainId, at: Index) -> Result<Copied, String>;
    /// Whether the install still serves a pack: the status of `packs/{id}?network=`, its body unread.
    async fn pack_status(&self, network: NetworkId, id: PackId) -> Result<u16, String>;
    /// A create-only put (`If-None-Match: *`) carrying `sha256` as the object's `sha256` metadata: true when it
    /// created the object, false when the precondition refused it. R2 checks a bucket lock before the precondition, so
    /// a create-only put to a key a lock holds fails with `ObjectLockedByBucketPolicy` (409), not 412, and comes back
    /// as `Err`; `put_once` asks `head` either way.
    async fn put_create(&self, key: &str, body: &[u8], sha256: &Digest) -> Result<bool, String>;
    /// The object at `key`: its size and its `sha256` metadata, if any.
    async fn head(&self, key: &str) -> Result<Option<Held>, String>;
    /// `state/vault` and its etag.
    async fn state(&self) -> Result<Option<(Vec<u8>, String)>, String>;
    /// Writes `state/vault` only if its etag is still `etag` (`None`: only if absent). The new etag, or `None` when
    /// another pass wrote it first.
    async fn put_state(&self, body: &[u8], etag: Option<&str>) -> Result<Option<String>, String>;
    async fn list(&self, prefix: &str) -> Result<Vec<String>, String>;
    /// False when a lock rule refused it.
    async fn delete(&self, key: &str) -> Result<bool, String>;
}

/// What `head` sees at a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub size: u64,
    pub sha256: Option<String>,
}

/// The object at a key against the one the vault means to write there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Taken {
    Absent,
    /// The same object: its size, and its `sha256` metadata where the vault knows the digest before writing.
    Same,
    /// Another object holds the key; the lock keeps it, so the vault must never call it stored.
    Different(String),
}

/// Judges `held` against an object of `len` bytes with SHA-256 `sha256`. A pack's, feed object's, or entries page's
/// digest is known before the write and stored as metadata, so it must match. A snapshot object's is known only after
/// its bytes have streamed, and its key names the snapshot's range and digest, so its size stands in (`None`).
pub fn taken(held: Option<Held>, len: u64, sha256: Option<&Digest>) -> Taken {
    let Some(h) = held else { return Taken::Absent };
    if h.size != len {
        return Taken::Different(format!("{} bytes where {len} were meant", h.size));
    }
    match (sha256, h.sha256.as_deref()) {
        (None, _) => Taken::Same,
        (Some(want), Some(got)) if got == want.hex() => Taken::Same,
        (Some(want), got) => {
            Taken::Different(format!("sha256 {} where {} was meant", got.unwrap_or("unknown"), want.hex()))
        }
    }
}

/// Writes `body` at `key` once: true when this call stored it, false when the same object was already there. A
/// refused put, by its precondition or by a lock rule, is judged by `head`, so a pass that repeats an earlier one's
/// writes moves on, and another object at the key, or a refusal with nothing there, fails the pass.
pub async fn put_once<H: Host>(h: &H, key: &str, body: &[u8]) -> Result<bool, String> {
    let sha = Digest(Sha256::digest(body).into());
    let refused = match h.put_create(key, body, &sha).await {
        Ok(true) => return Ok(true),
        Ok(false) => None,
        Err(e) => Some(e),
    };
    match taken(h.head(key).await?, body.len() as u64, Some(&sha)) {
        Taken::Same => Ok(false),
        Taken::Different(why) => Err(format!("{key} holds another object: {why}")),
        Taken::Absent => Err(format!("{key}: {}", refused.unwrap_or_else(|| "refused, and nothing is there".into()))),
    }
}

/// One pass's counts, logged as one line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub objects: usize,
    pub backfilled: usize,
    pub packs: usize,
    pub snapshots: usize,
    pub existed: usize,
    pub missed: usize,
    pub probes: usize,
    pub deleted: usize,
    /// How far the newest stored object trails `now`, by its feed minute.
    pub lag_ms: Option<u64>,
    /// A bound stopped the pass with work left.
    pub more: bool,
    /// Another pass moved the state first; this one stopped.
    pub lost: bool,
}

impl Report {
    pub fn line(&self, st: &VaultState) -> String {
        format!(
            "vault pass objects={} backfilled={} packs={} snapshots={} existed={} missed={} probes={} deleted={} \
             at={} lag_ms={} chains={} owed={} gc={} missed_total={} undecoded={}{}{}",
            self.objects,
            self.backfilled,
            self.packs,
            self.snapshots,
            self.existed,
            self.missed,
            self.probes,
            self.deleted,
            st.feed.last.map_or("-".into(), |p| p.text()),
            self.lag_ms.map_or("-".into(), |l| l.to_string()),
            st.feed.cursors.0.len(),
            st.copies.len() + st.backfill.len(),
            st.gc.len(),
            st.missed,
            st.undecoded,
            if self.more { " more" } else { "" },
            if self.lost { " lost" } else { "" },
        )
    }
}

/// The header of a stored `LogEntry` row, decoded without its body: `codec`'s version byte, then the fields in
/// order, so a row of another family is never decoded as a repository entry.
fn row_format(row: &[u8]) -> Option<Format> {
    let (&v, rest) = row.split_first()?;
    (v == codec::CODEC_VERSION).then_some(())?;
    postcard::take_from_bytes::<(Term, Index, Digest, Format)>(rest).ok().map(|((_, _, _, f), _)| f)
}

/// Every stored pack a repository entry names: pushes, merges, test merges, and merge-queue groups (`packs_of`), and a
/// promotion's or a roll's own pack.
fn stored_packs(events: &[RepoEvent]) -> Vec<(PackId, Digest, u64)> {
    let mut all = packs_of(events);
    for ev in events {
        match ev {
            RepoEvent::Promoted(p) => all.push(p.pack.clone()),
            RepoEvent::Compacted(c) => all.push(c.produced.clone()),
            _ => {}
        }
    }
    all.into_iter()
        .filter_map(|p| match p {
            PackRef::Stored { id, checksum, bytes, .. } => Some((id, checksum, bytes)),
            PackRef::Inline(_) => None,
        })
        .collect()
}

/// Folds one chain's stored rows into what the vault owes: packs and snapshots to copy, packs the install's GC may
/// delete, and deleted repositories. Only repository chains' rows of the repository family are decoded; one that does
/// not decode is counted, since a pack it names would be unknown.
pub fn absorb(st: &mut VaultState, chain: ChainId, rows: &[Vec<u8>]) {
    let Some(network) = network_of(chain) else { return };
    for row in rows {
        if row_format(row).is_none_or(|f| f.family != REPO_FAMILY) {
            continue;
        }
        let Ok(e) = codec::from_bytes::<LogEntry<RepoEvent>>(row) else {
            st.undecoded += 1;
            continue;
        };
        let EntryBody::Events(events) = &e.body else { continue };
        for (id, checksum, bytes) in stored_packs(events) {
            st.copies.insert(Copy::Pack { network, id, checksum, bytes });
        }
        for ev in events {
            match ev {
                RepoEvent::Archived(a) => {
                    if let R2Key::Snapshot { to, .. } = a.key {
                        st.copies.insert(Copy::Snapshot { chain, at: to });
                    }
                }
                RepoEvent::Compacted(c) => {
                    for id in &c.consumed {
                        st.gc.entry(pack_key(network, *id)).or_insert(Gc {
                            after: Timestamp(e.at.0 + CONSUMED_KEEP),
                            probed: None,
                            gone: None,
                        });
                    }
                }
                RepoEvent::Deleted { retain_until, .. } => {
                    st.deleted.insert(chain, *retain_until);
                }
                _ => {}
            }
        }
    }
}

/// Takes one checked feed object into the state: a chain first carried past index 1 owes its earlier entries, then
/// its rows are absorbed and the cursors move.
fn take_object(st: &mut VaultState, o: &ExportObject) {
    for (chain, first, rows) in &o.entries {
        if st.feed.cursors.get(*chain) == Index(0) && first.0 > 1 && !st.backfill.contains_key(chain) {
            st.backfill.insert(*chain, Backfill { next: Index(1), until: *first });
        }
        absorb(st, *chain, rows);
    }
    st.feed.advance(&o.batch);
}

/// A GC candidate's next step at `now`: probe it, delete it, or nothing yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GcStep {
    Wait,
    Probe,
    Delete,
}

pub fn gc_step(g: &Gc, now: Timestamp) -> GcStep {
    match g.gone {
        Some(gone) if now.0 >= gone.0.saturating_add(restore_window_ms()) => GcStep::Delete,
        Some(_) => GcStep::Wait,
        None if now < g.after => GcStep::Wait,
        None if g.probed.is_some_and(|p| now.0.saturating_sub(p.0) < PROBE_EVERY_MS) => GcStep::Wait,
        None => GcStep::Probe,
    }
}

/// Checks a pack's bytes as they stream: the vault puts the object (or completes its upload) only if `done` holds.
pub struct PackCheck {
    want: Digest,
    bytes: u64,
    seen: u64,
    sha: Sha256,
}

impl PackCheck {
    pub fn new(want: Digest, bytes: u64) -> PackCheck {
        PackCheck { want, bytes, seen: 0, sha: Sha256::new() }
    }
    /// False once more bytes came than the entry names, so the copy can stop early.
    pub fn push(&mut self, chunk: &[u8]) -> bool {
        self.seen += chunk.len() as u64;
        self.sha.update(chunk);
        self.seen <= self.bytes
    }
    pub fn done(self) -> Result<(), String> {
        let got = Digest(self.sha.finalize().into());
        if self.seen != self.bytes {
            return Err(format!("{} bytes where the entry names {}", self.seen, self.bytes));
        }
        if got != self.want {
            return Err(format!("SHA-256 {} where the entry names {}", got.hex(), self.want.hex()));
        }
        Ok(())
    }
}

/// What a snapshot body yields as it streams (`export` frames, per object: its key as a frame, then a 4-byte length,
/// the bytes, and their SHA-256).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapPart {
    Start {
        key: String,
        len: u64,
    },
    Bytes(Vec<u8>),
    /// The object's bytes matched its SHA-256.
    End {
        ok: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SnapAt {
    KeyLen,
    Key(usize),
    KeyDigest(usize),
    Len,
    Body(u64),
    Digest,
}

/// Splits a snapshot body into objects as it arrives, in chunks of any size, never holding an object whole.
pub struct SnapshotReader {
    at: SnapAt,
    buf: Vec<u8>,
    key: Vec<u8>,
    sha: Sha256,
}

impl Default for SnapshotReader {
    fn default() -> Self {
        SnapshotReader { at: SnapAt::KeyLen, buf: Vec::new(), key: Vec::new(), sha: Sha256::new() }
    }
}

impl SnapshotReader {
    pub fn push(&mut self, mut chunk: &[u8]) -> Result<Vec<SnapPart>, String> {
        let mut out = Vec::new();
        while !chunk.is_empty() {
            match self.at {
                SnapAt::Body(left) => {
                    let n = (left as usize).min(chunk.len());
                    self.sha.update(&chunk[..n]);
                    out.push(SnapPart::Bytes(chunk[..n].to_vec()));
                    chunk = &chunk[n..];
                    self.at = if left == n as u64 { SnapAt::Digest } else { SnapAt::Body(left - n as u64) };
                }
                _ => {
                    let want = match self.at {
                        SnapAt::KeyLen | SnapAt::Len => 4,
                        SnapAt::Key(n) => n,
                        SnapAt::KeyDigest(_) | SnapAt::Digest => 32,
                        SnapAt::Body(_) => unreachable!(),
                    };
                    let n = (want - self.buf.len()).min(chunk.len());
                    self.buf.extend_from_slice(&chunk[..n]);
                    chunk = &chunk[n..];
                    if self.buf.len() == want {
                        let b = std::mem::take(&mut self.buf);
                        self.step(&b, &mut out)?;
                    }
                }
            }
        }
        Ok(out)
    }

    fn step(&mut self, b: &[u8], out: &mut Vec<SnapPart>) -> Result<(), String> {
        let be = |b: &[u8]| u32::from_be_bytes(b.try_into().expect("4 bytes"));
        self.at = match self.at {
            SnapAt::KeyLen => match be(b) as usize {
                0 => return Err("an empty snapshot key".into()),
                n if n > 512 => return Err(format!("a snapshot key of {n} bytes")),
                n => SnapAt::Key(n),
            },
            SnapAt::Key(n) => {
                self.key = b.to_vec();
                SnapAt::KeyDigest(n)
            }
            SnapAt::KeyDigest(_) => {
                if Sha256::digest(&self.key).as_slice() != b {
                    return Err("a snapshot key fails its digest".into());
                }
                SnapAt::Len
            }
            SnapAt::Len => {
                let key =
                    String::from_utf8(std::mem::take(&mut self.key)).map_err(|_| "a snapshot key is not UTF-8")?;
                let len = u64::from(be(b));
                out.push(SnapPart::Start { key, len });
                self.sha = Sha256::new();
                if len == 0 { SnapAt::Digest } else { SnapAt::Body(len) }
            }
            SnapAt::Digest => {
                let sha = std::mem::replace(&mut self.sha, Sha256::new());
                out.push(SnapPart::End { ok: sha.finalize().as_slice() == b });
                SnapAt::KeyLen
            }
            SnapAt::Body(_) => unreachable!(),
        };
        Ok(())
    }

    /// Whether the body ended between objects.
    pub fn finished(&self) -> bool {
        self.at == SnapAt::KeyLen && self.buf.is_empty()
    }
}

/// Whether `key` is a snapshot of `chain`, as the vault stores only those from a snapshot copy.
pub fn snapshot_of(key: &str, chain: ChainId) -> bool {
    matches!(key.parse::<R2Key>(), Ok(R2Key::Snapshot { chain: c, .. }) if c == chain)
}

fn refused(what: &str, status: u16) -> String {
    match status {
        401 => format!("{what}: 401, the export token is refused (revoked, expired, or not this install's)"),
        s => format!("{what}: {s}"),
    }
}

/// One pass: the feed, then entries the feed never carried, then owed copies, then GC. The state is written after
/// each feed page and at the end, compare-and-swap; a pass that loses the swap stops.
pub async fn pass<H: Host>(h: &H) -> Result<(Report, VaultState), String> {
    let now = h.now();
    let (mut st, mut etag) = match h.state().await? {
        Some((b, e)) => (VaultState::decode(&b)?, Some(e)),
        None => (VaultState::default(), None),
    };
    let mut r = Report::default();

    // The feed, page by page from the stored position. A 410 means the position is past the feed's retention (the
    // export bucket's lifecycle), which an idle install reaches with nothing lost: the pass lists the feed again from
    // its oldest object, skips what it holds, and fills a chain whose next object skips past its cursor from the
    // entries endpoint.
    let (mut after, mut resync) = (st.feed.last, false);
    for page in 0.. {
        if page == PAGES_PER_PASS {
            r.more = true;
            break;
        }
        let path = match after {
            Some(p) => format!("batches?after={}", p.text()),
            None => "batches".into(),
        };
        let (body, more) = match h.get(&path).await? {
            Got::Ok { body, more } => (body, more),
            Got::Status(410) if after.is_some() && !resync => {
                (after, resync) = (None, true);
                continue;
            }
            Got::Status(s) => return Err(refused(&path, s)),
        };
        let frames = export::unframe(&body)?;
        if frames.is_empty() {
            break;
        }
        let mut took = 0;
        for raw in &frames {
            let o = export::decode_object(raw)?;
            let pos = o.batch.position();
            after = Some(pos);
            if st.feed.last.is_some_and(|l| pos <= l) {
                if resync {
                    continue;
                }
                return Err(format!("batch {} is not after {}", pos.text(), path));
            }
            let mut cursors = st.feed.cursors.clone();
            if resync {
                for (c, first, _) in &o.entries {
                    let at = cursors.get(*c);
                    if at.0 > 0 && first.0 > at.0 + 1 {
                        st.backfill.insert(*c, Backfill { next: Index(at.0 + 1), until: *first });
                        cursors.0.insert(*c, Index(first.0 - 1));
                    }
                }
            }
            export::check_object(&o, &cursors).map_err(|e| format!("feed {}: {e}", pos.text()))?;
            put_once(h, &feed_key(pos), raw).await?;
            take_object(&mut st, &o);
            r.objects += 1;
            took += 1;
        }
        if took > 0 {
            match h.put_state(&st.encode(), etag.as_deref()).await? {
                Some(e) => etag = Some(e),
                None => {
                    r.lost = true;
                    return Ok((r, st));
                }
            }
        }
        if !more {
            break;
        }
    }
    r.lag_ms = st.feed.last.map(|p| now.0.saturating_sub(p.minute * 60_000));

    // Entries archived before the feed carried their chain.
    let fills: Vec<(ChainId, Backfill)> = st.backfill.iter().map(|(c, b)| (*c, b.clone())).collect();
    for (n, (chain, b)) in fills.into_iter().enumerate() {
        if n == BACKFILL_PER_PASS {
            r.more = true;
            break;
        }
        let limit = (b.until.0 - b.next.0).min(export::ENTRIES_PAGE);
        let path = format!("chains/{chain}/entries?from={}&limit={limit}", b.next.0);
        match h.get(&path).await? {
            Got::Ok { body, .. } => {
                let rows = export::unframe(&body)?;
                let rows = &rows[..rows.len().min(limit as usize)];
                if rows.is_empty() {
                    continue;
                }
                let to = Index(b.next.0 + rows.len() as u64 - 1);
                let mut page = Vec::new();
                for row in rows {
                    export::frame(&mut page, row);
                }
                put_once(h, &entries_key(chain, b.next, to), &page).await?;
                absorb(&mut st, chain, rows);
                r.backfilled += rows.len();
                if to.0 + 1 >= b.until.0 {
                    st.backfill.remove(&chain);
                } else {
                    st.backfill.insert(chain, Backfill { next: Index(to.0 + 1), until: b.until });
                }
            }
            // Archived and dropped before the vault reached them: the newest snapshot below the carry stands in.
            Got::Status(410) => {
                st.backfill.remove(&chain);
                st.copies.insert(Copy::Snapshot { chain, at: Index(b.until.0 - 1) });
            }
            Got::Status(401) => return Err(refused(&path, 401)),
            // Retried on the next pass.
            Got::Status(_) => r.more = true,
        }
    }

    // Owed copies.
    let owed: Vec<Copy> = st.copies.iter().cloned().collect();
    for (n, c) in owed.into_iter().enumerate() {
        if n == COPIES_PER_PASS {
            r.more = true;
            break;
        }
        let got = match &c {
            Copy::Pack { network, id, checksum, bytes } => h.copy_pack(*network, *id, *checksum, *bytes).await,
            Copy::Snapshot { chain, at } => h.copy_snapshot(*chain, *at).await,
        };
        match got? {
            Copied::Stored if matches!(c, Copy::Pack { .. }) => r.packs += 1,
            Copied::Stored => r.snapshots += 1,
            Copied::Exists => r.existed += 1,
            // The install no longer serves it, or served bytes that fail the entry's checksum: alarmed, not retried.
            Copied::Status(404 | 410) | Copied::Corrupt(_) => {
                r.missed += 1;
                st.missed += 1;
            }
            Copied::Status(401) => return Err(refused(&format!("{c:?}"), 401)),
            // A 5xx: kept owed for the next pass.
            Copied::Status(_) => {
                r.more = true;
                continue;
            }
        }
        st.copies.remove(&c);
    }

    // GC: deleted repositories past retention join the candidates; each is probed, then deleted RESTORE_WINDOW after
    // the install first answered 404.
    let due: Vec<(ChainId, Timestamp)> = st.deleted.iter().filter(|(_, t)| now >= **t).map(|(c, t)| (*c, *t)).collect();
    for (chain, until) in due {
        let Some(network) = network_of(chain) else { continue };
        for key in h.list(&R2Key::pack_prefix(network)).await? {
            if key.ends_with(".pack") {
                st.gc.entry(key).or_insert(Gc { after: until, probed: None, gone: None });
            }
        }
        st.deleted.remove(&chain);
    }
    let candidates: Vec<(String, Gc)> = st.gc.iter().map(|(k, g)| (k.clone(), g.clone())).collect();
    for (key, g) in candidates {
        let step = gc_step(&g, now);
        if step == GcStep::Wait {
            continue;
        }
        if r.probes == PROBES_PER_PASS {
            r.more = true;
            continue;
        }
        let Ok(R2Key::Pack { network, id }) = key.parse::<R2Key>() else {
            st.gc.remove(&key);
            continue;
        };
        // A delete is probed again first: it cannot be undone, so the install must still answer 404 for the pack.
        r.probes += 1;
        match (h.pack_status(network, id).await?, step) {
            (401, _) => return Err(refused(&key, 401)),
            (404, GcStep::Delete) => {
                if h.delete(&key).await? {
                    st.gc.remove(&key);
                    r.deleted += 1;
                }
            }
            (404, _) => {
                st.gc.insert(key, Gc { probed: Some(now), gone: Some(now), ..g });
            }
            // Served again, or a 410 (marked and in no bucket: a loss at the install, not a GC) or a 5xx: the clock
            // starts over at the next 404.
            _ => {
                st.gc.insert(key, Gc { probed: Some(now), gone: None, ..g });
            }
        }
    }

    if r.backfilled + r.packs + r.snapshots + r.existed + r.missed + r.probes + r.deleted > 0
        && h.put_state(&st.encode(), etag.as_deref()).await?.is_none()
    {
        r.lost = true;
    }
    Ok((r, st))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap_body(objects: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = Vec::new();
        for (k, bytes) in objects {
            export::frame(&mut b, k.as_bytes());
            export::frame(&mut b, bytes);
        }
        b
    }

    /// Objects come back whole and checked whatever the chunking, across key, length, body, and digest boundaries,
    /// and a changed byte fails only its own object.
    #[test]
    fn snapshot_reader_splits_objects_across_any_chunking() {
        let a = vec![7u8; 1000];
        let body = snap_body(&[("snapshot/repo:1/core/x", &a), ("snapshot/repo:1/metadata/y", b"")]);
        for step in [1, 3, 31, 32, 37, 4096] {
            let mut rd = SnapshotReader::default();
            let mut parts = Vec::new();
            for c in body.chunks(step) {
                parts.extend(rd.push(c).unwrap());
            }
            assert!(rd.finished());
            let bytes: Vec<u8> = parts
                .iter()
                .filter_map(|p| if let SnapPart::Bytes(b) = p { Some(b.clone()) } else { None })
                .flatten()
                .collect();
            assert_eq!(bytes, a);
            let marks: Vec<&SnapPart> = parts.iter().filter(|p| !matches!(p, SnapPart::Bytes(_))).collect();
            assert_eq!(
                marks,
                vec![
                    &SnapPart::Start { key: "snapshot/repo:1/core/x".into(), len: 1000 },
                    &SnapPart::End { ok: true },
                    &SnapPart::Start { key: "snapshot/repo:1/metadata/y".into(), len: 0 },
                    &SnapPart::End { ok: true },
                ]
            );
        }
        let mut bad = body.clone();
        let at = 4 + 22 + 32 + 4 + 500;
        bad[at] ^= 1;
        let parts = SnapshotReader::default().push(&bad).unwrap();
        let ends: Vec<bool> =
            parts.iter().filter_map(|p| if let SnapPart::End { ok } = p { Some(*ok) } else { None }).collect();
        assert_eq!(ends, vec![false, true]);
        let mut cut = SnapshotReader::default();
        cut.push(&body[..body.len() - 5]).unwrap();
        assert!(!cut.finished());
    }

    #[test]
    fn pack_check_needs_the_entry_s_length_and_digest() {
        let body = b"PACK some bytes".to_vec();
        let sum = Digest(Sha256::digest(&body).into());
        let mut ok = PackCheck::new(sum, body.len() as u64);
        for c in body.chunks(4) {
            assert!(ok.push(c));
        }
        ok.done().unwrap();
        let mut short = PackCheck::new(sum, body.len() as u64 + 1);
        short.push(&body);
        assert!(short.done().unwrap_err().contains("bytes"));
        let mut long = PackCheck::new(sum, body.len() as u64 - 1);
        assert!(!long.push(&body));
        let mut other = PackCheck::new(Digest([1; 32]), body.len() as u64);
        other.push(&body);
        assert!(other.done().unwrap_err().contains("SHA-256"));
    }

    /// Probed from `after`, at most daily; deleted only `RESTORE_WINDOW` after the first 404, never before.
    #[test]
    fn gc_deletes_only_a_restore_window_after_the_install_did() {
        let day = 86_400_000;
        let g = Gc { after: Timestamp(10 * day), probed: None, gone: None };
        assert_eq!(gc_step(&g, Timestamp(10 * day - 1)), GcStep::Wait);
        assert_eq!(gc_step(&g, Timestamp(10 * day)), GcStep::Probe);
        let probed = Gc { probed: Some(Timestamp(10 * day)), ..g.clone() };
        assert_eq!(gc_step(&probed, Timestamp(11 * day - 1)), GcStep::Wait);
        assert_eq!(gc_step(&probed, Timestamp(11 * day)), GcStep::Probe);
        let gone = Gc { gone: Some(Timestamp(12 * day)), ..probed };
        assert_eq!(gc_step(&gone, Timestamp(12 * day + restore_window_ms() - 1)), GcStep::Wait);
        assert_eq!(gc_step(&gone, Timestamp(12 * day + restore_window_ms())), GcStep::Delete);
    }

    #[test]
    fn every_kept_key_is_under_a_locked_prefix_and_the_state_is_not() {
        let p = FeedPosition { minute: 29_891_520, seq: 3 };
        let keys = [
            feed_key(p),
            entries_key(ChainId::Release, Index(1), Index(9)),
            pack_key(NetworkId(graftyard_types::DbId(7)), PackId(graftyard_types::Ulid(5))),
            R2Key::Snapshot {
                chain: ChainId::Ids,
                tier: graftyard_types::Tier::Core,
                from: Index(0),
                to: Index(4),
                digest: Digest([2; 32]),
            }
            .to_string(),
        ];
        for k in &keys {
            assert_eq!(LOCKED.iter().filter(|l| k.starts_with(*l)).count(), 1, "{k}");
        }
        assert!(!LOCKED.iter().any(|l| STATE_KEY.starts_with(l)));
        assert_eq!(LOCK_DAYS * 86_400_000, restore_window_ms(), "the lock is the restore window");
    }
}
