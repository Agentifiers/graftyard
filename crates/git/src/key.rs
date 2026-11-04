//! `R2Key` and its text form (`design/DESIGN.md`, Shape, "Packs"). Pack, index, marker, and upload-journal keys start
//! with their network, so the sweeper and the third-bucket filler list one network with one prefix per kind, never a
//! chain. Indices are zero-padded to 20 digits, so a listing sorts ranges by index.

use std::fmt;
use std::str::FromStr;

use graftyard_types::{ChainId, DbId, Digest, Index, NetworkId, RepoId, Tier};
use serde::{Deserialize, Serialize};

use crate::pack::PackId;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum R2Key {
    Pack {
        network: NetworkId,
        id: PackId,
    },
    Idx {
        network: NetworkId,
        id: PackId,
    },
    /// `.idx` of an inline pack on a hot network.
    InlineIdx {
        chain: ChainId,
        index: Index,
    },
    Snapshot {
        chain: ChainId,
        tier: Tier,
        from: Index,
        to: Index,
        digest: Digest,
    },
    Archive {
        chain: ChainId,
        from: Index,
        to: Index,
        digest: Digest,
    },
    /// `key` is the SHA-256 of the encoded `CloneKey`, which the edge builds (M1-WP6); the network leads the key so a
    /// network's entries list together.
    CloneCache {
        network: NetworkId,
        key: Digest,
        replica: u8,
    },
    /// SHA-256 of the bytes: LFS objects and release assets.
    Blob {
        repo: RepoId,
        oid: Digest,
    },
    /// A derived codeload archive in the warm site's bucket beside `CloneCache` entries, named by no entry and rebuilt
    /// by `git archive` on a miss. `tree` is the tree's id joined to a hash of the archive's prefix
    /// (`crate::blob::archive_key`).
    ArchiveCache {
        network: NetworkId,
        tree: String,
        format: crate::blob::ArchiveFormat,
    },
    /// Marker for a committed `Pack` or `Blob`. `R2Key::committed` is the only constructor that callers use, so a
    /// marker never names another kind.
    Committed {
        of: Box<R2Key>,
    },
    /// The upload id of a multipart upload of a pack in flight, written to the same bucket before the first part. The
    /// Workers R2 binding cannot list multipart uploads, so the sweeper finds orphans here. Not in the design's
    /// `R2Key`.
    Upload {
        network: NetworkId,
        id: PackId,
    },
}

impl R2Key {
    /// `None` unless `of` is a `Pack` or a `Blob`.
    pub fn committed(of: R2Key) -> Option<R2Key> {
        matches!(of, R2Key::Pack { .. } | R2Key::Blob { .. }).then(|| R2Key::Committed { of: Box::new(of) })
    }

    pub fn pack_prefix(network: NetworkId) -> String {
        format!("pack/{}/", network.0.0)
    }

    pub fn marker_prefix(network: NetworkId) -> String {
        format!("committed/pack/{}/", network.0.0)
    }

    pub fn upload_prefix(network: NetworkId) -> String {
        format!("upload/{}/", network.0.0)
    }
}

fn pad(i: Index) -> String {
    format!("{:020}", i.0)
}

impl fmt::Display for R2Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            R2Key::Pack { network, id } => write!(f, "pack/{}/{id}.pack", network.0.0),
            R2Key::Idx { network, id } => write!(f, "pack/{}/{id}.idx", network.0.0),
            R2Key::InlineIdx { chain, index } => write!(f, "inline-idx/{chain}/{}", pad(*index)),
            R2Key::Snapshot { chain, tier, from, to, digest } => {
                let tier = match tier {
                    Tier::Core => "core",
                    Tier::Metadata => "metadata",
                };
                write!(f, "snapshot/{chain}/{tier}/{}-{}-{}", pad(*from), pad(*to), digest.hex())
            }
            R2Key::Archive { chain, from, to, digest } => {
                write!(f, "archive/{chain}/{}-{}-{}", pad(*from), pad(*to), digest.hex())
            }
            R2Key::CloneCache { network, key, replica } => write!(f, "clone/{}/{}/{replica}", network.0.0, key.hex()),
            R2Key::Blob { repo, oid } => write!(f, "blob/{}/{}", repo.0.0, oid.hex()),
            R2Key::ArchiveCache { network, tree, format } => {
                write!(f, "archive-cache/{}/{tree}.{}", network.0.0, format.ext())
            }
            R2Key::Committed { of } => match &**of {
                R2Key::Pack { network, id } => write!(f, "committed/pack/{}/{id}", network.0.0),
                R2Key::Blob { repo, oid } => write!(f, "committed/blob/{}/{}", repo.0.0, oid.hex()),
                other => unreachable!("a marker names only a pack or a blob, not {other:?}"),
            },
            R2Key::Upload { network, id } => write!(f, "upload/{}/{id}", network.0.0),
        }
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("not an R2 key: {0:?}")]
pub struct ParseKeyError(pub String);

