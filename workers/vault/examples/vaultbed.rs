//! `tests/vaultbed/run.sh`'s fixture: one `ExportObject` of a repository chain whose two entries name stored packs,
//! with the packs the bed plants in site 0's bucket under their `Committed` markers. The first pack's bytes are the
//! entry's; the second's are not, so the vault must refuse to store it. The first spans two of the vault's upload
//! parts. The object sits at feed minute `minute`, before the bed's feed rounds. Writes `object.bin`, `good.pack`,
//! `bad.pack`, and `plant.json` (keys and marker metadata) into `dir`.
//!
//!     cargo run -q -p graftyard-vault --example vaultbed -- <dir> <minute>

use graftyard_git::export::{self, ChangeBatch, ExportObject};
use graftyard_git::pack::{PackId, PackRef};
use graftyard_git::repo::{REPO_FORMAT_V1, RepoEvent};
use graftyard_git::store::marker_meta;
use graftyard_git::{Marker, R2Key};
use graftyard_raft::{EntryBody, LogEntry, Term, TierSet, codec};
use graftyard_types::{ChainId, DbId, Digest, Index, NetworkId, RepoId, RequestId, SiteSet, Timestamp, Ulid};
use serde_json::json;
use sha2::{Digest as _, Sha256};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, dir, minute] = args.as_slice() else { panic!("usage: vaultbed <dir> <minute>") };
    let dir = std::path::PathBuf::from(dir);
    let minute: u64 = minute.parse().unwrap();
    let chain = ChainId::Repo(RepoId(DbId(900_500)));
    let network = NetworkId(DbId(900_500));
    let at = 1_800_000_000_000u64;
    let good: Vec<u8> = (0..graftyard_vault::PART_BYTES + (1 << 20)).map(|i| (i * 7 % 251) as u8).collect();
    let named_bad = b"PACK the bytes the entry names".to_vec();
    let served_bad = b"PACK other bytes, same length!".to_vec();
    let sum = |b: &[u8]| Digest(Sha256::digest(b).into());
    let ids = [PackId(Ulid((u128::from(at) << 80) | 1)), PackId(Ulid((u128::from(at) << 80) | 2))];
    let named = [&good, &named_bad];
    let row = |k: usize, pack: Option<PackRef>| {
        codec::to_bytes(&LogEntry {
            term: Term(1),
            index: Index(k as u64 + 1),
            prev: Digest([0; 32]),
            format: REPO_FORMAT_V1,
            tiers: TierSet::CORE,
            at: Timestamp(at),
            requests: Default::default(),
            extend: None,
            body: EntryBody::Events(vec![RepoEvent::Refs {
                push: RequestId(Ulid(k as u128 + 1)),
                actor: None,
                updates: Vec::new(),
                pack,
            }]),
        })
    };
    let rows: Vec<Vec<u8>> = (0..2)
        .map(|k| {
            let pack = PackRef::Stored {
                id: ids[k],
                checksum: sum(named[k]),
                bytes: named[k].len() as u64,
                sites: SiteSet(1),
            };
            row(k, Some(pack))
        })
        .collect();
    let o = ExportObject {
        batch: ChangeBatch { minute, seq: 0, chains: vec![(chain, Index(2))] },
        entries: vec![(chain, Index(1), rows)],
    };
    let w = |name: &str, b: &[u8]| std::fs::write(dir.join(name), b).unwrap();
    w("object.bin", &export::encode_object(&o));
    w("good.pack", &good);
    w("bad.pack", &served_bad);
    let plant = |k: usize, bytes: &[u8]| {
        let m = Marker { chain, index: Index(k as u64 + 1), checksum: sum(named[k]), bytes: named[k].len() as u64 };
        json!({
            "pack": R2Key::Pack { network, id: ids[k] }.to_string(),
            "marker": R2Key::committed(R2Key::Pack { network, id: ids[k] }).unwrap().to_string(),
            "meta": marker_meta(&m).into_iter().collect::<std::collections::BTreeMap<_, _>>(),
            "sha256": sum(bytes).hex(),
        })
    };
    let out = json!({
        "object": o.batch.key().to_string(),
        "chain": chain.to_string(),
        "good": plant(0, &good),
        "bad": plant(1, &served_bad),
    });
    w("plant.json", serde_json::to_string_pretty(&out).unwrap().as_bytes());
}
