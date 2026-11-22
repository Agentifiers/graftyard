//! Composed clone-cache misses: the chain's R2 layout and the rules the shard object applies before it forwards an
//! upload-pack (`design/DESIGN.md`, Shape, "Git cache shards (derived)"; `decisions/DECISIONS.md`, "Clone-cache misses
//! are composed from the cached tip"). The container builds and serves the chain (`containers/git/server/compose.go`);
//! this module decides which requests wait for it and which chain files a shard still has to fetch.
//!
//! In each warm bucket a network's chain is `compose/<net>/head`, a small JSON object naming the base's tip, each
//! part's tip in order, and the last tip's tree, and beside it `base-<tip>` and `part-<from>-<to>`, each a `.pack`
//! and its `.idx`. The home site's first shard writes the files first and the head last, so a head never names a file
//! that is not there yet; a shard that finds one missing anyway serves stock.

use serde::{Deserialize, Serialize};

use crate::refs::ObjectId;
use crate::smart::V2Request;

/// The chain as the container's `/compose-head` and R2's `compose/<net>/head` hold it. Hex strings, as the container
/// writes them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainHead {
    pub base: String,
    #[serde(default)]
    pub parts: Vec<String>,
    pub tree: String,
}

impl ChainHead {
    /// `None` unless every id is a full SHA-1 in lowercase hex.
    pub fn checked(self) -> Option<ChainHead> {
        let ok = |s: &str| s.len() == 40 && ObjectId::parse_hex(s.as_bytes()).is_some_and(|o| o.hex() == s);
        (ok(&self.base) && ok(&self.tree) && self.parts.iter().all(|p| ok(p))).then_some(self)
    }

    /// The chain's tip: its last part's, else its base's.
    pub fn tip(&self) -> &str {
        self.parts.last().unwrap_or(&self.base)
    }

    /// The chain's packs in pack order, each by its name without the extension: `base-<tip>`, then
    /// `part-<from>-<to>` per move.
    pub fn packs(&self) -> Vec<String> {
        let mut out = vec![format!("base-{}", self.base)];
        let mut from = &self.base;
        for p in &self.parts {
            out.push(format!("part-{from}-{p}"));
            from = p;
        }
        out
    }

    /// Every file the chain needs, `.pack` and `.idx` of each pack.
    pub fn files(&self) -> Vec<String> {
        self.packs().into_iter().flat_map(|p| [format!("{p}.pack"), format!("{p}.idx")]).collect()
    }

    /// The files `self` names that `had` does not: what a shard holding `had` fetches to install `self`. All of them
    /// when it holds no chain.
    pub fn to_fetch(&self, had: Option<&ChainHead>) -> Vec<String> {
        let have = had.map(ChainHead::files).unwrap_or_default();
        self.files().into_iter().filter(|f| !have.contains(f)).collect()
    }
}

/// R2's `compose/<net>/head` as the builder puts it: the container's chain, and beside it each chain pack's object as
/// the builder found it in that bucket after putting it, in `ChainHead::packs` order (`head-etags`). The container
/// never sees `packs`: `ready_chain` compares and installs `chain` alone, so two heads whose chains match are the same
/// step whatever their packs say, and a head without packs (an older builder, or one past `HEAD_MAX`) reads as one
/// with none, whose every pack the edge checks as before.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadObject {
    #[serde(flatten)]
    pub chain: ChainHead,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packs: Vec<HeadPack>,
}

/// One chain pack's object as a head names it: its R2 etag, its size, and its pack header's object count. A tuple on
/// the wire, so a head of `foldEvery` packs stays under `HEAD_MAX`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadPack(pub String, pub u64, pub u32);

/// The most bytes a head object may take with its packs; past it the builder puts the chain alone.
pub const HEAD_MAX: usize = 2_048;

impl HeadObject {
    /// `chain`'s head with `packs`, or with none when they do not name every pack of the chain or would take the head
    /// past `HEAD_MAX`.
    pub fn new(chain: ChainHead, packs: Vec<HeadPack>) -> HeadObject {
        let full = HeadObject { packs, chain };
        if full.packs.len() == full.chain.packs().len()
            && serde_json::to_vec(&full).map_or(usize::MAX, |b| b.len()) <= HEAD_MAX
        {
            return full;
        }
        HeadObject { chain: full.chain, packs: Vec::new() }
    }

    /// What the builder does to a bucket whose head is `old` to bring it to `chain`: `None` when `old` is already that
    /// step, else the files to put before the head. The chains alone are compared, never the packs, so a head the
    /// builder put with its packs is the same step as the container's chain without them, and a builder never puts a
    /// head again only because the packs it would record differ.
    pub fn plan(old: Option<&HeadObject>, chain: &ChainHead) -> Option<Vec<String>> {
        if old.is_some_and(|o| &o.chain == chain) {
            return None;
        }
        Some(chain.to_fetch(old.map(|o| &o.chain)))
    }

    /// The packs record for `chain` once `put` were put: each pack of `chain`, with the old head's record of it when
    /// this step did not put it (the same object the old head named), else `None`, for the builder to read back.
    pub fn carried(old: Option<&HeadObject>, chain: &ChainHead, put: &[String]) -> Vec<(String, Option<HeadPack>)> {
        let kept = |name: &str| {
            let o = old?;
            let names = o.chain.packs();
            if names.len() != o.packs.len() || put.iter().any(|f| f == &format!("{name}.pack")) {
                return None;
            }
            names.iter().position(|n| n == name).map(|i| o.packs[i].clone())
        };
        chain.packs().into_iter().map(|p| (p.clone(), kept(&p))).collect()
    }

    /// Each named pack's header as a header set would hold it, by pack name (`ChainHead::packs`); none when the head
    /// names no packs or not one for each of the chain's.
    pub fn headers(&self) -> std::collections::BTreeMap<String, Header> {
        let names = self.chain.packs();
        if names.len() != self.packs.len() {
            return Default::default();
        }
        names
            .into_iter()
            .zip(&self.packs)
            .filter(|(_, p)| p.1 >= 32)
            .map(|(n, p)| (n, Header { n: p.2, etag: p.0.clone(), body: p.1 - 32 }))
            .collect()
    }
}

/// Whether `name` is a chain file name: `base-<tip>` or `part-<from>-<to>`, then `.pack` or `.idx`.
pub fn file_name_ok(name: &str) -> bool {
    let hex = |s: &str| s.len() == 40 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let Some((stem, ext)) = name.rsplit_once('.') else { return false };
    if ext != "pack" && ext != "idx" {
        return false;
    }
    match stem.split('-').collect::<Vec<_>>().as_slice() {
        ["base", t] => hex(t),
        ["part", f, t] => hex(f) && hex(t),
        _ => false,
    }
}

/// The arguments a composable fetch may carry besides one `want`, `deepen 1`, `ofs-delta`, and `done`.
const OTHER_ARGS: [&str; 3] = ["thin-pack", "no-progress", "include-tag"];

