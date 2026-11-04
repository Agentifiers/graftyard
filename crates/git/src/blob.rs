//! The blob store for LFS objects and release assets (`design/DESIGN.md`, Shape, "Packs"; `design/SHARED.md`, "Issues
//! and pull requests", `BlobRef` and `DurableBlob`), and the derived `ArchiveCache` objects (`codeload-archives`).
//!
//! A blob is placed like a stored pack: create-only puts of the same bytes to every site bucket of the repository's
//! placement, under `R2Key::Blob { repo, oid }` where `oid` is the bytes' SHA-256, and it is durable once two sites in
//! two regions hold it (`store::durable_sites`). `BlobStore::place` is the only constructor of `DurableBlob`, and the
//! `LfsObject` and `ReleaseAsset` events take a `BlobRef` that only a `DurableBlob` hands out, so `verify` and the 201
//! of an upload, which render from the committed entry, cannot come before two buckets hold the bytes (BF-25).
//!
//! A read tries the buckets the `BlobRef` names, then the others, and checks each copy's SHA-256, so one unreadable or
//! corrupt bucket never fails a download.

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use graftyard_types::{ChainId, Digest, Index, NetworkId, RepoId, SiteId, SiteSet, SiteTable};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::bucket::{Bucket, PutNew, StoreError};
use crate::key::R2Key;
use crate::pack::{Marker, ObjectMeta};
use crate::store::{Copies, PlaceError, PutOutcome, durable_sites, marker_from_meta, marker_meta};

/// SHARED's `BlobRef`: SHA-256 of the bytes, their size, and the sites whose bucket held a verified copy when the blob
/// became durable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlobRef {
    pub oid: Digest,
    pub bytes: u64,
    pub sites: SiteSet,
}

/// SHARED's `DurableBlob`: built only by `BlobStore::place`, once the bytes are in the buckets of two sites in two
/// regions (one for `Solo`), each put create-only with its SHA-256.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DurableBlob {
    blob: BlobRef,
    _sealed: (),
}

impl DurableBlob {
    pub fn blob(&self) -> BlobRef {
        self.blob
    }
}

/// The largest blob the edge accepts in one request body [guess: the Workers request body limit on the paid plan is
/// 100 MB; release assets and LFS objects above it need a multipart upload this package does not build].
pub const BLOB_MAX: u64 = 100 << 20;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BlobError {
    /// The bytes' SHA-256 or size is not what the client declared: LFS's `oid` and `size`.
    #[error("the bytes are not the declared object")]
    Mismatch,
    #[error("over the {BLOB_MAX}-byte blob limit")]
    TooLarge,
    #[error(transparent)]
    Place(#[from] PlaceError),
}

pub struct BlobStore<'a, B> {
    pub sites: &'a SiteTable,
    /// Every bucket of the repository's placement, home site first.
    pub buckets: &'a [(SiteId, &'a B)],
    pub copies: Copies,
}

/// A read's answer: the bytes, and from which site they came.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub bytes: Vec<u8>,
    pub site: SiteId,
}

