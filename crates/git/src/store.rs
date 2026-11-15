//! `PackStore::place` and the `Committed` marker (`design/DESIGN.md`, Shape, "Packs"; journey 1, steps 2 and 3).

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use graftyard_types::{ChainId, Digest, Index, NetworkId, Region, SiteId, SiteSet, SiteTable};
use sha2::{Digest as _, Sha256};

use crate::bucket::{Bucket, PutNew, StoreError};
use crate::key::R2Key;
use crate::pack::{DurablePack, INLINE_MAX, Marker, ObjectMeta, PackId, PackIndex, PackRef};

/// How many copies make a stored pack durable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Copies {
    /// Two sites in two regions: every placement but `Solo`.
    TwoRegions,
    /// One bucket, for `Solo`.
    Solo,
}

/// One bucket's answer to a create-only put of the pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PutOutcome {
    Created,
    /// The key existed with the same SHA-256: an earlier attempt of this placement put the same bytes.
    ExistedSame,
    /// The key existed with other bytes, or with no checksum to compare. Never counts.
    ExistedOther,
    Failed(String),
}

impl PutOutcome {
    fn holds(&self) -> bool {
        matches!(self, PutOutcome::Created | PutOutcome::ExistedSame)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PlaceError {
    #[error("the bytes are not the ones validate indexed")]
    NotValidated,
    #[error("held by {held} buckets in {regions} regions")]
    TooFew { held: usize, regions: usize },
    #[error("site {0:?} is not in the site table")]
    UnknownSite(SiteId),
}

/// The two-of-three rule: the sites whose bucket holds the pack, once they span two regions (one bucket for `Solo`).
/// `Err` while the rule does not hold. Pure, so the rule is tested apart from any bucket.
pub fn durable_sites(results: &[(SiteId, Region, PutOutcome)], copies: Copies) -> Result<SiteSet, PlaceError> {
    let held: Vec<(SiteId, Region)> = results.iter().filter(|(_, _, o)| o.holds()).map(|(s, r, _)| (*s, *r)).collect();
    let mut regions: Vec<Region> = held.iter().map(|(_, r)| *r).collect();
    regions.sort_by_key(|r| r.hint());
    regions.dedup();
    let need = match copies {
        Copies::TwoRegions => 2,
        Copies::Solo => 1,
    };
    if regions.len() >= need {
        Ok(SiteSet::of(held.iter().map(|(s, _)| *s)))
    } else {
        Err(PlaceError::TooFew { held: held.len(), regions: regions.len() })
    }
}

pub struct PackStore<'a, B> {
    pub sites: &'a SiteTable,
    /// Every bucket the pack is teed to: the three site buckets of the network's placement.
    pub buckets: &'a [(SiteId, &'a B)],
    pub copies: Copies,
}

/// One placement. `index` is what the shard's `validate` returned for these bytes.
pub struct Place<'a> {
    pub network: NetworkId,
    pub id: PackId,
    pub meta: ObjectMeta,
    pub bytes: &'a [u8],
    pub index: &'a PackIndex,
    /// The shard's `.idx`, put beside the pack in each bucket that holds it. Derived: a failure is not an error.
    pub idx: Option<&'a [u8]>,
    /// A promotion's range, written as the `.idx`'s `from` and `to` custom metadata beside the pack's chain: a cache
    /// of its `Promoted`, which a rehydrate checks against the record (`crate::catchup::skippable`).
    pub idx_range: Option<(Index, Index)>,
    /// `INLINE_MAX`, or `INLINE_MAX_HOT` for an agent push on a hot group.
    pub inline_max: usize,
}

impl<B: Bucket> PackStore<'_, B> {
    /// The only constructor of `DurablePack`. Inline when the pack fits `inline_max`. Otherwise create-only puts to
    /// every bucket at once, returning as soon as the two-of-three rule holds; a put still running finishes or fails
    /// on its own, and the third-bucket filler copies a pack the third bucket lacks once its marker exists.
    pub async fn place(&self, p: Place<'_>) -> Result<DurablePack, PlaceError> {
        let checksum = Digest(Sha256::digest(p.bytes).into());
        if checksum != p.index.checksum || p.bytes.len() as u64 != p.index.bytes {
            return Err(PlaceError::NotValidated);
        }
        if p.bytes.len() <= p.inline_max.min(INLINE_MAX) {
            return Ok(DurablePack::seal(PackRef::Inline(p.bytes.to_vec())));
        }
        let key = R2Key::Pack { network: p.network, id: p.id }.to_string();
        let meta = p.meta.to_pairs().to_vec();
        let mut puts = FuturesUnordered::new();
        for (site, bucket) in self.buckets {
            let region = self.sites.get(*site).ok_or(PlaceError::UnknownSite(*site))?.region;
            let (key, meta) = (&key, &meta);
            puts.push(async move {
                let outcome = match bucket.put_new(key, p.bytes, checksum, meta).await {
                    Ok(PutNew::Created) => PutOutcome::Created,
                    Ok(PutNew::Exists(Some(d))) if d == checksum => PutOutcome::ExistedSame,
                    Ok(PutNew::Exists(_)) => PutOutcome::ExistedOther,
                    Err(StoreError(e)) => PutOutcome::Failed(e),
                };
                (*site, region, outcome)
            });
        }
        let mut results = Vec::new();
        let mut verdict = Err(PlaceError::TooFew { held: 0, regions: 0 });
        while let Some(r) = puts.next().await {
            results.push(r);
            verdict = durable_sites(&results, self.copies);
            if verdict.is_ok() {
                break;
            }
        }
        let sites = verdict?;
        drop(puts);
        if let Some(idx) = p.idx {
            self.put_idx(p.network, p.id, p.meta, idx, p.idx_range, sites).await;
        }
        Ok(DurablePack::seal(PackRef::Stored { id: p.id, checksum, bytes: p.bytes.len() as u64, sites }))
    }

    /// A placed pack's `.idx` beside it in each bucket of `sites`, create-only, with the pack's chain and, for a
    /// promotion, its range as `from` and `to`. Derived: a failed put is not an error.
    async fn put_idx(
        &self,
        network: NetworkId,
        id: PackId,
        meta: ObjectMeta,
        idx: &[u8],
        range: Option<(Index, Index)>,
        sites: SiteSet,
    ) {
        let idx_key = R2Key::Idx { network, id }.to_string();
        let idx_sum = Digest(Sha256::digest(idx).into());
        let mut idx_meta = meta.to_pairs().to_vec();
        if let Some((from, to)) = range {
            idx_meta.extend([("from".to_owned(), from.0.to_string()), ("to".to_owned(), to.0.to_string())]);
        }
        for (_, bucket) in self.buckets.iter().filter(|(s, _)| sites.contains(*s)) {
            let _ = bucket.put_new(&idx_key, idx, idx_sum, &idx_meta).await;
        }
    }
}