/// Whether a protocol v2 request is a depth-1 fetch the chain may answer: `fetch`, SHA-1, one wanted id (git clone
/// sends HEAD's and its branch's, the same id twice), `deepen 1`, `ofs-delta`, `done`, and no other argument but
/// `thin-pack`, `no-progress`, and `include-tag`. Only these wait for the shard to ready its chain; anything else goes to stock upload-pack at once. The container applies the
/// same rule (`composable` in `compose.go`) and adds the checks that need the repository: the want is a ref's tip and a
/// commit, the chain is at HEAD, and with `include-tag` no annotated tag points at it.
pub fn composable(req: &V2Request) -> bool {
    if req.command != "fetch" || req.caps.iter().any(|c| c.starts_with("object-format=") && c != "object-format=sha1") {
        return false;
    }
    let (mut want, mut depth1, mut ofs, mut done) = (None, false, false, false);
    for a in &req.args {
        let Ok(a) = std::str::from_utf8(a) else { return false };
        match a {
            "deepen 1" => depth1 = true,
            "ofs-delta" => ofs = true,
            "done" => done = true,
            a if OTHER_ARGS.contains(&a) => {}
            a => match a.strip_prefix("want ") {
                Some(w)
                    if w.len() == 40 && ObjectId::parse_hex(w.as_bytes()).is_some() && want.is_none_or(|o| o == w) =>
                {
                    want = Some(w)
                }
                _ => return false,
            },
        }
    }
    want.is_some() && depth1 && ofs && done
}

/// The container's reason when a composable want is not on the disk yet: the tip moved and this shard has not caught
/// up to it (`compose.go`, `composeMiss`). Retried after the running catch-up.
pub const NOT_YET: &str = "stock: not a ref tip or behind HEAD";

/// The container's reason when the want's tree is past `composeDiffMax` entries from the chain's: the chain is on a
/// tip far from HEAD. Retried after the chain's next step, which builds a new base in that case (`compose.go`,
/// `handleCompose`). On B0-34 m1b34 it was a chain built on the wrong tip, never a far want: over the row's 600 real
/// tip moves a want was at most 2,969 entries from the tip 20 moves back (`tests/am1/results/b034-attr.md`).
pub const OVER_CAP: &str = "stock: over the diff cap";

/// What a composable miss that the container answered 409 got in the end (`compose_then_catch_up`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterCompose<R> {
    /// Composed after the running catch-up, or after the chain's next step.
    Composed(R),
    /// Catch up as before, through a catch-up that starts after now, then serve (composed if the disk then allows it,
    /// else stock): the reason is neither `NOT_YET` nor `OVER_CAP`, `NOT_YET` with no catch-up running, or the retry
    /// still missed.
    CatchUp,
    /// The caller's deadline passed first: the shard answers 503 `warming` and the edge asks the next site.
    Late,
}

/// A composable miss after the container's first `compose=only` answer was 409 with `why`, all of it within
/// `deadline`, the one deadline of the whole upload-pack (the caller's `wait`).
/// - `NOT_YET` with a catch-up running (`running`): wait for it to end (`in_flight`) and ask composed again
///   (`compose`, `Some` on a 200). m41's B0-34 had 143 misses on a just-moved tip, all composed in the end.
/// - `OVER_CAP`: wait for any running catch-up, then for a chain step that starts after now (`step`), then ask
///   composed again.
/// - Anything else, or a retry that missed: `AfterCompose::CatchUp`, as it always did.
///
/// Past `deadline`, `AfterCompose::Late`. Unbounded, a miss waited out a whole stuck catch-up before its 15 s even
/// started: 25 of m1b34's 50 shard 503s, 29 to 99 s each (`tests/am1/results/b034-attr.md`).
pub async fn compose_then_catch_up<R, D, W, Fw, S, Fs, C, Fc>(
    deadline: D,
    why: Option<&str>,
    running: bool,
    in_flight: W,
    step: S,
    compose: C,
) -> AfterCompose<R>
where
    D: std::future::Future<Output = ()>,
    W: FnOnce() -> Fw,
    Fw: std::future::Future<Output = ()>,
    S: FnOnce() -> Fs,
    Fs: std::future::Future<Output = ()>,
    C: FnOnce() -> Fc,
    Fc: std::future::Future<Output = Option<R>>,
{
    let over = why == Some(OVER_CAP);
    if !(over || why == Some(NOT_YET) && running) {
        return AfterCompose::CatchUp;
    }
    let work = async {
        if running {
            in_flight().await;
        }
        if over {
            step().await;
        }
        match compose().await {
            Some(r) => AfterCompose::Composed(r),
            None => AfterCompose::CatchUp,
        }
    };
    match futures_util::future::select(std::pin::pin!(work), std::pin::pin!(deadline)).await {
        futures_util::future::Either::Left((a, _)) => a,
        futures_util::future::Either::Right(_) => AfterCompose::Late,
    }
}

/// How long a composable miss waits for its shard's chain step when the edge has another site to try (`next=1`)
/// [guess: site b composed B0-34 mt6's misses in 0.2 to 0.75 s from a synced chain]. Past it the shard answers 503
/// `warming` and the step goes on in the background (`decisions/DECISIONS.md`, "`chain-wait`").
pub const CHAIN_WAIT_FORWARDED: std::time::Duration = std::time::Duration::from_secs(2);

/// How many chain packs a composed answer opens in R2 at once [guess] (`decisions/DECISIONS.md`, "`chain-open`"): a
/// Workers invocation has six connections waiting for headers [doc], and an R2 `get` holds one until its headers come,
/// so four leave two for the request's other subrequests, as `crate::catchup::FETCH_AT_ONCE` does.
pub const CHAIN_OPEN_AT_ONCE: usize = 4;

/// Runs `opens`, at most `CHAIN_OPEN_AT_ONCE` at a time, and answers their values in the order given: the edge's
/// `open_chain`, which checked a base and up to 19 parts one after another while no header left. `None` as soon as one answers `None` (a chain pack missing or not the one the
/// manifest counted), and the first error, either way without starting the rest.
pub async fn open_in_order<T, E, F>(opens: impl IntoIterator<Item = F>) -> Result<Option<Vec<T>>, E>
where
    F: std::future::Future<Output = Result<Option<T>, E>>,
{
    use futures_util::StreamExt;
    let mut opened = futures_util::stream::iter(opens).buffered(CHAIN_OPEN_AT_ONCE);
    let mut out = Vec::new();
    while let Some(r) = opened.next().await {
        match r? {
            Some(t) => out.push(t),
            None => return Ok(None),
        }
    }
    Ok(Some(out))
}

/// How long the edge waits for a composed answer's chain checks (`open_chain`) before it refuses the clone with 503 and
/// `Retry-After` [guess: 20 checks took 0.4 to 0.5 s on graft-m1's R2, and the clone line is 5 s to the first byte]
/// (`decisions/DECISIONS.md`, "`chain-hold`"). The answer's first bytes leave as soon as the checks end.
pub const CHAIN_CHECK_WITHIN: std::time::Duration = std::time::Duration::from_secs(3);

/// How long a composed answer's stream waits for R2's next read (a pack's `get` or its next chunk) before it cuts the
/// answer short, which the client's index-pack refuses [guess] (`decisions/DECISIONS.md`, "`chain-hold`"): before it,
/// a stalled read held the client until its own timeout.
pub const CHAIN_STALL: std::time::Duration = std::time::Duration::from_secs(20);

/// `f`'s value, or `None` once `sleep(d)` ends first.
pub async fn within<T, S, D>(d: std::time::Duration, f: impl std::future::Future<Output = T>, sleep: S) -> Option<T>
where
    S: FnOnce(std::time::Duration) -> D,
    D: std::future::Future<Output = ()>,
{
    match futures_util::future::select(std::pin::pin!(f), std::pin::pin!(sleep(d))).await {
        futures_util::future::Either::Left((v, _)) => Some(v),
        futures_util::future::Either::Right(_) => None,
    }
}