impl<B: Bucket> BlobStore<'_, B> {
    /// Places `bytes` as `R2Key::Blob { repo, oid }`. `declared` is the client's (`oid`, `size`) when it named them, as
    /// LFS does; an upload names none and the oid is computed. The puts run at once and the call returns as soon as the
    /// two-of-three rule holds; a third put still running finishes or fails on its own, and the third-bucket filler
    /// copies the blob once its marker exists.
    pub async fn place(
        &self,
        repo: RepoId,
        bytes: &[u8],
        declared: Option<(Digest, u64)>,
        meta: ObjectMeta,
    ) -> Result<DurableBlob, BlobError> {
        if bytes.len() as u64 > BLOB_MAX {
            return Err(BlobError::TooLarge);
        }
        let oid = Digest(Sha256::digest(bytes).into());
        if declared.is_some_and(|(d, n)| d != oid || n != bytes.len() as u64) {
            return Err(BlobError::Mismatch);
        }
        let key = R2Key::Blob { repo, oid }.to_string();
        let meta = meta.to_pairs().to_vec();
        let mut puts = FuturesUnordered::new();
        for (site, bucket) in self.buckets {
            let region = self.sites.get(*site).ok_or(PlaceError::UnknownSite(*site))?.region;
            let (key, meta) = (&key, &meta);
            puts.push(async move {
                let outcome = match bucket.put_new(key, bytes, oid, meta).await {
                    Ok(PutNew::Created) => PutOutcome::Created,
                    // The key is the bytes' own hash, so an existing object with a matching checksum is this blob.
                    Ok(PutNew::Exists(Some(d))) if d == oid => PutOutcome::ExistedSame,
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
        Ok(DurableBlob { blob: BlobRef { oid, bytes: bytes.len() as u64, sites }, _sealed: () })
    }

    /// LFS `verify`: the blob the transfer placed earlier, as a `DurableBlob` again, once a HEAD of every bucket shows
    /// the key with SHA-256 `oid` and `bytes` in two sites in two regions. A bucket that errs, lacks the key, or holds
    /// other bytes does not count, so `verify` fails while only one bucket holds the object (BF-25).
    pub async fn confirm(&self, repo: RepoId, oid: Digest, bytes: u64) -> Result<DurableBlob, BlobError> {
        let key = R2Key::Blob { repo, oid }.to_string();
        let mut results = Vec::new();
        for (site, bucket) in self.buckets {
            let region = self.sites.get(*site).ok_or(PlaceError::UnknownSite(*site))?.region;
            let outcome = match bucket.head(&key).await {
                Ok(Some(o)) if o.sha256 == Some(oid) && o.size == bytes => PutOutcome::ExistedSame,
                Ok(Some(_)) => PutOutcome::ExistedOther,
                Ok(None) => PutOutcome::Failed("absent".into()),
                Err(StoreError(e)) => PutOutcome::Failed(e),
            };
            results.push((*site, region, outcome));
        }
        let sites = durable_sites(&results, self.copies)?;
        Ok(DurableBlob { blob: BlobRef { oid, bytes, sites }, _sealed: () })
    }

    /// The bytes of `blob`: the sites it names first, in table order, then any other bucket of the placement (the
    /// filler may have copied it there). A bucket that errs, lacks the key, or returns bytes whose SHA-256 is not the
    /// oid is skipped. `Ok(None)` when every bucket answered and none has a good copy; an error when some bucket could
    /// not answer and none had one, since that bucket might hold it.
    pub async fn read(&self, repo: RepoId, blob: &BlobRef) -> Result<Option<Fetched>, StoreError> {
        let key = R2Key::Blob { repo, oid: blob.oid }.to_string();
        let mut order: Vec<&(SiteId, &B)> = self.buckets.iter().filter(|(s, _)| blob.sites.contains(*s)).collect();
        order.extend(self.buckets.iter().filter(|(s, _)| !blob.sites.contains(*s)));
        let mut last = None;
        for (site, bucket) in order {
            match bucket.get(&key).await {
                Ok(Some(b)) if b.len() as u64 == blob.bytes && Digest(Sha256::digest(&b).into()) == blob.oid => {
                    return Ok(Some(Fetched { bytes: b, site: *site }));
                }
                Ok(_) => {}
                Err(e) => last = Some(e),
            }
        }
        last.map_or(Ok(None), Err)
    }
}

/// After `Acked`, the leader puts the blob's create-only `Committed` marker in each bucket its `sites` names, as for a
/// stored pack (`store::mark_committed`). Returns how many marker puts failed; the sweeper repairs them.
pub async fn mark_blob<B: Bucket>(
    buckets: &[(SiteId, &B)],
    repo: RepoId,
    blob: &BlobRef,
    chain: ChainId,
    acked_index: Index,
) -> usize {
    let key = R2Key::committed(R2Key::Blob { repo, oid: blob.oid }).expect("blob marker").to_string();
    let m = Marker { chain, index: acked_index, checksum: blob.oid, bytes: blob.bytes };
    let body = serde_json::to_vec(&m).expect("marker encodes");
    let sum = Digest(Sha256::digest(&body).into());
    let mut failed = 0;
    for (_, bucket) in buckets.iter().filter(|(s, _)| blob.sites.contains(*s)) {
        if bucket.put_new(&key, &body, sum, &marker_meta(&m)).await.is_err() {
            failed += 1;
        }
    }
    failed
}

/// The marker a bucket holds for a blob, if any.
pub async fn blob_marker<B: Bucket>(bucket: &B, repo: RepoId, oid: Digest) -> Result<Option<Marker>, StoreError> {
    let key = R2Key::committed(R2Key::Blob { repo, oid }).expect("blob marker").to_string();
    Ok(bucket.head(&key).await?.and_then(|o| marker_from_meta(&o.meta)))
}

// ---- archives ----

/// `codeload-archives` formats (`gh release download --archive=tar.gz|zip`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ArchiveFormat {
    TarGz,
    Zip,
}

impl ArchiveFormat {
    pub fn ext(self) -> &'static str {
        match self {
            ArchiveFormat::TarGz => "tar.gz",
            ArchiveFormat::Zip => "zip",
        }
    }

    pub fn parse(ext: &str) -> Option<ArchiveFormat> {
        match ext {
            "tar.gz" => Some(ArchiveFormat::TarGz),
            "zip" => Some(ArchiveFormat::Zip),
            _ => None,
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            ArchiveFormat::TarGz => "application/x-gzip",
            ArchiveFormat::Zip => "application/zip",
        }
    }
}

/// What builds an archive on a miss: `git archive` on the git cache (`design/SHARED.md`, `codeload-archives`).
/// `prefix` is the top directory every entry sits under, `{repo}-{ref}/`; the bytes depend only on the tree, the
/// commit's time, the prefix, and the format, which is what makes a rebuilt archive equal the deleted one.
#[allow(async_fn_in_trait)]
pub trait Archiver {
    async fn archive(
        &self,
        network: NetworkId,
        commit: &str,
        prefix: &str,
        format: ArchiveFormat,
    ) -> Result<Vec<u8>, String>;
}

/// One archive request, after the ref resolved to a commit.
pub struct ArchiveRequest<'a> {
    pub network: NetworkId,
    /// The commit's hex id.
    pub commit: &'a str,
    pub prefix: &'a str,
    pub format: ArchiveFormat,
}