/// Multipart part size of the edge's tee [guess]. A pack of at most one part is put whole, with its SHA-256.
pub const PART_SIZE: usize = 16 << 20;

/// One push's pack as the edge stages it, before `validate` has answered.
pub struct Stage<'a> {
    pub network: NetworkId,
    pub id: PackId,
    pub meta: ObjectMeta,
    pub bytes: &'a [u8],
    /// The edge chose inline: the pack fits `INLINE_MAX` and the encoded push with it stays under `COMMAND_MAX`.
    pub inline: bool,
    /// Multipart part size, `PART_SIZE` unless a test bed lowers it (R2's floor is 5 MiB).
    pub part: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TeeError<E> {
    #[error("validate: {0}")]
    Invalid(E),
    #[error(transparent)]
    Place(#[from] PlaceError),
}

impl<B: Bucket> PackStore<'_, B> {
    /// The edge's tee (`design/DESIGN.md`, journey 1, step 2): the puts to every bucket run at once with `validate`
    /// on the same bytes. A `DurablePack` comes back only once `validate` passed on bytes whose SHA-256 is these
    /// bytes' and the two-of-three rule holds. An inline pack skips the buckets. On a failure, the copies already put
    /// stay for the sweeper, which deletes them after 24 h because no `request` row names their pack.
    pub async fn place_teed<F, E>(&self, s: Stage<'_>, validate: F) -> Result<(DurablePack, PackIndex), TeeError<E>>
    where
        F: std::future::Future<Output = Result<PackIndex, E>>,
    {
        let checksum = Digest(Sha256::digest(s.bytes).into());
        let check = |ix: &PackIndex| -> Result<(), PlaceError> {
            if ix.checksum != checksum || ix.bytes != s.bytes.len() as u64 {
                return Err(PlaceError::NotValidated);
            }
            Ok(())
        };
        if s.inline {
            let ix = validate.await.map_err(TeeError::Invalid)?;
            check(&ix)?;
            if s.bytes.len() > INLINE_MAX {
                return Err(PlaceError::TooFew { held: 0, regions: 0 }.into());
            }
            return Ok((DurablePack::seal(PackRef::Inline(s.bytes.to_vec())), ix));
        }
        let key = R2Key::Pack { network: s.network, id: s.id }.to_string();
        let journal = R2Key::Upload { network: s.network, id: s.id }.to_string();
        let meta = s.meta.to_pairs().to_vec();
        let puts = async {
            let mut puts = FuturesUnordered::new();
            for (site, bucket) in self.buckets {
                let region = self.sites.get(*site).ok_or(PlaceError::UnknownSite(*site))?.region;
                let (key, journal, meta) = (&key, &journal, &meta);
                puts.push(async move {
                    let r = if s.bytes.len() > s.part {
                        bucket.put_new_parts(key, journal, s.bytes, checksum, meta, s.part).await
                    } else {
                        bucket.put_new(key, s.bytes, checksum, meta).await
                    };
                    let outcome = match r {
                        Ok(PutNew::Created) => PutOutcome::Created,
                        Ok(PutNew::Exists(Some(d))) if d == checksum => PutOutcome::ExistedSame,
                        Ok(PutNew::Exists(_)) => PutOutcome::ExistedOther,
                        Err(StoreError(e)) => PutOutcome::Failed(e),
                    };
                    (*site, region, outcome)
                });
            }
            let mut results = Vec::new();
            let mut verdict = Err(PlaceError::TooFew { held: 0, regions: 0 });
            while let Some(r) = puts.next().await {
                results.push(r);
                verdict = durable_sites(&results, self.copies);
                if verdict.is_ok() {
                    break;
                }
            }
            verdict
        };
        let (sites, ix) = futures_util::future::join(puts, validate).await;
        let ix = ix.map_err(TeeError::Invalid)?;
        check(&ix)?;
        let sites = sites?;
        Ok((DurablePack::seal(PackRef::Stored { id: s.id, checksum, bytes: s.bytes.len() as u64, sites }), ix))
    }
}

/// One push's pack as the edge streams it (`PackStore::place_streamed`): where it goes, before any byte of it.
pub struct Streamed {
    pub network: NetworkId,
    pub id: PackId,
    pub meta: ObjectMeta,
    /// Multipart part size, `PART_SIZE` unless a test bed lowers it (R2's floor is 5 MiB).
    pub part: usize,
}

/// Where the streamed tee hands the pack's bytes on to `validate` while it puts them.
#[allow(async_fn_in_trait)]
pub trait Feed {
    /// Hands `b` on. False once validate no longer reads, after which the tee hands it nothing more.
    async fn write(&self, b: &[u8]) -> bool;
    /// The pack ended.
    fn close(&self);
    /// The pack did not end: what validate read is not the pack.
    fn abort(&self);
}

/// A one-chunk pipe from the streamed tee to validate's body: `write` waits until the reader took the chunk before,
/// so the pipe holds one chunk. `stop`, or dropping the reader, ends it for the writer, so a validate that answered
/// before it read the whole body no longer holds the tee up.
#[derive(Clone, Default)]
pub struct Pipe(std::rc::Rc<std::cell::RefCell<PipeState>>);

#[derive(Default)]
struct PipeState {
    chunk: Option<Vec<u8>>,
    /// The writer is done: `Ok` once the pack ended, `Err` when it did not.
    end: Option<Result<(), String>>,
    stopped: bool,
    reader: Option<std::task::Waker>,
    writer: Option<std::task::Waker>,
}

impl Pipe {
    /// The reading end.
    pub fn reader(&self) -> PipeReader {
        PipeReader(self.0.clone())
    }

    /// Validate no longer reads: the writer's `write` answers false from now on.
    pub fn stop(&self) {
        let mut s = self.0.borrow_mut();
        s.stopped = true;
        s.chunk = None;
        if let Some(w) = s.writer.take() {
            w.wake();
        }
    }

    fn end(&self, r: Result<(), String>) {
        let mut s = self.0.borrow_mut();
        s.end.get_or_insert(r);
        if let Some(w) = s.reader.take() {
            w.wake();
        }
    }
}

impl Feed for Pipe {
    async fn write(&self, b: &[u8]) -> bool {
        let mut b = Some(b.to_vec());
        futures_util::future::poll_fn(|cx| {
            let mut s = self.0.borrow_mut();
            if s.stopped {
                return std::task::Poll::Ready(false);
            }
            if s.chunk.is_some() {
                s.writer = Some(cx.waker().clone());
                return std::task::Poll::Pending;
            }
            s.chunk = b.take();
            if let Some(w) = s.reader.take() {
                w.wake();
            }
            std::task::Poll::Ready(true)
        })
        .await
    }

    fn close(&self) {
        self.end(Ok(()));
    }

    fn abort(&self) {
        self.end(Err("the push body broke off".into()));
    }
}

/// `Pipe`'s reading end: the chunks in order, then an `Err` if the pack broke off.
pub struct PipeReader(std::rc::Rc<std::cell::RefCell<PipeState>>);

impl futures_util::Stream for PipeReader {
    type Item = Result<Vec<u8>, String>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let mut s = self.0.borrow_mut();
        if let Some(c) = s.chunk.take() {
            if let Some(w) = s.writer.take() {
                w.wake();
            }
            return std::task::Poll::Ready(Some(Ok(c)));
        }
        match s.end.take() {
            Some(Ok(())) => std::task::Poll::Ready(None),
            Some(Err(e)) => std::task::Poll::Ready(Some(Err(e))),
            None => {
                s.reader = Some(cx.waker().clone());
                std::task::Poll::Pending
            }
        }
    }
}