/// What a composed answer's chain checks found.
#[derive(Debug, PartialEq, Eq)]
pub enum ChainCheck<T> {
    /// Every pack is there and is the one the manifest counted.
    Checked(T),
    /// One is missing or another pack: the edge asks the shard instead, as before.
    Declined,
    /// The checks did not end within `CHAIN_CHECK_WITHIN`: the edge refuses the clone with 503 and `Retry-After`.
    Late,
}

/// The edge's `open_chain`: each of `buckets` in turn, its `opens` run as `open_in_order` runs them, and the first
/// bucket whose checks all pass with their values; `Declined` only when every bucket declines, and all of it bounded
/// once by `CHAIN_CHECK_WITHIN` on `sleep`'s clock. `buckets` are the network's warm buckets, home first
/// (`chain_buckets`): a shard installs its chain from its own warm bucket, and the builder brings the buckets up one at
/// a time, so after a fold the home bucket can lack a chain the other still holds whole. On B0-34 mt10 the home shard's
/// kill stopped a fold's step after it deleted the old chain's files from the home bucket and before it reached
/// bucket b, whose shards then named the old chain for eight minutes; checked in the home bucket alone, every one of
/// those misses declined and composed on the shard (`decisions/DECISIONS.md`, "`chain-warm`").
pub async fn check_in_time<B, T, E, I, S, D>(
    buckets: impl IntoIterator<Item = B>,
    mut opens: impl FnMut(B) -> I,
    sleep: S,
) -> Result<ChainCheck<(B, Vec<T>)>, E>
where
    B: Copy,
    I: IntoIterator,
    I::Item: std::future::Future<Output = Result<Option<T>, E>>,
    S: FnOnce(std::time::Duration) -> D,
    D: std::future::Future<Output = ()>,
{
    let first = async {
        for b in buckets {
            if let Some(v) = open_in_order(opens(b)).await? {
                return Ok(Some((b, v)));
            }
        }
        Ok(None)
    };
    bounded(first, sleep).await
}

/// The chain checks' one bound: `first` (the first bucket that holds every pack, or none) within `CHAIN_CHECK_WITHIN`,
/// or `Late`.
async fn bounded<T, E, S, D>(
    first: impl std::future::Future<Output = Result<Option<T>, E>>,
    sleep: S,
) -> Result<ChainCheck<T>, E>
where
    S: FnOnce(std::time::Duration) -> D,
    D: std::future::Future<Output = ()>,
{
    Ok(match within(CHAIN_CHECK_WITHIN, first, sleep).await {
        None => ChainCheck::Late,
        Some(r) => r?.map_or(ChainCheck::Declined, ChainCheck::Checked),
    })
}

/// No header sets: every pack is checked, as before `colo-gate` (the test arm with the share off, and the edgebed).
pub struct NoSets;

impl SetStore for NoSets {
    fn enabled(&self) -> bool {
        false
    }
    async fn get(&self, _: usize, _: &str) -> Option<HeaderSet> {
        None
    }
    fn put(&self, _: usize, _: &str, _: HeaderSet) {}
    fn served(&self, _: Served) {}
}

/// Where an answer's chain packs' headers came from, for `server-timing`: a header set, the head object
/// (`head-etags`), or a check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Served {
    pub set: usize,
    pub head: usize,
    pub checked: usize,
}

/// One chain pack's checked header as a header set keeps it: the manifest's count, the R2 etag the check read, and the
/// body's length (the object less header and trailer).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub n: u32,
    pub etag: String,
    pub body: u64,
}

/// A chain bucket's checked headers under one version of its `head` object (`colo-gate`): only packs that head names,
/// each one checked after the head was read. Chain pack keys name commits, not bytes, and a fold deletes them, so a set
/// is never keyed by pack key alone; every put of `head` changes its version, and the builder puts a head's files
/// before that head and deletes an old base's files only after the head that replaces it, so every pack of the head an
/// edge read was there when it read it (`decisions/DECISIONS.md`, "`colo-gate`").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderSet {
    pub packs: std::collections::BTreeMap<String, Header>,
}

/// Where a chain bucket's header sets live (the isolate, and the data center's Cache API), by bucket index in the
/// order `check_with_sets` takes them and the head's version.
#[allow(async_fn_in_trait)]
pub trait SetStore {
    /// False: no head is read and no set is used, every pack is checked as before `colo-gate`.
    fn enabled(&self) -> bool;
    async fn get(&self, bucket: usize, version: &str) -> Option<HeaderSet>;
    fn put(&self, bucket: usize, version: &str, set: HeaderSet);
    /// Where the answer's packs' headers came from, for `server-timing`.
    fn served(&self, served: Served);
}

/// `check_in_time` with header sets. For each of `buckets` in turn, `head(b)` reads its `head` object's version and
/// contents (`None`: no head). A manifest pack that head names comes, at the manifest's count, from the set kept under
/// that version, else from the head's own record of the pack (`HeadObject::packs`, `head-etags`); every other pack is
/// checked by `check(b, key, n)`, `CHAIN_OPEN_AT_ONCE` at a time, in order. Either way each body still streams on the
/// etag it came with, so a pack replaced since ends the answer short. A bucket whose every check passed puts the set
/// again with each newly checked pack its head names; a failed check puts nothing. All of it is bounded once by
/// `CHAIN_CHECK_WITHIN`, as before.
pub async fn check_with_sets<B, E, HF, CF, S, D>(
    buckets: impl IntoIterator<Item = B>,
    m: &Manifest,
    head: impl Fn(B) -> HF,
    check: impl Fn(B, String, u32) -> CF,
    store: &impl SetStore,
    sleep: S,
) -> Result<ChainCheck<(B, Vec<(String, Header)>)>, E>
where
    B: Copy,
    HF: std::future::Future<Output = Result<Option<(String, HeadObject)>, E>>,
    CF: std::future::Future<Output = Result<Option<Header>, E>>,
    S: FnOnce(std::time::Duration) -> D,
    D: std::future::Future<Output = ()>,
{
    use futures_util::future::{Either, ready};
    let first = async {
        for (i, b) in buckets.into_iter().enumerate() {
            let named = head(b).await?;
            let set = match &named {
                Some((v, _)) => store.get(i, v).await.unwrap_or_default(),
                None => HeaderSet::default(),
            };
            let files: Vec<String> = named.as_ref().map(|(_, h)| h.chain.packs()).unwrap_or_default();
            let named_by_head = named.as_ref().map(|(_, h)| h.headers()).unwrap_or_default();
            let name_of = |key: &str| files.iter().find(|f| key.ends_with(&format!("/{f}.pack"))).cloned();
            let in_head = |key: &str| name_of(key).is_some();
            let known = |p: &ManifestPack| {
                let name = name_of(&p.key)?;
                let at_count = |h: &&Header| h.n == p.n;
                match set.packs.get(&p.key).filter(at_count) {
                    Some(h) => Some((h.clone(), Src::Set)),
                    None => named_by_head.get(&name).filter(at_count).map(|h| (h.clone(), Src::Head)),
                }
            };
            let opens = m.packs.iter().map(|p| match known(p) {
                Some((h, src)) => Either::Left(ready(Ok(Some((p.key.clone(), h, src))))),
                None => {
                    let key = p.key.clone();
                    let c = check(b, key.clone(), p.n);
                    Either::Right(async move { Ok(c.await?.map(|h| (key, h, Src::Checked))) })
                }
            });
            let Some(got) = open_in_order(opens).await? else { continue };
            let count = |s: Src| got.iter().filter(|(_, _, c)| *c == s).count();
            store.served(Served { set: count(Src::Set), head: count(Src::Head), checked: count(Src::Checked) });
            if let Some((v, _)) = &named {
                let mut next = set.clone();
                for (key, h, c) in &got {
                    if *c == Src::Checked && in_head(key) {
                        next.packs.insert(key.clone(), h.clone());
                    }
                }
                if next != set {
                    store.put(i, v, next);
                }
            }
            return Ok(Some((b, got.into_iter().map(|(k, h, _)| (k, h)).collect())));
        }
        Ok(None)
    };
    bounded(first, sleep).await
}