impl FromStr for R2Key {
    type Err = ParseKeyError;

    fn from_str(s: &str) -> Result<R2Key, ParseKeyError> {
        let bad = || ParseKeyError(s.to_owned());
        let parts: Vec<&str> = s.split('/').collect();
        let id = |v: &str| v.parse::<i64>().ok().filter(|n| *n >= 0 && v == n.to_string()).map(DbId).ok_or_else(bad);
        let net = |v: &str| id(v).map(NetworkId);
        let pack_id = |v: &str| v.parse::<PackId>().map_err(|_| bad());
        let chain = |v: &str| v.parse::<ChainId>().map_err(|_| bad());
        let index = |v: &str| {
            (v.len() == 20 && v.bytes().all(|b| b.is_ascii_digit()))
                .then(|| v.parse().map(Index).ok())
                .flatten()
                .ok_or_else(bad)
        };
        let digest = |v: &str| Digest::parse_hex(v).ok_or_else(bad);
        let range = |v: &str| -> Result<(Index, Index, Digest), ParseKeyError> {
            let mut it = v.splitn(3, '-');
            let (a, b, d) = (it.next().ok_or_else(bad)?, it.next().ok_or_else(bad)?, it.next().ok_or_else(bad)?);
            Ok((index(a)?, index(b)?, digest(d)?))
        };
        let key = match parts.as_slice() {
            ["pack", n, file] => match file.rsplit_once('.') {
                Some((i, "pack")) => R2Key::Pack { network: net(n)?, id: pack_id(i)? },
                Some((i, "idx")) => R2Key::Idx { network: net(n)?, id: pack_id(i)? },
                _ => return Err(bad()),
            },
            ["inline-idx", c, i] => R2Key::InlineIdx { chain: chain(c)?, index: index(i)? },
            ["snapshot", c, t, r] => {
                let tier = match *t {
                    "core" => Tier::Core,
                    "metadata" => Tier::Metadata,
                    _ => return Err(bad()),
                };
                let (from, to, digest) = range(r)?;
                R2Key::Snapshot { chain: chain(c)?, tier, from, to, digest }
            }
            ["archive", c, r] => {
                let (from, to, digest) = range(r)?;
                R2Key::Archive { chain: chain(c)?, from, to, digest }
            }
            ["clone", n, k, r] => R2Key::CloneCache {
                network: net(n)?,
                key: digest(k)?,
                replica: r.parse().ok().filter(|v: &u8| *r == v.to_string()).ok_or_else(bad)?,
            },
            ["blob", r, o] => R2Key::Blob { repo: RepoId(id(r)?), oid: digest(o)? },
            ["archive-cache", n, file] => {
                let (tree, ext) = file.split_once('.').ok_or_else(bad)?;
                let ok = !tree.is_empty()
                    && tree.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase() || b == b'-');
                let format = crate::blob::ArchiveFormat::parse(ext).filter(|_| ok).ok_or_else(bad)?;
                R2Key::ArchiveCache { network: net(n)?, tree: tree.to_owned(), format }
            }
            ["committed", "pack", n, i] => R2Key::committed(R2Key::Pack { network: net(n)?, id: pack_id(i)? }).unwrap(),
            ["committed", "blob", r, o] => {
                R2Key::committed(R2Key::Blob { repo: RepoId(id(r)?), oid: digest(o)? }).unwrap()
            }
            ["upload", n, i] => R2Key::Upload { network: net(n)?, id: pack_id(i)? },
            _ => return Err(bad()),
        };
        Ok(key)
    }
}

impl From<R2Key> for String {
    fn from(k: R2Key) -> String {
        k.to_string()
    }
}

impl TryFrom<String> for R2Key {
    type Error = ParseKeyError;
    fn try_from(s: String) -> Result<R2Key, ParseKeyError> {
        s.parse()
    }
}

#[cfg(test)]
mod tests {
    use graftyard_types::{Timestamp, Ulid};

    use super::*;