impl Drop for PipeReader {
    fn drop(&mut self) {
        Pipe(self.0.clone()).stop();
    }
}

/// What the streamed tee's reader hands its puts: a full part with more of the pack behind it, the pack's last part,
/// or a pack that fit one part, put whole with its SHA-256 as `place_teed` puts one.
enum Put<P> {
    Part(u16, P),
    Last(u16, P, Digest),
    Whole(P, Digest),
}

/// One bucket's copy of a streamed pack.
enum Upload {
    NotStarted,
    Open {
        id: String,
        etags: Vec<(u16, String)>,
    },
    /// The key existed at the start; its SHA-256, when R2 has one, decides at the end whether it counts.
    Existed(Option<Digest>),
    Done(PutOutcome),
    /// A put failed, so the bucket takes no more of this pack. `open` is its upload, aborted once the pack ends. The
    /// third-bucket filler copies the pack there once its marker exists.
    Out {
        why: String,
        open: Option<String>,
    },
}

impl Upload {
    fn outcome(&self) -> PutOutcome {
        match self {
            Upload::Done(o) => o.clone(),
            Upload::Out { why, .. } => PutOutcome::Failed(why.clone()),
            _ => PutOutcome::Failed("the pack ended before its last part".into()),
        }
    }

    fn live(&self) -> bool {
        matches!(self, Upload::NotStarted | Upload::Open { .. } | Upload::Existed(_))
    }

    fn out(why: String) -> Upload {
        Upload::Out { why, open: None }
    }
}

/// `tx.send(m)` without the `Sink` traits: false once the receiver is gone.
async fn send<T>(tx: &mut futures_channel::mpsc::Sender<T>, m: T) -> bool {
    futures_util::future::poll_fn(|cx| tx.poll_ready(cx)).await.is_ok() && tx.start_send(m).is_ok()
}

type Ups = std::cell::RefCell<Vec<Upload>>;