/// Where `check_with_sets` took one pack's header from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Src {
    Set,
    Head,
    Checked,
}

/// Streams a composed answer's chain packs one after another: `open` one, hand it to `send`, which lets its R2 body
/// go when it returns, and only then open the next. One invocation that holds eight R2 bodies open never gets a ninth
/// (graft-m1, 2026-11-20), and main's `open_chain` held every pack's body from its check to its turn, so a chain of nine
/// or more packs hung the clone with no byte sent (`decisions/DECISIONS.md`, "`chain-hold`"). The first error ends it.
pub async fn stream_in_turn<P, B, E>(
    packs: impl IntoIterator<Item = P>,
    mut open: impl AsyncFnMut(P) -> Result<B, E>,
    mut send: impl AsyncFnMut(B) -> Result<(), E>,
) -> Result<(), E> {
    for p in packs {
        let b = open(p).await?;
        send(b).await?;
    }
    Ok(())
}

/// Whether a shard takes the composed-miss chain's step after its own catch-up, before any miss asks for it: the
/// builder builds the next step, and a shard on a chain site (one of `crate::chain_buckets`) installs the
/// chain from the warm buckets, so a chain site that takes the misses when the home shard goes already holds a chain to
/// answer from (`decisions/DECISIONS.md`, "`chain-install`"). Any other shard installs only when a miss asks. On B0-34
/// mt12 site c, the other chain site, served none of the 46 misses before the kill and so held no chain; at the kill its
/// first miss waited 13.8 s for the install, and the misses behind it 0.5 to 10.9 s (`tests/am1/results/b034-mt12.md`).
pub fn steps_after_catch_up(
    network: graftyard_types::NetworkId,
    record: &graftyard_types::PlacementRecord,
    site: graftyard_types::SiteId,
    builder: bool,
) -> bool {
    builder || crate::chain_buckets(network, record).contains(&site)
}

/// Whether a composable miss goes on to ask its container (`true`) or the shard answers 503 `warming` (`false`), once
/// its chain is ready (`ready` ended) or its wait runs out. `follows`: the edge has another site to try (`next=1`).
/// `chain_site`: this site's bucket holds the network's chain (`crate::chain_buckets`, its warm sites).
/// - On a chain site, a forwarded miss waits at most `CHAIN_WAIT_FORWARDED` and the last site all `left`, what remains
///   of the upload-pack's deadline, so a miss never runs out of sites that would wait for a chain. As
///   `crate::warming_now` does for a rehydrate: on B0-34 mt6 a builder whose chain had not stepped after its rehydrate
///   held 38 misses for the home's whole 15 s, each then composed on site b in under 1 s
///   (`tests/am1/results/b034-mt6.md`, "1. Where the clone TTFB tail goes").
/// - On any other site it waits for nothing: a chain already synced there answers, a forwarded miss is otherwise sent
///   on at once, and the last site asks its container at once, which composes from a chain it holds or serves stock.
///   On B0-34 mt11 site c, which is not a warm site and holds no chain, waited `COMPOSE_WAIT`'s whole 20 s for each of
///   kill's last-site misses and 2 s for each forwarded one (`decisions/DECISIONS.md`, "`chain-site`").
pub async fn chain_ready<R, S, D>(
    follows: bool,
    chain_site: bool,
    left: std::time::Duration,
    ready: R,
    sleep: S,
) -> bool
where
    R: std::future::Future<Output = ()>,
    S: FnOnce(std::time::Duration) -> D,
    D: std::future::Future<Output = ()>,
{
    let wait = match (chain_site, follows) {
        (false, _) => std::time::Duration::ZERO,
        (true, true) => left.min(CHAIN_WAIT_FORWARDED),
        (true, false) => left,
    };
    let ready = std::pin::pin!(ready);
    match futures_util::future::select(ready, std::pin::pin!(sleep(wait))).await {
        futures_util::future::Either::Left(_) => true,
        futures_util::future::Either::Right(_) => !chain_site && !follows,
    }
}

/// `gate` and `lookup` run at once, and `lookup`'s value is kept only when `allows` holds of the gate's answer
/// (`decisions/DECISIONS.md`, "`hit-gate`"): the edge checks a clone-cache hit's manifest while its read gate reads the
/// repository's access rows, and sends nothing before the gate allows. A gate that refuses first drops the lookup
/// unfinished; one that refuses after it drops its value. On B0-34 mt11 a manifest hit ran the two one after the other,
/// and the gate's cold read (front p99 1,078 ms in j9) and the manifest's own read and chain checks summed into hits
/// over 1 s [measured, `tests/am1/results/b034-mt11.md`, "Hits over 1 s, by stage"].
pub async fn beside_gate<G, L>(
    gate: G,
    lookup: L,
    allows: impl FnOnce(&G::Output) -> bool,
) -> (G::Output, Option<L::Output>)
where
    G: std::future::Future,
    L: std::future::Future,
{
    use futures_util::future::{Either, select};
    match select(std::pin::pin!(gate), std::pin::pin!(lookup)).await {
        Either::Left((g, l)) => {
            if allows(&g) {
                let v = l.await;
                (g, Some(v))
            } else {
                (g, None)
            }
        }
        Either::Right((v, g)) => {
            let g = g.await;
            let ok = allows(&g);
            (g, ok.then_some(v))
        }
    }
}

/// The manifest line at the start of an answer read in chunks: the bytes before the first newline are held, at most
/// `MANIFEST_MAX` of them, and nothing after it, so the tail streams on from the chunk the line ended in.
#[derive(Debug, Default)]
pub struct LineSplit {
    held: Vec<u8>,
}

/// What one chunk did to a `LineSplit`.
#[derive(Debug, PartialEq, Eq)]
pub enum LineStep {
    /// No newline yet: read the next chunk.
    More,
    /// The line, its newline included, and where the tail starts in the chunk just pushed.
    Line(Vec<u8>, usize),
    /// No newline within `MANIFEST_MAX`: not a manifest answer.
    TooLong,
}

impl LineSplit {
    pub fn push(&mut self, chunk: &[u8]) -> LineStep {
        match chunk.iter().position(|b| *b == b'\n') {
            Some(i) if self.held.len() + i < MANIFEST_MAX => {
                let mut line = std::mem::take(&mut self.held);
                line.extend_from_slice(&chunk[..=i]);
                LineStep::Line(line, i + 1)
            }
            Some(_) => LineStep::TooLong,
            None if self.held.len() + chunk.len() >= MANIFEST_MAX => LineStep::TooLong,
            None => {
                self.held.extend_from_slice(chunk);
                LineStep::More
            }
        }
    }
}