/// Where the cached object lives. The design's key is `ArchiveCache { network, tree, format }`, but `git archive` of a
/// commit writes the commit's time into every entry and the prefix is part of the bytes, so two commits on one tree,
/// or one commit under two prefixes, are different archives. The key's `tree` segment holds the commit id and a hash
/// of the prefix instead [inference].
pub fn archive_key(r: &ArchiveRequest<'_>) -> R2Key {
    R2Key::ArchiveCache {
        network: r.network,
        tree: format!("{}-{}", r.commit, prefix_hash(r.prefix)),
        format: r.format,
    }
}

fn prefix_hash(prefix: &str) -> String {
    Sha256::digest(prefix.as_bytes())[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether the archive came from the cache or was rebuilt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Cache,
    Built,
}

/// The archive's bytes from the warm site's bucket, or, on a miss, from `git archive`, put back create-only. The object
/// is derived and named by no entry, so a failed put changes nothing but the next request's speed.
pub async fn archive<B: Bucket, A: Archiver>(
    warm: &B,
    git: &A,
    r: &ArchiveRequest<'_>,
) -> Result<(Vec<u8>, Source), String> {
    let key = archive_key(r).to_string();
    if let Ok(Some(b)) = warm.get(&key).await {
        return Ok((b, Source::Cache));
    }
    let bytes = git.archive(r.network, r.commit, r.prefix, r.format).await?;
    let sum = Digest(Sha256::digest(&bytes).into());
    let _ = warm.put_new(&key, &bytes, sum, &[]).await;
    Ok((bytes, Source::Built))
}

#[cfg(test)]
mod tests {
    use futures_executor::block_on;
    use graftyard_types::{DbId, Region, RequestId, Site, SiteLink, Ulid};

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

    const REPO: RepoId = RepoId(DbId(7));

    fn meta() -> ObjectMeta {
        ObjectMeta { chain: ChainId::Repo(REPO), request: RequestId(Ulid(5)) }
    }

    fn sha(b: &[u8]) -> Digest {
        Digest(Sha256::digest(b).into())
    }

    #[test]
    fn a_blob_is_durable_only_in_two_regions_and_never_on_one_bucket() {
        let sites = tri();
        let (a, b, c, d) = (MemBucket::default(), MemBucket::default(), MemBucket::default(), MemBucket::default());
        let store = |bk: &[(SiteId, &MemBucket)]| {
            let v: Vec<(SiteId, &MemBucket)> = bk.to_vec();
            block_on(BlobStore { sites: &sites, buckets: &v, copies: Copies::TwoRegions }.place(
                REPO,
                b"lfs",
                None,
                meta(),
            ))
        };
        let ok = store(&[(SiteId(0), &a), (SiteId(1), &b), (SiteId(2), &c)]).unwrap();
        assert!(ok.blob().sites.len() >= 2);
        assert_eq!(ok.blob().oid, sha(b"lfs"));
        b.fail_puts(true);
        c.fail_puts(true);
        let one = store(&[(SiteId(0), &a), (SiteId(1), &b), (SiteId(2), &c)]);
        assert_eq!(
            one,
            Err(BlobError::Place(PlaceError::TooFew { held: 1, regions: 1 })),
            "one bucket is never enough"
        );
        let same_region = store(&[(SiteId(0), &a), (SiteId(3), &d), (SiteId(2), &c)]);
        assert!(matches!(same_region, Err(BlobError::Place(PlaceError::TooFew { regions: 1, .. }))), "{same_region:?}");
    }

    #[test]
    fn confirm_needs_two_regions_holding_the_same_bytes() {
        let sites = tri();
        let (a, b, c) = (MemBucket::default(), MemBucket::default(), MemBucket::default());
        let bk = [(SiteId(0), &a), (SiteId(1), &b), (SiteId(2), &c)];
        let s = BlobStore { sites: &sites, buckets: &bk, copies: Copies::TwoRegions };
        let blob = block_on(s.place(REPO, b"obj", None, meta())).unwrap().blob();
        assert_eq!(block_on(s.confirm(REPO, blob.oid, 3)).unwrap().blob().oid, blob.oid);
        assert!(block_on(s.confirm(REPO, blob.oid, 4)).is_err(), "the size is checked");
        b.fail_gets(true);
        c.fail_gets(true);
        assert!(
            matches!(block_on(s.confirm(REPO, blob.oid, 3)), Err(BlobError::Place(PlaceError::TooFew { held: 1, .. }))),
            "one readable bucket is not durable"
        );
        let other = sha(b"never put");
        assert!(block_on(s.confirm(REPO, other, 9)).is_err());
    }

    #[test]
    fn the_declared_oid_and_size_must_be_the_bytes() {
        let sites = tri();
        let (a, b, c) = (MemBucket::default(), MemBucket::default(), MemBucket::default());
        let bk = [(SiteId(0), &a), (SiteId(1), &b), (SiteId(2), &c)];
        let s = BlobStore { sites: &sites, buckets: &bk, copies: Copies::TwoRegions };
        assert_eq!(block_on(s.place(REPO, b"x", Some((sha(b"y"), 1)), meta())), Err(BlobError::Mismatch));
        assert_eq!(block_on(s.place(REPO, b"x", Some((sha(b"x"), 2)), meta())), Err(BlobError::Mismatch));
        assert!(a.keys().is_empty(), "nothing is put for a mismatch");
        assert!(block_on(s.place(REPO, b"x", Some((sha(b"x"), 1)), meta())).is_ok());
        // A retry of the same bytes counts the copies already there.
        assert!(block_on(s.place(REPO, b"x", None, meta())).is_ok());
    }

    #[test]
    fn a_read_survives_one_unreadable_or_corrupt_bucket() {
        let sites = tri();
        let (a, b, c) = (MemBucket::default(), MemBucket::default(), MemBucket::default());
        let bk = [(SiteId(0), &a), (SiteId(1), &b), (SiteId(2), &c)];
        let s = BlobStore { sites: &sites, buckets: &bk, copies: Copies::TwoRegions };
        let blob = block_on(s.place(REPO, b"asset", None, meta())).unwrap().blob();
        let first = blob.sites.iter().next().unwrap();
        let bucket = |id: SiteId| bk.iter().find(|(s, _)| *s == id).unwrap().1;
        bucket(first).fail_gets(true);
        let got = block_on(s.read(REPO, &blob)).unwrap().unwrap();
        assert_eq!(got.bytes, b"asset");
        assert_ne!(got.site, first);
        bucket(first).fail_gets(false);
        let key = R2Key::Blob { repo: REPO, oid: blob.oid }.to_string();
        bucket(first).plant(&key, b"assez", graftyard_types::Timestamp(0), &[]);
        assert_eq!(block_on(s.read(REPO, &blob)).unwrap().unwrap().bytes, b"asset", "a corrupt copy is skipped");
    }

    #[test]
    fn markers_go_to_the_blobs_sites() {
        let (a, b, c) = (MemBucket::default(), MemBucket::default(), MemBucket::default());
        let bk = [(SiteId(0), &a), (SiteId(1), &b), (SiteId(2), &c)];
        let blob = BlobRef { oid: sha(b"z"), bytes: 1, sites: SiteSet(0b101) };
        assert_eq!(block_on(mark_blob(&bk, REPO, &blob, ChainId::Repo(REPO), Index(4))), 0);
        assert_eq!(block_on(mark_blob(&bk, REPO, &blob, ChainId::Repo(REPO), Index(4))), 0, "idempotent");
        assert_eq!((a.keys().len(), b.keys().len(), c.keys().len()), (1, 0, 1));
        let m = block_on(blob_marker(&a, REPO, blob.oid)).unwrap().unwrap();
        assert_eq!((m.index, m.checksum, m.bytes), (Index(4), blob.oid, 1));
    }

    struct Git(std::cell::Cell<u32>);

    impl Archiver for Git {
        async fn archive(&self, _: NetworkId, commit: &str, prefix: &str, f: ArchiveFormat) -> Result<Vec<u8>, String> {
            self.0.set(self.0.get() + 1);
            Ok(format!("{commit}|{prefix}|{}", f.ext()).into_bytes())
        }
    }

    #[test]
    fn an_archive_is_rebuilt_on_a_miss_with_the_same_bytes() {
        let warm = MemBucket::default();
        let git = Git(std::cell::Cell::new(0));
        let req =
            ArchiveRequest { network: NetworkId(DbId(7)), commit: "c1", prefix: "r-v1/", format: ArchiveFormat::TarGz };
        let (first, src) = block_on(archive(&warm, &git, &req)).unwrap();
        assert_eq!(src, Source::Built);
        let (again, src) = block_on(archive(&warm, &git, &req)).unwrap();
        assert_eq!((src, &again), (Source::Cache, &first));
        assert_eq!(git.0.get(), 1);
        block_on(warm.delete(&warm.keys())).unwrap();
        let (rebuilt, src) = block_on(archive(&warm, &git, &req)).unwrap();
        assert_eq!((src, rebuilt), (Source::Built, first));
        // Another ref on the same tree has another prefix, so another object.
        let other = ArchiveRequest { prefix: "r-main/", ..req };
        assert_ne!(archive_key(&other), archive_key(&ArchiveRequest { prefix: "r-v1/", ..other }));
    }
}