impl<B: crate::bucket::Multipart> PackStore<'_, B> {
    /// The edge's tee for a pack that streams in (`design/DESIGN.md`, journey 1, step 2). Each chunk goes to `feed`
    /// (validate's body) and into a part, and each full part to the buckets' multipart uploads while the next fills.
    /// A part is filled again only once its puts ended, so the tee holds at most two parts whatever the pack's size,
    /// and each part is one copy that every bucket's put reads. A part is done once every bucket still taking the pack
    /// put it, so the slowest of them paces the push; one whose put failed takes no more of the pack, and the tee
    /// stops putting once those left cannot make the two-of-three rule. A pack that fits one part is put whole, with
    /// its SHA-256, and returns once two buckets in two regions hold it, as `place_teed` does. As with `place_teed`, a `DurablePack` comes back only once `validate` passed on
    /// bytes whose SHA-256 and length are the ones the tee read and put. The tee reads `pack` to its end whatever the
    /// puts and validate answered; the stream's first error ends it at once as the outer `Err`, with validate dropped
    /// and the uploads it opened aborted.
    pub async fn place_streamed<S, X, Fd, F, E>(
        &self,
        s: Streamed,
        pack: S,
        feed: &Fd,
        validate: F,
    ) -> Result<Result<(DurablePack, PackIndex), TeeError<E>>, X>
    where
        S: futures_util::Stream<Item = Result<Vec<u8>, X>>,
        Fd: Feed,
        F: std::future::Future<Output = Result<PackIndex, E>>,
    {
        use crate::bucket::PartBuf;
        use futures_channel::mpsc;
        use futures_util::future::{Either, join, select};
        let mut regions = Vec::new();
        for (site, _) in self.buckets {
            match self.sites.get(*site) {
                Some(x) => regions.push(x.region),
                None => return Ok(Err(PlaceError::UnknownSite(*site).into())),
            }
        }
        let regions = &regions;
        let key = &R2Key::Pack { network: s.network, id: s.id }.to_string();
        let journal = &R2Key::Upload { network: s.network, id: s.id }.to_string();
        let meta = &s.meta.to_pairs().to_vec();
        let ups: &Ups = &std::cell::RefCell::new(self.buckets.iter().map(|_| Upload::NotStarted).collect());
        let (mut to_put, mut puts) = mpsc::channel::<Put<B::Part>>(0);
        let (back, mut free) = mpsc::unbounded::<B::Part>();
        let read = async move {
            let mut pack = std::pin::pin!(pack);
            let mut sha = Sha256::new();
            let mut n = 0u64;
            let (mut feeding, mut putting) = (true, true);
            let (mut made, mut parts) = (0, 0u16);
            let mut cur: Option<B::Part> = None;
            while let Some(c) = pack.next().await {
                let c = match c {
                    Ok(c) => c,
                    Err(e) => {
                        feed.abort();
                        return Err(e);
                    }
                };
                sha.update(&c);
                n += c.len() as u64;
                if feeding {
                    feeding = feed.write(&c).await;
                }
                let mut rest = &c[..];
                while putting && !rest.is_empty() {
                    if cur.as_ref().is_some_and(|p| p.filled() == s.part) {
                        parts += 1;
                        let full = cur.take().expect("a full part");
                        if !send(&mut to_put, Put::Part(parts, full)).await {
                            putting = false;
                            break;
                        }
                    }
                    // Two parts in all: the next fills one while the buckets read the other.
                    let p = match cur {
                        Some(ref mut p) => p,
                        None if made < 2 => {
                            made += 1;
                            cur.insert(B::Part::with_capacity(s.part))
                        }
                        None => match free.next().await {
                            Some(mut p) => {
                                p.clear();
                                cur.insert(p)
                            }
                            None => {
                                putting = false;
                                break;
                            }
                        },
                    };
                    let take = (s.part - p.filled()).min(rest.len());
                    p.push(&rest[..take]);
                    rest = &rest[take..];
                }
            }
            feed.close();
            let checksum = Digest(sha.finalize().into());
            if let (true, Some(last)) = (putting, cur) {
                let m = if parts == 0 { Put::Whole(last, checksum) } else { Put::Last(parts + 1, last, checksum) };
                send(&mut to_put, m).await;
            }
            Ok((checksum, n))
        };
        let put = async move {
            let mut ended = false;
            while let Some(m) = puts.next().await {
                match m {
                    Put::Part(no, p) => {
                        self.put_part(ups, key, journal, meta, no, &p).await;
                        let _ = back.unbounded_send(p);
                    }
                    Put::Last(no, p, sum) => {
                        self.put_part(ups, key, journal, meta, no, &p).await;
                        self.complete(ups, key, journal, sum).await;
                        ended = true;
                    }
                    Put::Whole(p, sum) => {
                        self.put_whole(ups, regions, key, meta, &p, sum).await;
                        ended = true;
                    }
                }
                // Once the buckets still taking the pack cannot make the rule, the reader stops putting.
                if !ended && self.held(ups, regions, |u| u.live()).is_err() {
                    break;
                }
            }
            drop(puts);
            self.abort_open(ups, key).await;
            if !ended {
                return self.held(ups, regions, |u| u.live()).and(Err(PlaceError::TooFew { held: 0, regions: 0 }));
            }
            self.held(ups, regions, |u| u.outcome().holds())
        };
        let tee = std::pin::pin!(join(read, put));
        let validate = std::pin::pin!(validate);
        // `put` aborts every upload left open once the reader ends, so a body that broke off leaves none.
        let ((read, sites), ix) = match select(tee, validate).await {
            Either::Left(((Err(e), _), _)) => return Err(e),
            Either::Left((tee, validate)) => (tee, validate.await),
            Either::Right((ix, tee)) => (tee.await, ix),
        };
        let (checksum, n) = read?;
        let seal = || {
            let ix = ix.map_err(TeeError::Invalid)?;
            if ix.checksum != checksum || ix.bytes != n {
                return Err(PlaceError::NotValidated.into());
            }
            let sites = sites?;
            Ok((DurablePack::seal(PackRef::Stored { id: s.id, checksum, bytes: n, sites }), ix))
        };
        Ok(seal())
    }

    /// A promotion's pack as the site reads it from the shard's `/promote` (`design/DESIGN.md`, Shape, `Promoted`): it
    /// streams to the buckets in parts and, through a `Pipe`, to `validate` on the same shard, so the site holds two
    /// parts whatever the pack's size and `PROMOTE_MAX` bounds only how much one promotion batches. Once both
    /// answered, the `.idx` that validate wrote (`idx` of the pack's SHA-256; `None` stores the pack alone) goes
    /// beside it in each bucket that holds it, with `range` as its `from` and `to`. The outer `Err` is the pack's
    /// stream breaking off, the shard's failure.
    pub async fn place_promoted<S, X, V, F, E, I, G>(
        &self,
        s: Streamed,
        pack: S,
        validate: V,
        idx: I,
        range: Option<(Index, Index)>,
    ) -> Result<Result<PackRef, TeeError<E>>, X>
    where
        S: futures_util::Stream<Item = Result<Vec<u8>, X>>,
        V: FnOnce(PipeReader) -> F,
        F: std::future::Future<Output = Result<PackIndex, E>>,
        I: FnOnce(Digest) -> G,
        G: std::future::Future<Output = Option<Vec<u8>>>,
    {
        let (network, id, meta) = (s.network, s.id, s.meta);
        let pipe = Pipe::default();
        let reader = pipe.reader();
        let fed = &pipe;
        let validate = async move {
            let r = validate(reader).await;
            // The shard answered: whatever it did not read, the tee hands it no more.
            fed.stop();
            r
        };
        let pack = match self.place_streamed(s, pack, &pipe, validate).await? {
            Ok((d, _)) => d.into_pack(),
            Err(e) => return Ok(Err(e)),
        };
        if let PackRef::Stored { checksum, sites, .. } = &pack
            && let Some(b) = idx(*checksum).await
        {
            self.put_idx(network, id, meta, &b, range, *sites).await;
        }
        Ok(Ok(pack))
    }

    /// The two-of-three rule over the buckets whose upload `counts`.
    fn held(&self, ups: &Ups, regions: &[Region], counts: impl Fn(&Upload) -> bool) -> Result<SiteSet, PlaceError> {
        let ups = ups.borrow();
        let r: Vec<_> = (self.buckets.iter().zip(regions).zip(ups.iter()))
            .filter(|(_, u)| counts(u))
            .map(|(((site, _), r), _)| (*site, *r, PutOutcome::Created))
            .collect();
        durable_sites(&r, self.copies)
    }

    /// Aborts every upload a bucket left open when it dropped out, and any still open: the pack never ended.
    async fn abort_open(&self, ups: &Ups, key: &str) {
        let open: Vec<(usize, String)> = (ups.borrow_mut().iter_mut().enumerate())
            .filter_map(|(i, u)| match u {
                Upload::Out { open, .. } => open.take().map(|id| (i, id)),
                Upload::Open { id, .. } => Some((i, std::mem::take(id))),
                _ => None,
            })
            .collect();
        for (i, id) in open {
            let _ = self.buckets[i].1.abort_upload(key, &id).await;
        }
    }

    /// Part `no` to every bucket still taking the pack, at once, starting each one's upload with its first part. A
    /// bucket whose put fails takes no more of the pack.
    async fn put_part(&self, ups: &Ups, key: &str, journal: &str, meta: &[(String, String)], no: u16, p: &B::Part) {
        let each = (0..self.buckets.len()).map(|i| async move {
            let b = self.buckets[i].1;
            let id = match &ups.borrow()[i] {
                Upload::Open { id, .. } => Some(id.clone()),
                Upload::NotStarted => None,
                // A key that existed takes no parts; the end decides whether it holds.
                _ => return,
            };
            let id = match id {
                Some(id) => id,
                None => {
                    let started = b.start_upload(key, journal, meta).await;
                    let mut u = ups.borrow_mut();
                    match started {
                        Ok(Ok(id)) => {
                            u[i] = Upload::Open { id: id.clone(), etags: Vec::new() };
                            id
                        }
                        Ok(Err(PutNew::Exists(d))) => return u[i] = Upload::Existed(d),
                        Ok(Err(PutNew::Created)) => return u[i] = Upload::out("start_upload answered Created".into()),
                        Err(StoreError(e)) => return u[i] = Upload::out(e),
                    }
                }
            };
            match b.put_part(key, &id, no, p).await {
                Ok(etag) => {
                    if let Upload::Open { etags, .. } = &mut ups.borrow_mut()[i] {
                        etags.push((no, etag));
                    }
                }
                Err(StoreError(e)) => ups.borrow_mut()[i] = Upload::Out { why: e, open: Some(id) },
            }
        });
        futures_util::future::join_all(each).await;
    }

    async fn complete(&self, ups: &Ups, key: &str, journal: &str, sum: Digest) {
        let each = (0..self.buckets.len()).map(|i| async move {
            let b = self.buckets[i].1;
            let open = match &ups.borrow()[i] {
                Upload::Open { id, etags } => Some((id.clone(), etags.clone())),
                _ => None,
            };
            let o = match open {
                Some((id, etags)) => match b.complete_upload(key, &id, journal, &etags).await {
                    Ok(()) => PutOutcome::Created,
                    Err(StoreError(e)) => {
                        ups.borrow_mut()[i] = Upload::Out { why: e, open: Some(id) };
                        return;
                    }
                },
                None => match &ups.borrow()[i] {
                    Upload::Existed(Some(d)) if *d == sum => PutOutcome::ExistedSame,
                    Upload::Existed(_) => PutOutcome::ExistedOther,
                    _ => return,
                },
            };
            ups.borrow_mut()[i] = Upload::Done(o);
        });
        futures_util::future::join_all(each).await;
    }

    /// A pack that fit one part: a create-only put of it with its SHA-256 to every bucket at once, until two in two
    /// regions hold it, as `place_teed` puts a small pack.
    async fn put_whole(
        &self,
        ups: &Ups,
        regions: &[Region],
        key: &str,
        meta: &[(String, String)],
        p: &B::Part,
        sum: Digest,
    ) {
        let mut each: FuturesUnordered<_> = (0..self.buckets.len())
            .map(|i| async move {
                let o = match self.buckets[i].1.put_whole(key, p, sum, meta).await {
                    Ok(PutNew::Created) => PutOutcome::Created,
                    Ok(PutNew::Exists(Some(d))) if d == sum => PutOutcome::ExistedSame,
                    Ok(PutNew::Exists(_)) => PutOutcome::ExistedOther,
                    Err(StoreError(e)) => PutOutcome::Failed(e),
                };
                ups.borrow_mut()[i] = Upload::Done(o.clone());
                (i, o)
            })
            .collect();
        let mut results = Vec::new();
        while let Some((i, o)) = each.next().await {
            results.push((self.buckets[i].0, regions[i], o));
            if durable_sites(&results, self.copies).is_ok() {
                break;
            }
        }
    }
}