/// Stock upload-packs one shard container runs at once [guess], as the container's `stockMax`: linux's took 2.6 GB of
/// RSS each, and m1b34's monorepo container ran 15 to 21 of them in 12 GiB, holding the network's lock shared 20 to
/// 99 s each, while catch-ups and every validate queued behind them (`tests/am1/results/b034-attr.md`). The next B0-34
/// row's stock-process count per sample (`shardprocs`) measures it. Past it the container answers 429 and the object
/// holds the request in its `StockLine`.
pub const STOCK_MAX: u32 = 2;

/// How often the head of a `StockLine` asks the container again [guess].
pub const STOCK_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// The stock upload-packs one shard object holds while its container's `STOCK_MAX` slots are full, in arrival order:
/// only the head asks the container again, so a later request never takes a slot ahead of an earlier one. A wait, not
/// a 429 to the client: git retries neither 429 nor 503 unless `http.maxRetries` is set, and then only 429 [doc,
/// git-config, `http.maxRetries`]. Each waits within its upload-pack's deadline and leaves the line past it.
#[derive(Debug, Default)]
pub struct StockLine {
    next: u64,
    waiting: std::collections::VecDeque<u64>,
}

impl StockLine {
    /// Whether a new stock request must queue: someone is already waiting.
    pub fn busy(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// A place at the back of the line.
    pub fn join(&mut self) -> u64 {
        self.next += 1;
        self.waiting.push_back(self.next);
        self.next
    }

    /// Whether `t` is at the front, the one place that asks the container.
    pub fn is_head(&self, t: u64) -> bool {
        self.waiting.front() == Some(&t)
    }

    /// Leaves the line: admitted, or past its deadline. The next place becomes the head.
    pub fn leave(&mut self, t: u64) {
        self.waiting.retain(|w| *w != t);
    }

    pub fn len(&self) -> usize {
        self.waiting.len()
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }
}

/// One stock upload-pack through `line` within `deadline`. `call` asks the container, `None` when its slots are full
/// (429); `pause` waits `STOCK_POLL`. A request that finds nobody waiting asks at once; otherwise, or once refused, it
/// joins the line, and only the head asks again. `None` past `deadline`, having left the line.
pub async fn stock_in_line<R, D, C, Fc, P, Fp>(
    line: &std::cell::RefCell<StockLine>,
    deadline: D,
    mut call: C,
    mut pause: P,
) -> Option<R>
where
    D: std::future::Future<Output = ()>,
    C: FnMut() -> Fc,
    Fc: std::future::Future<Output = Option<R>>,
    P: FnMut() -> Fp,
    Fp: std::future::Future<Output = ()>,
{
    struct Place<'a>(&'a std::cell::RefCell<StockLine>, Option<u64>);
    impl Drop for Place<'_> {
        fn drop(&mut self) {
            if let Some(t) = self.1 {
                self.0.borrow_mut().leave(t);
            }
        }
    }
    let work = async {
        if !line.borrow().busy()
            && let Some(r) = call().await
        {
            return r;
        }
        let place = Place(line, Some(line.borrow_mut().join()));
        let t = place.1.expect("joined");
        loop {
            pause().await;
            if line.borrow().is_head(t)
                && let Some(r) = call().await
            {
                return r;
            }
        }
    };
    match futures_util::future::select(std::pin::pin!(work), std::pin::pin!(deadline)).await {
        futures_util::future::Either::Left((r, _)) => Some(r),
        futures_util::future::Either::Right(_) => None,
    }
}

/// One clone-cache miss on a shard object, for its log line `shard miss <net>` (`workers/site/src/shard.rs`,
/// "/upload-pack"). Each time is epoch ms: when the miss arrived, `Compose::ready` returned, the `compose=only` answer
/// came (status and `x-graft-compose`), the catch-up before stock ended, the serving answer's headers came (the
/// container sends them with its first byte), and its first chunk went on to the edge. Beside the client's own time
/// to first byte, the line splits a miss into the wait for the chain, the container's work, and the transit.
#[derive(Clone, Debug, Default)]
pub struct MissLog {
    pub arrived: u64,
    pub ready: Option<u64>,
    pub only: Option<(u64, u16, Option<String>)>,
    pub caught_up: Option<u64>,
    pub served: Option<(u64, u16, Option<String>)>,
    pub first: Option<u64>,
}

impl MissLog {
    /// The line's JSON: `arrived` as epoch ms, every later time in ms after it, and `answer`, the serving answer's
    /// `x-graft-compose` (a 200 without one is stock upload-pack on a request the container found not composable).
    pub fn line(&self) -> serde_json::Value {
        let ms = |t: u64| t.saturating_sub(self.arrived);
        let answer = match &self.served {
            Some((_, 200, c)) => c.clone().unwrap_or_else(|| "stock: not composable".into()),
            Some((_, s, _)) => format!("status {s}"),
            None => "no answer".into(),
        };
        serde_json::json!({
            "arrived": self.arrived,
            "answer": answer,
            "ready_ms": self.ready.map(ms),
            "only": self.only.as_ref().map(|(t, s, c)| serde_json::json!({ "ms": ms(*t), "status": s, "compose": c })),
            "caught_up_ms": self.caught_up.map(ms),
            "headers_ms": self.served.as_ref().map(|(t, ..)| ms(*t)),
            "first_ms": self.first.map(ms),
        })
    }
}

// ---- edge-compose: the composed answer streamed from R2 ----

/// The `x-graft-compose` of a `compose=only&tail=1` answer under `edge-compose`: a `Manifest` line and the tail, not the
/// composed pack. The clone-cache entry it fills holds the same bytes under `R2Key::CloneManifest`.
pub const TAIL: &str = "tail";

/// The most bytes a manifest answer may hold: the line and the container's `tailMax` of 8 MiB. Past it the edge does not
/// read it and answers as a miss.
pub const TAIL_ANSWER_MAX: u64 = MANIFEST_MAX as u64 + (8 << 20);

/// The longest manifest line the edge reads before it gives up and answers as a miss [guess: 20 packs of about 130
/// bytes each, a fold's worth, are under 3 KB].
pub const MANIFEST_MAX: usize = 64 << 10;

/// The first line of a `compose=tail` answer and of a manifest clone-cache entry (`containers/git/server/compose.go`,
/// `manifestOf`): the chain's packs in pack order, each by its R2 key in the warm bucket with its object count, and the
/// tail that follows the line, the body of the want's own pack with no header or trailer. The pack the client gets is a
/// header of the total count, each chain pack's body (its bytes 12 to its length less 20), the tail, and a SHA-1
/// trailer of all of it, the header in its own side-band packet (`header_packet`) and the rest in `Framer`'s, the same
/// bytes as the container's composed answer (`TestComposedTailMatchesTheComposedAnswer`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub v: u32,
    pub want: String,
    pub packs: Vec<ManifestPack>,
    #[serde(rename = "tailN")]
    pub tail_n: u32,
    /// The tail's length in bytes.
    pub tail: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestPack {
    pub key: String,
    pub n: u32,
    /// The body's length on the container that wrote the line. The edge streams the bucket's copy at its own length.
    pub len: u64,
}

