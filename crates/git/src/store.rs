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
            let idx_key = R2Key::Idx { network: p.network, id: p.id }.to_string();
            let idx_sum = Digest(Sha256::digest(idx).into());
            for (site, bucket) in self.buckets.iter().filter(|(s, _)| sites.contains(*s)) {
                let _ = (site, bucket.put_new(&idx_key, idx, idx_sum, &meta).await);
            }
        }
        Ok(DurablePack::seal(PackRef::Stored { id: p.id, checksum, bytes: p.bytes.len() as u64, sites }))
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
}