/// The marker's custom metadata, so a listing carries every marker's body.
pub fn marker_meta(m: &Marker) -> Vec<(String, String)> {
    vec![
        ("chain".into(), m.chain.to_string()),
        ("index".into(), m.index.0.to_string()),
        ("checksum".into(), m.checksum.hex()),
        ("bytes".into(), m.bytes.to_string()),
    ]
}

pub fn marker_from_meta(meta: &[(String, String)]) -> Option<Marker> {
    let get = |k: &str| meta.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    Some(Marker {
        chain: get("chain")?.parse().ok()?,
        index: Index(get("index")?.parse().ok()?),
        checksum: Digest::parse_hex(get("checksum")?)?,
        bytes: get("bytes")?.parse().ok()?,
    })
}

/// Puts the create-only `Committed` marker for a stored pack in `bucket`. A marker already there is success: markers
/// for one pack always carry the same body, because one pack id is named by one committed entry.
pub async fn put_marker<B: Bucket>(bucket: &B, network: NetworkId, id: PackId, m: &Marker) -> Result<(), StoreError> {
    let key = R2Key::committed(R2Key::Pack { network, id }).expect("pack marker").to_string();
    let body = serde_json::to_vec(m).expect("marker encodes");
    let sum = Digest(Sha256::digest(&body).into());
    bucket.put_new(&key, &body, sum, &marker_meta(m)).await.map(|_| ())
}