/// What reading the start of a manifest answer found.
#[derive(Debug, PartialEq, Eq)]
pub enum ManifestLine {
    /// The line, checked, and where the tail starts in the bytes read.
    Read(Manifest, usize),
    /// No newline yet within `MANIFEST_MAX`: read more.
    More,
    /// Not a manifest of this network: the answer is not used.
    Bad,
}

impl Manifest {
    /// The manifest line at the start of `head`, checked against `network`: version 1, a full SHA-1 want, and every
    /// key a chain pack of this network (`R2Key::Compose`, a `.pack` file).
    pub fn read(head: &[u8], network: graftyard_types::NetworkId) -> ManifestLine {
        let Some(nl) = head.iter().position(|b| *b == b'\n') else {
            return if head.len() < MANIFEST_MAX { ManifestLine::More } else { ManifestLine::Bad };
        };
        let Ok(m) = serde_json::from_slice::<Manifest>(&head[..nl]) else { return ManifestLine::Bad };
        let pack_ok = |p: &ManifestPack| {
            matches!(p.key.parse::<crate::key::R2Key>(),
                Ok(crate::key::R2Key::Compose { network: n, file }) if n == network && file.ends_with(".pack"))
        };
        let want_ok = ObjectId::parse_hex(m.want.as_bytes()).is_some();
        if m.v != 1 || !want_ok || m.packs.is_empty() || !m.packs.iter().all(pack_ok) {
            return ManifestLine::Bad;
        }
        ManifestLine::Read(m, nl + 1)
    }

    /// The answer's object count: every chain pack's and the tail's. `None` past `u32`.
    pub fn count(&self) -> Option<u32> {
        self.packs.iter().try_fold(self.tail_n, |n, p| n.checked_add(p.n))
    }

    /// The v2 fetch answer's bytes before the pack: the shallow line for the depth-1 want and the `packfile` section
    /// header, as the container's composed answer writes them.
    pub fn prefix(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for l in ["shallow-info\n".to_owned(), format!("shallow {}", self.want)] {
            out.extend(format!("{:04x}{l}", l.len() + 4).bytes());
        }
        out.extend(b"0001");
        out.extend(format!("{:04x}packfile\n", 13).bytes());
        out
    }
}

/// A chain pack's object count from its first 12 bytes: `PACK`, version 2, then the count. `None` for anything else.
pub fn pack_count(head: &[u8]) -> Option<u32> {
    (head.len() >= 12 && &head[..8] == b"PACK\0\0\0\x02").then(|| u32::from_be_bytes(head[8..12].try_into().unwrap()))
}

/// The pack header for `count` objects.
pub fn pack_header(count: u32) -> [u8; 12] {
    let mut h = *b"PACK\0\0\0\x02\0\0\0\0";
    h[8..].copy_from_slice(&count.to_be_bytes());
    h
}

/// The most pack bytes in one side-band packet, as upload-pack's side-band-64k sends: 65,520 less the 4-byte length
/// and the band byte.
pub const SIDEBAND_MAX: u64 = 65_515;

/// The pack header as its own side-band packet: the container's composed answer flushes it before the chain's bodies,
/// so the client's first pack byte comes before any R2 read, and the edge's answer is the same bytes.
pub fn header_packet(count: u32) -> [u8; 17] {
    let mut p = [0u8; 17];
    p[..5].copy_from_slice(b"0011\x01");
    p[5..].copy_from_slice(&pack_header(count));
    p
}

/// The answer's whole length for a `prefix` of `prefix` bytes and a pack of `pack` bytes (header, bodies, tail, and
/// trailer): the header's packet, the rest in `Framer`'s packets, then the closing flush. Known before the first byte,
/// so the edge streams it through a fixed-length stream and a cut answer can never look whole.
pub fn answer_len(prefix: u64, pack: u64) -> u64 {
    let rest = pack - 12;
    prefix + 17 + rest.div_ceil(SIDEBAND_MAX) * 5 + rest + 4
}

/// One step of writing pack bytes in side-band packets: a packet's 5-byte header, or bytes `start..end` of the chunk
/// being written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece {
    Header([u8; 5]),
    Data(usize, usize),
}

/// Cuts the pack after its header (`header_packet`), of known length, into side-band packets of `SIDEBAND_MAX` bytes,
/// the last one shorter, whatever sizes its bytes arrive in: each packet's header gives its length before its first byte, so the edge writes R2's chunks
/// as they come, as views, and never holds a packet.
#[derive(Clone, Debug)]
pub struct Framer {
    left: u64,
    in_packet: u64,
}

impl Framer {
    pub fn new(pack: u64) -> Framer {
        Framer { left: pack, in_packet: 0 }
    }

    /// The pieces that write the next `len` pack bytes. `None` when they pass the pack's length.
    pub fn pieces(&mut self, len: usize) -> Option<Vec<Piece>> {
        if len as u64 > self.left + self.in_packet {
            return None;
        }
        let mut out = Vec::new();
        let mut at = 0;
        while at < len {
            if self.in_packet == 0 {
                let n = self.left.min(SIDEBAND_MAX);
                let mut h = [1u8; 5];
                h[..4].copy_from_slice(format!("{:04x}", n + 5).as_bytes());
                out.push(Piece::Header(h));
                self.in_packet = n;
                self.left -= n;
            }
            let k = (self.in_packet as usize).min(len - at);
            out.push(Piece::Data(at, at + k));
            at += k;
            self.in_packet -= k as u64;
        }
        Some(out)
    }