    fn all() -> Vec<R2Key> {
        let network = NetworkId(DbId(77));
        let id = PackId(Ulid::new(Timestamp(1_793_491_200_000), [7; 10]));
        let chain = ChainId::Repo(RepoId(DbId(77)));
        let digest = Digest([0xab; 32]);
        vec![
            R2Key::Pack { network, id },
            R2Key::Idx { network, id },
            R2Key::InlineIdx { chain, index: Index(12) },
            R2Key::Snapshot { chain, tier: Tier::Metadata, from: Index(0), to: Index(100_000), digest },
            R2Key::Archive {
                chain: ChainId::Actions(graftyard_types::AccountId(DbId(3)), 2),
                from: Index(1),
                to: Index(9),
                digest,
            },
            R2Key::CloneCache { network, key: digest, replica: 7 },
            R2Key::Blob { repo: RepoId(DbId(5)), oid: digest },
            R2Key::ArchiveCache {
                network,
                tree: format!("{}-0011223344556677", "ab".repeat(20)),
                format: crate::blob::ArchiveFormat::TarGz,
            },
            R2Key::ArchiveCache { network, tree: "ab".repeat(20), format: crate::blob::ArchiveFormat::Zip },
            R2Key::committed(R2Key::Pack { network, id }).unwrap(),
            R2Key::committed(R2Key::Blob { repo: RepoId(DbId(5)), oid: digest }).unwrap(),
            R2Key::Upload { network, id },
        ]
    }

    #[test]
    fn every_key_round_trips_and_has_one_spelling() {
        let keys = all();
        for k in &keys {
            let s = k.to_string();
            assert_eq!(s.parse::<R2Key>().as_ref(), Ok(k), "{s}");
            assert_eq!(serde_json::from_str::<R2Key>(&serde_json::to_string(k).unwrap()).unwrap(), *k);
        }
        let mut texts: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
        texts.sort();
        texts.dedup();
        assert_eq!(texts.len(), keys.len());
    }

    #[test]
    fn listing_prefixes_cover_exactly_their_kind() {
        let network = NetworkId(DbId(77));
        for k in all() {
            let s = k.to_string();
            assert_eq!(
                s.starts_with(&R2Key::pack_prefix(network)),
                matches!(k, R2Key::Pack { .. } | R2Key::Idx { .. })
            );
            assert_eq!(
                s.starts_with(&R2Key::marker_prefix(network)),
                matches!(&k, R2Key::Committed { of } if matches!(**of, R2Key::Pack { .. }))
            );
            assert_eq!(s.starts_with(&R2Key::upload_prefix(network)), matches!(k, R2Key::Upload { .. }));
        }
        // Network 7's prefix must not match network 77's keys.
        assert!(!"pack/77/x".starts_with(&R2Key::pack_prefix(NetworkId(DbId(7)))));
    }

    #[test]
    fn markers_name_only_packs_and_blobs_and_neighbours_are_refused() {
        assert!(R2Key::committed(R2Key::Upload { network: NetworkId(DbId(1)), id: PackId(Ulid(1)) }).is_none());
        assert!(
            R2Key::committed(R2Key::committed(R2Key::Blob { repo: RepoId(DbId(1)), oid: Digest([0; 32]) }).unwrap())
                .is_none()
        );
        let id = PackId(Ulid(u128::MAX >> 3)).to_string();
        for bad in [
            format!("pack/77/{id}.zip"),
            format!("pack/077/{id}.pack"),
            format!("pack/-1/{id}.pack"),
            format!("pack/77/{id}"),
            format!("pack/77/x/{id}.pack"),
            "archive/repo:1/1-2-ab".to_owned(),
            format!("archive/repo:1/{}-{}-{}", "0".repeat(20), "0".repeat(19), "a".repeat(64)),
            format!("archive/repo:1/{}-{}-{}", "0".repeat(20), "0".repeat(20), "A".repeat(64)),
            "clone/1/".to_owned() + &"a".repeat(64) + "/07",
            format!("committed/upload/77/{id}"),
            format!("committed/pack/77/{}", id.to_lowercase()),
            "snapshot/repo:1/cold/".to_owned(),
            "archive-cache/77/abc.tar".to_owned(),
            "archive-cache/77/ABC.zip".to_owned(),
            "archive-cache/77/.zip".to_owned(),
            format!("committed/archive-cache/77/{}.zip", "a".repeat(40)),
        ] {
            assert!(bad.parse::<R2Key>().is_err(), "{bad}");
        }
    }
}