/// After `Acked`, the leader marks each stored pack its entry names, in every bucket the pack's `sites` names. The
/// caller passes the `Acked` index: this crate does not depend on the Raft core, so it cannot take `&Acked` itself.
/// Returns how many marker puts failed; the sweeper repairs them from the `request` table.
pub async fn mark_committed<B: Bucket>(
    buckets: &[(SiteId, &B)],
    network: NetworkId,
    pack: &PackRef,
    chain: ChainId,
    acked_index: Index,
) -> usize {
    let PackRef::Stored { id, checksum, bytes, sites } = pack else { return 0 };
    let m = Marker { chain, index: acked_index, checksum: *checksum, bytes: *bytes };
    let mut failed = 0;
    for (_, bucket) in buckets.iter().filter(|(s, _)| sites.contains(*s)) {
        if put_marker(*bucket, network, *id, &m).await.is_err() {
            failed += 1;
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use futures_executor::block_on;
    use futures_util::FutureExt;
    use graftyard_types::{DbId, RepoId, RequestId, Site, SiteLink, Timestamp, Ulid};

    use super::*;
    use crate::mem::MemBucket;

    fn site(id: u8, region: Region) -> Site {
        Site {
            id: SiteId(id),
            script: format!("s{id}"),
            account: "x".into(),
            region,
            jurisdiction: None,
            bucket: format!("b{id}"),
            release: None,
            link: SiteLink::Binding { member_class: "Member".into() },
            spare: false,
        }
    }

    fn tri() -> SiteTable {
        SiteTable(vec![site(0, Region::Enam), site(1, Region::Weur), site(2, Region::Apac), site(3, Region::Enam)])
    }

    fn index_of(bytes: &[u8]) -> PackIndex {
        PackIndex {
            checksum: Digest(Sha256::digest(bytes).into()),
            bytes: bytes.len() as u64,
            objects: 1,
            trailer: "t".into(),
        }
    }

    const NET: NetworkId = NetworkId(DbId(9));

    fn meta() -> ObjectMeta {
        ObjectMeta { chain: ChainId::Repo(RepoId(DbId(9))), request: RequestId(Ulid(3)) }
    }

    fn place<'a>(bytes: &'a [u8], index: &'a PackIndex, id: u128) -> Place<'a> {
        Place {
            network: NET,
            id: PackId(Ulid::new(Timestamp(1_000), id.to_be_bytes()[6..].try_into().unwrap())),
            meta: meta(),
            bytes,
            index,
            idx: Some(b"idx"),
            idx_range: None,
            inline_max: INLINE_MAX,
        }
    }

    #[test]
    fn rule_needs_two_regions_not_two_buckets() {
        use PutOutcome::*;
        let ok = |o: &[(u8, Region, PutOutcome)]| {
            durable_sites(
                &o.iter().map(|(s, r, x)| (SiteId(*s), *r, x.clone())).collect::<Vec<_>>(),
                Copies::TwoRegions,
            )
        };
        assert_eq!(ok(&[(0, Region::Enam, Created), (1, Region::Weur, Created)]), Ok(SiteSet(0b11)));
        // Two buckets in one region are one copy for the rule.
        assert!(
            ok(&[(0, Region::Enam, Created), (3, Region::Enam, Created), (2, Region::Apac, Failed("x".into()))])
                .is_err()
        );
        // A key that existed with other bytes is not a copy of this pack.
        assert!(ok(&[(0, Region::Enam, Created), (1, Region::Weur, ExistedOther)]).is_err());
        // The same bytes from an earlier attempt count.
        assert_eq!(ok(&[(0, Region::Enam, ExistedSame), (1, Region::Weur, Created)]), Ok(SiteSet(0b11)));
        assert!(ok(&[(0, Region::Enam, Created)]).is_err());
        assert_eq!(
            durable_sites(&[(SiteId(0), Region::Enam, Created)], Copies::Solo),
            Ok(SiteSet(0b1)),
            "Solo places one bucket"
        );
        assert!(durable_sites(&[(SiteId(0), Region::Enam, Failed("x".into()))], Copies::Solo).is_err());
    }

    #[test]
    fn place_inlines_small_packs_and_refuses_unvalidated_bytes() {
        let sites = tri();
        let (a, b, c) = (MemBucket::default(), MemBucket::default(), MemBucket::default());
        let buckets = [(SiteId(0), &a), (SiteId(1), &b), (SiteId(2), &c)];
        let store = PackStore { sites: &sites, buckets: &buckets, copies: Copies::TwoRegions };
        let small = vec![1u8; 100];
        let ix = index_of(&small);
        assert_eq!(block_on(store.place(place(&small, &ix, 1))).unwrap().pack(), &PackRef::Inline(small.clone()));
        assert!(a.keys().is_empty());
        // Bytes that differ from the validated ones by one byte are refused before any put.
        let mut other = vec![2u8; INLINE_MAX + 1];
        let ix = index_of(&other);
        other[7] ^= 1;
        assert_eq!(block_on(store.place(place(&other, &ix, 2))), Err(PlaceError::NotValidated));
        assert!(a.keys().is_empty() && b.keys().is_empty() && c.keys().is_empty());
    }

    #[test]
    fn place_survives_one_failed_bucket_and_fails_on_two() {
        let sites = tri();
        let big = vec![7u8; INLINE_MAX + 1];
        let ix = index_of(&big);
        let (a, b, c) = (MemBucket::default(), MemBucket::default(), MemBucket::default());
        b.fail_puts(true);
        let buckets = [(SiteId(0), &a), (SiteId(1), &b), (SiteId(2), &c)];
        let store = PackStore { sites: &sites, buckets: &buckets, copies: Copies::TwoRegions };
        let d = block_on(store.place(place(&big, &ix, 3))).unwrap();
        let PackRef::Stored { sites: held, bytes, .. } = d.pack() else { panic!() };
        assert_eq!((*held, *bytes), (SiteSet(0b101), big.len() as u64));
        assert_eq!(a.keys().len(), 2, "pack and idx");
        c.fail_puts(true);
        let r = block_on(store.place(place(&big, &ix, 4)));
        assert_eq!(r, Err(PlaceError::TooFew { held: 1, regions: 1 }));
        // The failed placement leaves its one copy behind for the sweeper.
        assert_eq!(a.keys().iter().filter(|k| k.ends_with(".pack")).count(), 2);
    }

    #[test]
    fn place_counts_a_retry_of_the_same_bytes_and_not_a_collision() {
        let sites = tri();
        let big = vec![5u8; INLINE_MAX + 10];
        let ix = index_of(&big);
        let (a, b, c) = (MemBucket::default(), MemBucket::default(), MemBucket::default());
        let buckets = [(SiteId(0), &a), (SiteId(1), &b), (SiteId(2), &c)];
        let store = PackStore { sites: &sites, buckets: &buckets, copies: Copies::TwoRegions };
        block_on(store.place(place(&big, &ix, 5))).unwrap();
        // The same placement again: every key exists with the same bytes.
        assert!(block_on(store.place(place(&big, &ix, 5))).is_ok());
        // Same id, other bytes: a bucket that holds the first bytes never counts for the second. The first placement
        // returned once two buckets held it, so at most the third bucket takes the new bytes, and two regions never do.
        let other = vec![6u8; INLINE_MAX + 10];
        let ix2 = index_of(&other);
        assert!(matches!(block_on(store.place(place(&other, &ix2, 5))), Err(PlaceError::TooFew { held: ..=1, .. })));
        let key = R2Key::Pack { network: NET, id: place(&big, &ix, 5).id }.to_string();
        assert_eq!(block_on(a.get(&key)).unwrap(), Some(big.clone()), "the first copy is untouched");
    }

    fn stage(bytes: &[u8], id: u128, inline: bool) -> Stage<'_> {
        Stage { network: NET, id: place(bytes, &index_of(bytes), id).id, meta: meta(), bytes, inline, part: PART_SIZE }
    }

    #[test]
    fn the_tee_returns_durable_only_when_validate_passed_on_the_same_bytes() {
        let sites = tri();
        let (a, b, c) = (MemBucket::default(), MemBucket::default(), MemBucket::default());
        let buckets = [(SiteId(0), &a), (SiteId(1), &b), (SiteId(2), &c)];
        let store = PackStore { sites: &sites, buckets: &buckets, copies: Copies::TwoRegions };
        let big = vec![3u8; INLINE_MAX + 5];
        let ok = |bytes: &[u8]| {
            let ix = index_of(bytes);
            async move { Ok::<_, String>(ix) }
        };
        let (d, _) = block_on(store.place_teed(stage(&big, 10, false), ok(&big))).unwrap();
        assert!(matches!(d.pack(), PackRef::Stored { sites, .. } if sites.len() >= 2));
        // validate refused: no DurablePack, and the copies the tee already put stay for the sweeper.
        let r = block_on(store.place_teed(stage(&big, 11, false), async { Err::<PackIndex, _>("connectivity") }));
        assert_eq!(r, Err(TeeError::Invalid("connectivity")));
        let key = R2Key::Pack { network: NET, id: stage(&big, 11, false).id }.to_string();
        assert!(block_on(a.head(&key)).unwrap().is_some(), "orphan left for the sweeper");
        // validate answered for other bytes.
        let other = vec![4u8; INLINE_MAX + 5];
        let r = block_on(store.place_teed(stage(&big, 12, false), ok(&other)));
        assert_eq!(r, Err(TeeError::Place(PlaceError::NotValidated)));
        // Two buckets down: validate passed, still not durable.
        b.fail_puts(true);
        c.fail_puts(true);
        let r = block_on(store.place_teed(stage(&big, 13, false), ok(&big)));
        assert!(matches!(r, Err(TeeError::Place(PlaceError::TooFew { held: 1, .. }))), "{r:?}");
        // Inline skips the buckets, but not validate, and never holds more than INLINE_MAX.
        let small = vec![5u8; 100];
        let before = a.keys().len();
        let (d, _) = block_on(store.place_teed(stage(&small, 14, true), ok(&small))).unwrap();
        assert_eq!(d.pack(), &PackRef::Inline(small.clone()));
        assert_eq!(a.keys().len(), before);
        let r = block_on(store.place_teed(stage(&small, 15, true), async { Err::<PackIndex, _>("bad") }));
        assert_eq!(r, Err(TeeError::Invalid("bad")));
        assert!(block_on(store.place_teed(stage(&big, 16, true), ok(&big))).is_err(), "inline over INLINE_MAX");
    }

    #[test]
    fn markers_go_to_the_packs_sites_and_are_idempotent() {
        let (a, b, c) = (MemBucket::default(), MemBucket::default(), MemBucket::default());
        let buckets = [(SiteId(0), &a), (SiteId(1), &b), (SiteId(2), &c)];
        let pack = PackRef::Stored { id: PackId(Ulid(77)), checksum: Digest([1; 32]), bytes: 9, sites: SiteSet(0b011) };
        let chain = ChainId::Repo(RepoId(DbId(9)));
        assert_eq!(block_on(mark_committed(&buckets, NET, &pack, chain, Index(12))), 0);
        assert_eq!(block_on(mark_committed(&buckets, NET, &pack, chain, Index(12))), 0);
        assert_eq!((a.keys().len(), b.keys().len(), c.keys().len()), (1, 1, 0));
        let info = block_on(a.list(&R2Key::marker_prefix(NET))).unwrap();
        assert_eq!(
            marker_from_meta(&info[0].meta),
            Some(Marker { chain, index: Index(12), checksum: Digest([1; 32]), bytes: 9 })
        );
        assert_eq!(block_on(mark_committed(&buckets, NET, &PackRef::Inline(vec![1]), chain, Index(1))), 0);
    }

    type Teed = Result<(DurablePack, PackIndex), TeeError<&'static str>>;

    /// Streams `pack` in `chunk`-byte chunks through `place_streamed` with parts of `part` bytes, validate answering
    /// `ix` for the bytes it read from the pipe. Answers the tee's result, the bytes validate read, and the most parts
    /// alive at once.
    fn stream(
        store: &PackStore<'_, MemBucket>,
        pack: &[u8],
        chunk: usize,
        part: usize,
        id: u128,
        ix: impl FnOnce(&[u8]) -> Result<PackIndex, &'static str>,
    ) -> (Teed, Vec<u8>, usize) {
        use crate::mem::MemPart;
        let s = Streamed { network: NET, id: stage(pack, id, false).id, meta: meta(), part };
        let chunks: Vec<Result<Vec<u8>, ()>> = pack.chunks(chunk).map(|c| Ok(c.to_vec())).collect();
        let pipe = Pipe::default();
        let mut reader = pipe.reader();
        let got = std::cell::RefCell::new(Vec::new());
        let validate = async {
            while let Some(c) = reader.next().await {
                got.borrow_mut().extend_from_slice(&c.map_err(|_| "broke")?);
            }
            ix(&got.borrow())
        };
        MemPart::reset_peak();
        let r = block_on(store.place_streamed(s, futures_util::stream::iter(chunks), &pipe, validate));
        (r.expect("the stream did not fail"), got.into_inner(), MemPart::reset_peak())
    }

    /// A pack of many parts streams through two part buffers, reaches validate whole, and lands in every bucket as
    /// the same bytes; the `DurablePack` names its SHA-256 and length. Chunks that straddle part boundaries
    /// (7 bytes against 64-byte parts) still fill each part exactly.
    #[test]
    fn a_streamed_pack_holds_two_parts_whatever_its_size() {
        let sites = tri();
        let (a, b, c) = (MemBucket::default(), MemBucket::default(), MemBucket::default());
        let buckets = [(SiteId(0), &a), (SiteId(1), &b), (SiteId(2), &c)];
        let store = PackStore { sites: &sites, buckets: &buckets, copies: Copies::TwoRegions };
        for parts in [3usize, 40] {
            let pack: Vec<u8> = (0..parts * 64 - 5).map(|i| (i * 31 % 251) as u8).collect();
            let (r, got, peak) = stream(&store, &pack, 7, 64, parts as u128, |b| Ok(index_of(b)));
            let (d, _) = r.unwrap();
            assert_eq!(got, pack, "validate reads the whole pack");
            assert!(peak <= 2, "{parts} parts held {peak} buffers at once");
            let PackRef::Stored { checksum, bytes, sites: held, id } = d.pack() else { panic!("stored") };
            assert_eq!((*checksum, *bytes), (index_of(&pack).checksum, pack.len() as u64));
            assert!(held.len() >= 2);
            let key = R2Key::Pack { network: NET, id: *id }.to_string();
            for x in held.iter() {
                assert_eq!(block_on(buckets[x.0 as usize].1.get(&key)).unwrap().as_deref(), Some(&pack[..]));
            }
            let journal = R2Key::Upload { network: NET, id: *id }.to_string();
            assert!(a.head(&journal).now_or_never().unwrap().unwrap().is_none(), "journal deleted on complete");
        }
        // One part or less: put whole, with its SHA-256, as `place_teed` puts one.
        let small = vec![9u8; 50];
        let (r, _, _) = stream(&store, &small, 7, 64, 99, |b| Ok(index_of(b)));
        let PackRef::Stored { id, .. } = r.unwrap().0.pack().clone() else { panic!("stored") };
        let key = R2Key::Pack { network: NET, id }.to_string();
        assert_eq!(block_on(a.head(&key)).unwrap().unwrap().sha256, Some(index_of(&small).checksum));
    }

    /// The SHA-256 and length gate holds on the streamed path: validate's index for other bytes, or for the same bytes
    /// one short, is no `DurablePack`; a refusal is `Invalid`; and two buckets failing mid-pack is `TooFew`, with
    /// every upload aborted.
    #[test]
    fn a_streamed_pack_is_durable_only_on_the_bytes_validate_indexed() {
        let sites = tri();
        let (a, b, c) = (MemBucket::default(), MemBucket::default(), MemBucket::default());
        let buckets = [(SiteId(0), &a), (SiteId(1), &b), (SiteId(2), &c)];
        let store = PackStore { sites: &sites, buckets: &buckets, copies: Copies::TwoRegions };
        let pack: Vec<u8> = (0..300u32).map(|i| (i % 7) as u8).collect();
        let other = |b: &[u8]| {
            let mut o = b.to_vec();
            o[150] ^= 1;
            Ok(index_of(&o))
        };
        assert_eq!(stream(&store, &pack, 13, 64, 1, other).0.unwrap_err(), TeeError::Place(PlaceError::NotValidated));
        let short = |b: &[u8]| Ok(PackIndex { bytes: b.len() as u64 - 1, ..index_of(b) });
        assert_eq!(stream(&store, &pack, 13, 64, 2, short).0.unwrap_err(), TeeError::Place(PlaceError::NotValidated));
        assert_eq!(
            stream(&store, &pack, 13, 64, 3, |_| Err("connectivity")).0.unwrap_err(),
            TeeError::Invalid("connectivity")
        );
        // One bucket down from part 2: the other two hold it.
        b.fail_parts_from(2);
        let (r, _, _) = stream(&store, &pack, 13, 64, 4, |b| Ok(index_of(b)));
        let PackRef::Stored { sites: held, .. } = r.unwrap().0.pack().clone() else { panic!("stored") };
        assert_eq!(held, SiteSet(0b101));
        assert!(b.open_uploads().is_empty(), "the failed bucket's upload is aborted");
        c.fail_parts_from(3);
        let (r, got, _) = stream(&store, &pack, 13, 64, 5, |b| Ok(index_of(b)));
        assert!(matches!(r, Err(TeeError::Place(PlaceError::TooFew { .. }))), "{:?}", r.err());
        assert_eq!(got, pack, "validate still reads the whole pack");
        assert!(a.open_uploads().is_empty() && b.open_uploads().is_empty() && c.open_uploads().is_empty());
    }

    /// A promotion of 112 MiB (seven `PART_SIZE` parts, over twice `PROMOTE_MAX`) streams from the shard through two
    /// part buffers: validate reads it whole from the pipe, two buckets hold the same bytes, and the `.idx` validate
    /// wrote goes beside it with the range as `from` and `to`. A validate that refuses places no `.idx`, and a shard
    /// with no `.idx` stores the pack alone.
    #[test]
    fn a_promotion_streams_through_two_parts_and_stores_its_idx() {
        let sites = tri();
        let (a, b, c) = (MemBucket::default(), MemBucket::default(), MemBucket::default());
        let buckets = [(SiteId(0), &a), (SiteId(1), &b), (SiteId(2), &c)];
        let store = PackStore { sites: &sites, buckets: &buckets, copies: Copies::TwoRegions };
        const CHUNK: usize = 1 << 20;
        let base: Vec<u8> = (0..CHUNK).map(|j| (j * 13 % 251) as u8).collect();
        // Each chunk differs in its first bytes, so a part put out of order or twice changes the object.
        let chunk = |i: usize| -> Vec<u8> {
            let mut c = base.clone();
            c[..8].copy_from_slice(&(i as u64).to_be_bytes());
            c
        };
        let run = |id: u128, chunks: usize, refuse: bool, idx: Option<&'static [u8]>| {
            let s = Streamed { network: NET, id: stage(b"x", id, false).id, meta: meta(), part: PART_SIZE };
            let pack = futures_util::stream::iter((0..chunks).map(|i| Ok::<_, ()>(chunk(i))));
            let read = std::cell::Cell::new(None);
            let validate = |mut r: PipeReader| {
                let read = &read;
                async move {
                    let mut sha = Sha256::new();
                    let mut n = 0u64;
                    while let Some(c) = r.next().await {
                        let c = c.map_err(|_| "broke")?;
                        n += c.len() as u64;
                        sha.update(&c);
                    }
                    let checksum = Digest(sha.finalize().into());
                    read.set(Some((checksum, n)));
                    if refuse {
                        return Err("index-pack refused");
                    }
                    Ok(PackIndex { checksum, bytes: n, objects: 1, trailer: "t".into() })
                }
            };
            let asked = std::cell::Cell::new(None);
            let get_idx = |sum: Digest| {
                asked.set(Some(sum));
                async move { idx.map(<[u8]>::to_vec) }
            };
            crate::mem::MemPart::reset_peak();
            let r = block_on(store.place_promoted(s, pack, validate, get_idx, Some((Index(17), Index(17)))));
            (r.expect("the stream did not fail"), read.get(), crate::mem::MemPart::reset_peak(), asked.get())
        };
        let chunks = 112;
        assert!((chunks * CHUNK) as u64 > 2 * crate::log_archive::PROMOTE_MAX);
        let (r, read, peak, asked) = run(41, chunks, false, Some(b"the idx"));
        let PackRef::Stored { id, checksum, bytes, sites: held } = r.unwrap() else { panic!("stored") };
        let (want, n) = read.expect("validate read the pack");
        assert_eq!((checksum, bytes, n), (want, (chunks * CHUNK) as u64, (chunks * CHUNK) as u64));
        assert!(peak <= 2, "a 112 MiB promotion held {peak} parts at once");
        assert_eq!(asked, Some(want), "the .idx is asked for under the pack's SHA-256");
        assert!(held.len() >= 2);
        let (key, idx_key) =
            (R2Key::Pack { network: NET, id }.to_string(), R2Key::Idx { network: NET, id }.to_string());
        for x in held.iter() {
            let bucket = buckets[x.0 as usize].1;
            let got = block_on(bucket.get(&key)).unwrap().unwrap();
            assert_eq!(got.len(), chunks * CHUNK);
            assert!(got.chunks(CHUNK).enumerate().all(|(i, c)| c == chunk(i)), "the object is the pack, in order");
            let info = block_on(bucket.head(&idx_key)).unwrap().expect("the .idx beside the pack");
            assert_eq!(block_on(bucket.get(&idx_key)).unwrap().as_deref(), Some(&b"the idx"[..]));
            assert_eq!((info.meta("from"), info.meta("to")), (Some("17"), Some("17")));
            assert_eq!(info.meta("chain"), Some(meta().to_pairs()[0].1.as_str()));
        }
        // Refused: no pack and no `.idx` is asked for.
        let (r, _, _, asked) = run(42, 3, true, Some(b"the idx"));
        assert_eq!(r.unwrap_err(), TeeError::Invalid("index-pack refused"));
        assert_eq!(asked, None);
        // A shard with no `.idx`: the pack alone.
        let (r, _, _, _) = run(43, 3, false, None);
        let PackRef::Stored { id, .. } = r.unwrap() else { panic!("stored") };
        assert!(block_on(a.head(&R2Key::Idx { network: NET, id }.to_string())).unwrap().is_none());
    }

    /// A body that breaks off is the outer `Err`: no `DurablePack`, validate's pipe ends in an error, and the open
    /// uploads are aborted.
    #[test]
    fn a_streamed_pack_that_breaks_off_places_nothing() {
        let sites = tri();
        let (a, b, c) = (MemBucket::default(), MemBucket::default(), MemBucket::default());
        let buckets = [(SiteId(0), &a), (SiteId(1), &b), (SiteId(2), &c)];
        let store = PackStore { sites: &sites, buckets: &buckets, copies: Copies::TwoRegions };
        let s = Streamed { network: NET, id: stage(b"x", 7, false).id, meta: meta(), part: 64 };
        let chunks = vec![Ok(vec![1u8; 100]), Ok(vec![2u8; 100]), Err("reset")];
        let pipe = Pipe::default();
        let mut reader = pipe.reader();
        let validate = async {
            while let Some(c) = reader.next().await {
                c.map_err(|_| "broke")?;
            }
            Err::<PackIndex, _>("ended")
        };
        let r = block_on(store.place_streamed(s, futures_util::stream::iter(chunks), &pipe, validate));
        assert_eq!(r.err(), Some("reset"));
        assert!(a.open_uploads().is_empty() && b.open_uploads().is_empty() && c.open_uploads().is_empty());
    }
}