    /// Whether every byte of the pack was written.
    pub fn done(&self) -> bool {
        self.left == 0 && self.in_packet == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smart::parse_v2;

    /// Every time is from the miss's arrival, not from the step before it, and the answer names stock unless the
    /// serving answer says composed: a 409 then a stock 200 after the catch-up is stock with the 409's reason kept.
    #[test]
    fn a_miss_line_times_each_step_from_arrival_and_names_its_answer() {
        let stock = MissLog {
            arrived: 1_000_000,
            ready: Some(1_000_003),
            only: Some((1_000_010, 409, Some("stock: no chain".into()))),
            caught_up: Some(1_004_000),
            served: Some((1_009_000, 200, Some("stock: no chain".into()))),
            first: Some(1_009_250),
        };
        let l = stock.line();
        assert_eq!(l["arrived"], 1_000_000);
        assert_eq!(l["ready_ms"], 3);
        assert_eq!(l["only"], serde_json::json!({ "ms": 10, "status": 409, "compose": "stock: no chain" }));
        assert_eq!(l["caught_up_ms"], 4_000);
        assert_eq!(l["headers_ms"], 9_000);
        assert_eq!(l["first_ms"], 9_250);
        assert_eq!(l["answer"], "stock: no chain");
        let bare = MissLog { arrived: 5, served: Some((7, 200, None)), ..MissLog::default() };
        assert_eq!(bare.line()["answer"], "stock: not composable");
        assert_eq!(bare.line()["first_ms"], serde_json::Value::Null);
        let warming = MissLog { arrived: 5, served: Some((9, 503, None)), ..MissLog::default() };
        assert_eq!(warming.line()["answer"], "status 503");
        assert_eq!(MissLog { arrived: 5, ..MissLog::default() }.line()["answer"], "no answer");
    }

    fn pkt(s: &str) -> String {
        format!("{:04x}{s}", s.len() + 4)
    }

    fn fetch(args: &[&str]) -> V2Request {
        let mut b = pkt("command=fetch\n") + &pkt("agent=git/2.56\n") + &pkt("object-format=sha1\n") + "0001";
        for a in args {
            b += &pkt(&format!("{a}\n"));
        }
        parse_v2((b + "0000").as_bytes()).unwrap()
    }

    const W: &str = "want 0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn only_a_one_want_depth_one_ofs_delta_fetch_is_composable() {
        // As `git clone --depth 1` asks it.
        let clone = ["thin-pack", "no-progress", "include-tag", "ofs-delta", "deepen 1", W, "done"];
        assert!(composable(&fetch(&clone)));
        assert!(composable(&fetch(&["ofs-delta", "deepen 1", W, "done"])));
        assert!(composable(&fetch(&[&clone[..], &[W]].concat())), "the same want twice, as git clone sends it");
        let without = |drop: &str| -> Vec<&str> { clone.iter().copied().filter(|a| *a != drop).collect() };
        for (case, args) in [
            ("no ofs-delta", without("ofs-delta")),
            ("no deepen", without("deepen 1")),
            ("no done", without("done")),
            ("no want", without(W)),
            ("two wants", [&clone[..], &["want 1123456789abcdef0123456789abcdef01234567"]].concat()),
            ("deepen 2", [&without("deepen 1")[..], &["deepen 2"]].concat()),
            ("a have", [&clone[..], &["have 1123456789abcdef0123456789abcdef01234567"]].concat()),
            ("a client shallow", [&clone[..], &["shallow 1123456789abcdef0123456789abcdef01234567"]].concat()),
            ("a filter", [&clone[..], &["filter blob:none"]].concat()),
            ("deepen-since", [&clone[..], &["deepen-since 1700000000"]].concat()),
            ("a short want", [&without(W)[..], &["want 0123"]].concat()),
            ("want-ref", [&without(W)[..], &["want-ref refs/heads/main"]].concat()),
        ] {
            assert!(!composable(&fetch(&args)), "{case}");
        }
        let mut sha256 = fetch(&clone);
        sha256.caps = vec!["object-format=sha256".into()];
        assert!(!composable(&sha256), "sha256");
        let mut ls = fetch(&clone);
        ls.command = "ls-refs".into();
        assert!(!composable(&ls), "ls-refs");
    }

    fn id(c: char) -> String {
        c.to_string().repeat(40)
    }

    #[test]
    fn a_shard_fetches_only_the_files_its_chain_lacks() {
        let h = ChainHead { base: id('a'), parts: vec![id('b'), id('c')], tree: id('d') };
        assert_eq!(h.tip(), id('c'));
        assert_eq!(
            h.packs(),
            [
                format!("base-{}", id('a')),
                format!("part-{}-{}", id('a'), id('b')),
                format!("part-{}-{}", id('b'), id('c'))
            ]
        );
        assert!(h.files().iter().all(|f| file_name_ok(f)));
        assert_eq!(h.to_fetch(None).len(), 6);
        // One move behind: only the new part.
        let had = ChainHead { base: id('a'), parts: vec![id('b')], tree: id('e') };
        assert_eq!(
            h.to_fetch(Some(&had)),
            [format!("part-{}-{}.pack", id('b'), id('c')), format!("part-{}-{}.idx", id('b'), id('c'))]
        );
        // After a fold the old files are no help.
        let folded = ChainHead { base: id('c'), parts: vec![], tree: id('d') };
        assert_eq!(folded.to_fetch(Some(&h)), [format!("base-{}.pack", id('c')), format!("base-{}.idx", id('c'))]);
        // A part to the same tip from another base is a different file.
        let other = ChainHead { base: id('f'), parts: vec![id('c')], tree: id('d') };
        assert_eq!(other.to_fetch(Some(&h)).len(), 4);
        assert_eq!(h.to_fetch(Some(&h)), Vec::<String>::new());
        let base_only = ChainHead { base: id('a'), parts: vec![], tree: id('d') };
        assert_eq!(base_only.tip(), id('a'));
    }

    /// `head-etags`: the packs a head records sit outside every chain comparison, carry only for objects this step
    /// did not put, and never take the head past `HEAD_MAX`.
    #[test]
    fn a_heads_pack_records_sit_beside_its_chain() {
        let h = ChainHead { base: id('a'), parts: vec![id('b'), id('c')], tree: id('d') };
        let rec = |e: &str| HeadPack(format!("\"{e}\""), 4_000, 7);
        let old = HeadObject::new(h.clone(), vec![rec("1"), rec("2"), rec("3")]);
        assert_eq!(old.packs.len(), 3);
        // The same chain with other records is the same step: the builder puts nothing again.
        assert_eq!(HeadObject::plan(Some(&old), &h), None);
        let bare = HeadObject::new(h.clone(), vec![]);
        assert_eq!(HeadObject::plan(Some(&bare), &h), None, "a head without packs is the same step too");
        // A moved chain puts only its new files, and carries the records of the packs it did not put.
        let next = ChainHead { base: id('a'), parts: vec![id('b'), id('c'), id('e')], tree: id('f') };
        let put = HeadObject::plan(Some(&old), &next).unwrap();
        let new_part = format!("part-{}-{}", id('c'), id('e'));
        assert_eq!(put, [format!("{new_part}.pack"), format!("{new_part}.idx")]);
        let carried = HeadObject::carried(Some(&old), &next, &put);
        assert_eq!(
            carried,
            vec![
                (format!("base-{}", id('a')), Some(rec("1"))),
                (format!("part-{}-{}", id('a'), id('b')), Some(rec("2"))),
                (format!("part-{}-{}", id('b'), id('c')), Some(rec("3"))),
                (new_part.clone(), None),
            ]
        );
        // A pack put again under its old name (a rebuild) is read back, never carried.
        let again = [format!("base-{}.pack", id('a'))];
        assert_eq!(HeadObject::carried(Some(&old), &h, &again)[0].1, None);
        assert_eq!(HeadObject::carried(Some(&old), &h, &again)[1].1, Some(rec("2")));
        // An old head without records, or whose records do not cover its chain, carries nothing.
        assert!(HeadObject::carried(Some(&bare), &h, &[]).iter().all(|(_, k)| k.is_none()));
        let short = HeadObject { chain: h.clone(), packs: vec![rec("1")] };
        assert!(HeadObject::carried(Some(&short), &h, &[]).iter().all(|(_, k)| k.is_none()));
        // After a fold the new base is put and nothing carries.
        let folded = ChainHead { base: id('c'), parts: vec![], tree: id('d') };
        let put = HeadObject::plan(Some(&old), &folded).unwrap();
        assert_eq!(HeadObject::carried(Some(&old), &folded, &put), vec![(format!("base-{}", id('c')), None)]);

        // Records that do not name every pack are dropped, and so are records past HEAD_MAX.
        assert!(HeadObject::new(h.clone(), vec![rec("1"), rec("2")]).packs.is_empty());
        // A chain holds at most `foldEvery` (20) packs. Twenty with realistic etags, sizes and counts fit; twenty with
        // u32-max sizes and counts do not, and fall back to the chain alone.
        let hex = |i: usize| format!("{i:040x}");
        let deep = ChainHead { base: hex(1), parts: (2..21).map(hex).collect(), tree: hex(0) };
        let recs = |size: u64, n: u32| (0..20).map(|i| HeadPack(format!("{i:032x}"), size, n)).collect();
        let fits = HeadObject::new(deep.clone(), recs(90_000_000, 40_000));
        assert_eq!(fits.packs.len(), 20);
        assert!(serde_json::to_vec(&fits).unwrap().len() <= HEAD_MAX);
        let over = HeadObject::new(deep.clone(), recs(u32::MAX as u64, u32::MAX));
        assert!(over.packs.is_empty(), "a head past HEAD_MAX keeps its chain alone");
        assert_eq!(over.chain, deep);

        // On the wire: the chain's fields flat, so the container's ChainHead reads a head with records as its chain,
        // and a head without records reads back as one with none.
        let bytes = serde_json::to_vec(&old).unwrap();
        assert_eq!(serde_json::from_slice::<ChainHead>(&bytes).unwrap(), h);
        assert_eq!(serde_json::from_slice::<HeadObject>(&bytes).unwrap(), old);
        let plain = serde_json::to_vec(&h).unwrap();
        assert_eq!(serde_json::from_slice::<HeadObject>(&plain).unwrap(), bare);
        assert_eq!(serde_json::to_vec(&bare).unwrap(), plain);
        // Headers: by pack name, body less the 32 bytes a check also takes off.
        let hs = old.headers();
        assert_eq!(hs[&format!("base-{}", id('a'))], Header { n: 7, etag: "\"1\"".into(), body: 3_968 });
        assert!(bare.headers().is_empty() && short.headers().is_empty());
    }

    #[test]
    fn heads_and_names_are_checked() {
        assert!(ChainHead { base: id('a'), parts: vec![id('b')], tree: id('c') }.checked().is_some());
        assert!(ChainHead { base: id('A'), parts: vec![], tree: id('c') }.checked().is_none());
        assert!(ChainHead { base: id('a'), parts: vec!["b".into()], tree: id('c') }.checked().is_none());
        assert!(ChainHead { base: id('a'), parts: vec![], tree: "../x".into() }.checked().is_none());
        let h: ChainHead = serde_json::from_str(&format!(r#"{{"base":"{}","tree":"{}"}}"#, id('a'), id('b'))).unwrap();
        assert!(h.parts.is_empty());
        for bad in [
            "base-x.pack",
            "base-.pack",
            "part-aa-bb.idx",
            &format!("base-{}.rev", id('a')),
            &format!("base-{}", id('a')),
            &format!("base-{}-{}.pack", id('a'), id('b')),
            &format!("part-{}.pack", id('a')),
            &format!("../base-{}.pack", id('a')),
            &format!("base-{}.pack", id('A')),
        ] {
            assert!(!file_name_ok(bad), "{bad}");
        }
    }

    fn manifest_line(net: i64, packs: &[(&str, u32)], tail: u64) -> String {
        let packs: Vec<serde_json::Value> = packs
            .iter()
            .map(|(k, n)| serde_json::json!({ "key": format!("compose/{net}/{k}"), "n": n, "len": 100 }))
            .collect();
        serde_json::json!({ "v": 1, "want": id('a'), "packs": packs, "tailN": 3, "tail": tail }).to_string()
    }

    /// A manifest is read only as a whole line of this network's chain packs, and its tail starts after the newline.
    #[test]
    fn a_manifest_names_only_this_networks_chain_packs() {
        use graftyard_types::{DbId, NetworkId};
        let net = NetworkId(DbId(7));
        let base = format!("base-{}.pack", id('b'));
        let line = manifest_line(7, &[(&base, 10), (&format!("part-{}-{}.pack", id('b'), id('c')), 4)], 5);
        let mut b = line.clone().into_bytes();
        b.push(b'\n');
        b.extend(b"TAIL!");
        let ManifestLine::Read(m, at) = Manifest::read(&b, net) else { panic!("{line}") };
        assert_eq!((at, &b[at..]), (line.len() + 1, &b"TAIL!"[..]));
        assert_eq!(m.count(), Some(17), "10 + 4 + the tail's 3");
        // Half a line asks for more, until MANIFEST_MAX.
        assert_eq!(Manifest::read(&b[..line.len()], net), ManifestLine::More);
        assert_eq!(Manifest::read(&vec![b' '; MANIFEST_MAX], net), ManifestLine::Bad);
        let bad = |l: String| Manifest::read(format!("{l}\n").as_bytes(), net);
        assert_eq!(bad(manifest_line(8, &[(&base, 1)], 0)), ManifestLine::Bad, "another network's pack");
        assert_eq!(bad(manifest_line(7, &[(&format!("base-{}.idx", id('b')), 1)], 0)), ManifestLine::Bad, "an .idx");
        assert_eq!(bad(manifest_line(7, &[("head", 1)], 0)), ManifestLine::Bad, "the head");
        assert_eq!(bad(manifest_line(7, &[], 0)), ManifestLine::Bad, "no packs");
        assert_eq!(bad(line.replace(&id('a'), &id('A'))), ManifestLine::Bad, "an upper-case want");
        assert_eq!(bad(line.replace("\"v\":1", "\"v\":2")), ManifestLine::Bad, "another version");
        let over = manifest_line(7, &[(&base, u32::MAX)], 0);
        let ManifestLine::Read(m, _) = bad(over) else { panic!() };
        assert_eq!(m.count(), None, "a count past u32");
    }

    /// The framer's packets are the container's side-band framing whatever sizes R2's chunks come in: the same bytes
    /// as framing the pack whole, and `answer_len` is their length with the prefix and the flush.
    #[test]
    fn the_framer_frames_any_chunking_as_the_container_does() {
        // upload-pack's side-band-64k packet: 65,520 bytes, 65,515 of them data.
        let pack: Vec<u8> = (0..(2 * 65_515 + 1000)).map(|i| (i * 7 % 251) as u8).collect();
        let mut want = header_packet(42).to_vec();
        assert_eq!(&want[5..], &pack_header(42)[..]);
        let mut hdr = pack_header(42).to_vec();
        hdr.extend(&pack);
        for c in pack.chunks(65_515) {
            want.extend(format!("{:04x}", c.len() + 5).bytes());
            want.push(1);
            want.extend(c);
        }
        for chunk in [1usize, 4096, 65_515, 65_516, 200_000] {
            let mut f = Framer::new(pack.len() as u64);
            let mut got = header_packet(42).to_vec();
            for c in pack.chunks(chunk) {
                for p in f.pieces(c.len()).unwrap() {
                    match p {
                        Piece::Header(h) => got.extend(h),
                        Piece::Data(a, b) => got.extend(&c[a..b]),
                    }
                }
            }
            assert!(f.done(), "chunk {chunk}");
            assert_eq!(got, want, "chunk {chunk}");
            assert_eq!(f.pieces(1), None, "past the end");
        }
        let prefix = 30;
        assert_eq!(answer_len(prefix, hdr.len() as u64), prefix + want.len() as u64 + 4);
        let mut f = Framer::new(10);
        assert!(f.pieces(4).is_some() && !f.done());
        assert_eq!(f.pieces(7), None, "7 more of 6 left");
        assert_eq!(pack_count(&pack_header(9)), Some(9));
        assert_eq!(pack_count(b"PACK\0\0\0\x03\0\0\0\x09"), None, "version 3");
        let m = Manifest { v: 1, want: id('a'), packs: vec![], tail_n: 0, tail: 0 };
        assert_eq!(m.prefix(), format!("0011shallow-info\n0034shallow {}0001000dpackfile\n", id('a')).into_bytes());
    }
}
