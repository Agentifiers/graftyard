//! `actions-cache` (`design/SHARED.md`, "Actions", Cache; `decisions/DECISIONS.md`, "`actions-cache`"): cache v1, the
//! `_apis/artifactcache` REST surface every `@actions/cache` from 4 to 6 speaks on a GHES-classified host
//! (`research/actions-protocol.md#43`, `#49`). This module is the pure half: the rows an owner's Actions shard keeps
//! for each entry, the scopes a run reads and writes, key matching, LRU eviction, the chunk tiling a commit checks,
//! and the signed download capability. The shard's commands are in `crate::shard`, the routes in
//! `workers/edge/src/cache.rs` and `workers/site/src/actions_cache.rs`.

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ed25519_dalek::{Signer as _, SigningKey, Verifier as _, VerifyingKey};
use graftyard_types::{ChainId, RepoId, Timestamp};
use serde::{Deserialize, Serialize};

use crate::ids::{JobId, RunId};
use crate::runsvc::ServiceKey;

/// One repository's cache bytes at most, LRU past it [doc: GHES 3.22, "Dependency caching reference", "By default,
/// the limit is 10 GB per repository"; `design/SHARED.md`, "Repository repair, tombstones, and retention"].
pub const CACHE_REPO_MAX: u64 = 10 << 30;
/// An entry unread this long is evicted [doc: GHES 3.22, same page, "GitHub will remove any cache entries that have
/// not been accessed in over 7 days"].
pub const CACHE_UNUSED_MS: u64 = 7 * 24 * 3_600_000;
/// A reservation not committed this long no longer holds its key, and the sweep removes it and its chunks [guess,
/// `decisions/OPEN-QUESTIONS.md`, F117]: a save's upload of 10 GB at about 30 MB/s takes under 6 minutes.
pub const CACHE_RESERVE_STALE_MS: u64 = 3_600_000;
/// A hit moves an entry's last use at most once in this long [guess, F117], so a cache read by every job of a busy
/// repository writes the chain at most once an hour; the 7-day expiry needs nothing finer.
pub const CACHE_USED_EVERY_MS: u64 = 3_600_000;
/// Live entries (reserved or committed) one Actions shard holds at most [guess, F117]. The shard's fold is stored as
/// one SQLite blob, which may not pass 8 MB [doc: https://developers.cloudflare.com/durable-objects/platform/limits/,
/// "Maximum string, BLOB or table row size"], and an entry takes up to about 750 bytes with a 512-character key, so
/// 4,000 keep the cache rows under 3 MB beside the runs and jobs. A reserve past it evicts the shard's least recently
/// used entries first, as GHES evicts rather than refuses [doc: same page, "There is no limit on the number of
/// caches"].
pub const CACHE_ENTRIES_MAX: usize = 4_000;
/// How long a download URL verifies [guess, F117]: the toolkit fetches it right after the lookup.
pub const CACHE_URL_LIFE_S: u64 = 600;
/// The events whose runs may write the default branch's scope [doc: GHES 3.22, same page, "Cache access for low-trust
/// workflow triggers"]. Any other run on the default branch reads it only.
pub const DEFAULT_SCOPE_WRITERS: [&str; 7] =
    ["push", "workflow_dispatch", "repository_dispatch", "delete", "registry_package", "page_build", "schedule"];

/// A cache entry's number on its shard, the toolkit's `cacheId`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CacheId(pub u64);

/// Wire tags: postcard stores the variants by position (`crates/git/tests/wire_tags.golden`). Add one only at the end.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheState {
    /// Reserved by `job`, whose chunks are uploading.
    Reserved {
        job: JobId,
    },
    Committed,
}

/// One entry. An evicted one leaves the fold, and the leader deletes its chunks after (`ActionsEvent::CacheEvicted`).
/// `created` is its commit time once committed (the toolkit's `creationTime`, which orders prefix matches),
/// its reserve time before; `last_used` the newest hit `CACHE_USED_EVERY_MS` let through, or `created`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheEntry {
    pub repo: RepoId,
    pub scope: Box<str>,
    pub key: Box<str>,
    pub version: Box<str>,
    pub size: u64,
    pub created: Timestamp,
    pub last_used: Timestamp,
    pub state: CacheState,
}

impl CacheEntry {
    pub fn committed(&self) -> bool {
        self.state == CacheState::Committed
    }
    /// A reservation that still holds its key at `now`.
    pub fn holds(&self, now: Timestamp) -> bool {
        match self.state {
            CacheState::Committed => true,
            CacheState::Reserved { .. } => now.0.saturating_sub(self.created.0) < CACHE_RESERVE_STALE_MS,
        }
    }
    /// When the sweep removes it: a reservation once stale, an entry 7 days after its last use.
    pub fn due(&self) -> Timestamp {
        Timestamp(match self.state {
            CacheState::Reserved { .. } => self.created.0 + CACHE_RESERVE_STALE_MS,
            CacheState::Committed => self.last_used.0 + CACHE_UNUSED_MS,
        })
    }
}

/// The shard's cache rows (format 0x15.6): every entry, the highest id assigned (ids only grow, so a removed one never
/// returns and its chunks are safe to delete), and each `pull_request` run's base branch
/// (`ActionsEvent::RunBase`), which its jobs read caches from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheRows {
    pub entries: BTreeMap<CacheId, CacheEntry>,
    pub next: u64,
    pub bases: BTreeMap<RunId, Box<str>>,
}

impl CacheRows {
    /// The earliest time an entry expires, where the leader's alarm goes.
    pub fn due(&self) -> Option<Timestamp> {
        self.entries.values().map(CacheEntry::due).min()
    }
}

/// The scopes a run reads, in GHES's order with none twice: its own ref, then a `pull_request` run's base branch,
/// then the default branch [doc: GHES 3.22, same page, "Restrictions for accessing a cache": a pull request's run
/// "can also restore caches created in the base branch"; "Cache key matching": "the `cache` action retries the same
/// steps on the default branch"].
pub fn scopes(run_ref: &str, base: Option<&str>, default_branch: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(3);
    for s in [Some(run_ref), base, Some(default_branch)].into_iter().flatten() {
        if !out.iter().any(|o| o == s) {
            out.push(s.to_owned());
        }
    }
    out
}

/// Whether a run of `event` on `run_ref` may write its scope: anywhere but the default branch, and there only from
/// `DEFAULT_SCOPE_WRITERS`.
pub fn may_write(event: &str, run_ref: &str, default_branch: &str) -> bool {
    run_ref != default_branch || DEFAULT_SCOPE_WRITERS.contains(&event)
}

/// The entry a lookup of `keys` (the primary key, then the restore keys) at `version` restores, from `entries` of one
/// repository, searching `scopes` in order: in each scope, the primary key exactly, then as a prefix, then each
/// restore key as a prefix; of several matches the newest `created`, then the highest id [doc: GHES 3.22, same page,
/// "Cache key matching"; "If there are multiple partial matches for a restore key, the action returns the most
/// recently created cache"]. Only committed entries of the same version match.
pub fn find<'a>(
    entries: impl IntoIterator<Item = (CacheId, &'a CacheEntry)>,
    keys: &[&str],
    version: &str,
    scopes: &[String],
) -> Option<(CacheId, &'a CacheEntry)> {
    let pool: Vec<(CacheId, &CacheEntry)> =
        entries.into_iter().filter(|(_, e)| e.committed() && &*e.version == version).collect();
    let newest = |hit: &dyn Fn(&CacheEntry) -> bool, scope: &str| {
        pool.iter().filter(|(_, e)| &*e.scope == scope && hit(e)).max_by_key(|(id, e)| (e.created, *id)).copied()
    };
    for scope in scopes {
        let (first, rest) = keys.split_first()?;
        if let Some(h) = newest(&|e| &*e.key == *first, scope) {
            return Some(h);
        }
        for k in std::iter::once(first).chain(rest) {
            if let Some(h) = newest(&|e| e.key.starts_with(*k), scope) {
                return Some(h);
            }
        }
    }
    None
}

/// The committed entries to evict so that `entries`' committed bytes are at most `limit`, never `keep`: least recently
/// used first, then oldest, then lowest id [doc: GHES 3.22, same page, "deleting the caches in order of last access
/// date, from oldest to most recent"].
pub fn lru<'a>(
    entries: impl IntoIterator<Item = (CacheId, &'a CacheEntry)>,
    keep: CacheId,
    limit: u64,
) -> Vec<CacheId> {
    let mut live: Vec<(CacheId, &CacheEntry)> = entries.into_iter().filter(|(_, e)| e.committed()).collect();
    let mut total: u64 = live.iter().map(|(_, e)| e.size).sum();
    live.sort_by_key(|(id, e)| (e.last_used, e.created, *id));
    let mut out = Vec::new();
    for (id, e) in live {
        if total <= limit {
            break;
        }
        if id != keep {
            total -= e.size;
            out.push(id);
        }
    }
    out
}

/// `Content-Range: bytes s-e/*` as `(s, e)`, inclusive, as the toolkit sends each chunk (`getContentRange`).
pub fn content_range(v: &str) -> Option<(u64, u64)> {
    let (range, _total) = v.trim().strip_prefix("bytes ")?.split_once('/')?;
    let (s, e) = range.split_once('-')?;
    let (s, e) = (s.parse().ok()?, e.parse().ok()?);
    (s <= e).then_some((s, e))
}

/// Why chunks do not make an entry: commit refuses them, and a download answers nothing.
#[derive(Debug, PartialEq, Eq)]
pub enum Tiling {
    Gap { at: u64 },
    Overlap { at: u64 },
    Short { have: u64 },
    Long { have: u64 },
}

/// Whether chunks `(start, len)` cover `[0, size)` exactly, each byte once, in any order.
pub fn tiles(chunks: &[(u64, u64)], size: u64) -> Result<(), Tiling> {
    let mut c: Vec<(u64, u64)> = chunks.to_vec();
    c.sort();
    let mut at = 0u64;
    for (s, n) in c {
        if s > at {
            return Err(Tiling::Gap { at });
        }
        if s < at {
            return Err(Tiling::Overlap { at: s });
        }
        at = s + n;
    }
    match at.cmp(&size) {
        std::cmp::Ordering::Equal => Ok(()),
        std::cmp::Ordering::Less => Err(Tiling::Short { have: at }),
        std::cmp::Ordering::Greater => Err(Tiling::Long { have: at }),
    }
}

/// A download's `Range` header against an entry of `size` bytes: `Ok(None)` for the whole body (no header, or one this
/// does not take, such as several ranges, which an answer of the whole body satisfies), `Ok(Some((s, e)))` inclusive,
/// or `Err(Unsatisfiable)` for a range that starts at or past the end (416).
pub fn byte_range(h: Option<&str>, size: u64) -> Result<Option<(u64, u64)>, Unsatisfiable> {
    let Some(spec) = h.and_then(|h| h.trim().strip_prefix("bytes=")) else { return Ok(None) };
    if spec.contains(',') {
        return Ok(None);
    }
    let Some((a, b)) = spec.split_once('-') else { return Ok(None) };
    let (s, e) = match (a.trim(), b.trim()) {
        ("", n) => match n.parse::<u64>() {
            Ok(0) | Err(_) => return Err(Unsatisfiable),
            Ok(n) => (size.saturating_sub(n), size.saturating_sub(1)),
        },
        (s, "") => match s.parse::<u64>() {
            Ok(s) => (s, size.saturating_sub(1)),
            Err(_) => return Ok(None),
        },
        (s, e) => match (s.parse::<u64>(), e.parse::<u64>()) {
            (Ok(s), Ok(e)) if s <= e => (s, e.min(size.saturating_sub(1))),
            _ => return Ok(None),
        },
    };
    if size == 0 || s >= size { Err(Unsatisfiable) } else { Ok(Some((s, e))) }
}

/// A `Range` no byte of the entry satisfies.
#[derive(Debug, PartialEq, Eq)]
pub struct Unsatisfiable;

/// The reads a download of bytes `s..=e` makes from chunks `(start, len)` that tile the entry (`tiles`), in order:
/// each chunk's index in `chunks` sorted by start, the offset into it, and the length. A range that starts in one chunk
/// and ends in another reads the tail of the first and the head of the last.
pub fn pieces(chunks: &[(u64, u64)], s: u64, e: u64) -> Vec<(usize, u64, u64)> {
    let mut c: Vec<(usize, u64, u64)> = chunks.iter().enumerate().map(|(i, (st, n))| (i, *st, *n)).collect();
    c.sort_by_key(|(_, st, _)| *st);
    c.into_iter()
        .filter(|(_, st, n)| *n > 0 && st + n > s && *st <= e)
        .map(|(i, st, n)| {
            let from = s.max(st);
            let to = e.min(st + n - 1);
            (i, from - st, to - from + 1)
        })
        .collect()
}

/// An entry's chunk objects, in the owner's home bucket beside the job logs: `actions/cache/<chain>/<id>/`, each chunk
/// at its start offset in 20 digits so a listing is in order.
pub fn chunk_prefix(chain: ChainId, id: CacheId) -> String {
    format!("actions/cache/{chain}/{}/", id.0)
}

pub fn chunk_key(chain: ChainId, id: CacheId, start: u64) -> String {
    format!("{}{start:020}", chunk_prefix(chain, id))
}

/// A chunk's start offset from its key under `chunk_prefix`.
pub fn chunk_start(prefix: &str, key: &str) -> Option<u64> {
    let s = key.strip_prefix(prefix)?;
    (s.len() == 20).then(|| s.parse().ok()).flatten()
}

fn capability(chain: ChainId, id: CacheId, size: u64, exp: u64) -> String {
    format!("graftyard/cache-download/1\0{chain}\0{}\0{size}\0{exp}", id.0)
}

/// The download URL's query for one entry: its size and expiry, signed with the run service's key, so the edge verifies
/// it offline under `RUN_SERVICE_PUBLIC` and the URL carries no bearer token (`decisions/DECISIONS.md`,
/// "`actions-cache`").
pub fn sign_download(key: &ServiceKey, chain: ChainId, id: CacheId, size: u64, exp: u64) -> String {
    let sig = SigningKey::from_bytes(&key.0).sign(capability(chain, id, size, exp).as_bytes());
    format!("n={size}&e={exp}&s={}", B64.encode(sig.to_bytes()))
}

/// The size a download query grants for `chain`'s entry `id`, if `public` signed it and it has not expired at `now`.
pub fn verify_download(query: &str, public: &[u8; 32], chain: ChainId, id: CacheId, now: Timestamp) -> Option<u64> {
    let mut size = None;
    let mut exp = None;
    let mut sig = None;
    for (k, v) in query.split('&').filter_map(|p| p.split_once('=')) {
        match k {
            "n" => size = v.parse::<u64>().ok(),
            "e" => exp = v.parse::<u64>().ok(),
            "s" => sig = B64.decode(v).ok().and_then(|b| <[u8; 64]>::try_from(b).ok()),
            _ => {}
        }
    }
    let (size, exp, sig) = (size?, exp?, sig?);
    if exp <= now.0 / 1000 {
        return None;
    }
    VerifyingKey::from_bytes(public)
        .ok()?
        .verify(capability(chain, id, size, exp).as_bytes(), &ed25519_dalek::Signature::from_bytes(&sig))
        .ok()?;
    Some(size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use graftyard_types::{AccountId, DbId};

    const R: RepoId = RepoId(DbId(7));

    fn e(scope: &str, key: &str, version: &str, created: u64, state: CacheState) -> CacheEntry {
        CacheEntry {
            repo: R,
            scope: scope.into(),
            key: key.into(),
            version: version.into(),
            size: 10,
            created: Timestamp(created),
            last_used: Timestamp(created),
            state,
        }
    }

    fn c(scope: &str, key: &str, created: u64) -> CacheEntry {
        e(scope, key, "v", created, CacheState::Committed)
    }

    fn rows(v: Vec<CacheEntry>) -> BTreeMap<CacheId, CacheEntry> {
        v.into_iter().enumerate().map(|(i, e)| (CacheId(i as u64 + 1), e)).collect()
    }

    fn hit(m: &BTreeMap<CacheId, CacheEntry>, keys: &[&str], scopes: &[String]) -> Option<u64> {
        find(m.iter().map(|(k, v)| (*k, v)), keys, "v", scopes).map(|(id, _)| id.0)
    }

    const MAIN: &str = "refs/heads/main";
    const REL: &str = "refs/heads/rel";
    const PR: &str = "refs/pull/3/merge";

    /// A pull request's run reads its merge ref, then its base, then the default branch; a base that is the default
    /// branch, or a run on the default branch, is searched once.
    #[test]
    fn a_pull_request_reads_its_ref_then_its_base_then_the_default_branch() {
        assert_eq!(scopes(PR, Some(REL), MAIN), [PR, REL, MAIN]);
        assert_eq!(scopes(PR, Some(MAIN), MAIN), [PR, MAIN]);
        assert_eq!(scopes(MAIN, None, MAIN), [MAIN]);
        assert_eq!(scopes(REL, None, MAIN), [REL, MAIN]);
    }

    /// The base branch's entry wins over a newer one on the default branch for a pull request that targets `rel`,
    /// which BF-69's asymmetric case reads; without the base the default branch's answers.
    #[test]
    fn the_base_branch_scope_comes_before_the_default_branch() {
        let m = rows(vec![c(REL, "npm-rel", 5), c(MAIN, "npm-main", 9)]);
        assert_eq!(hit(&m, &["npm-x", "npm-"], &scopes(PR, Some(REL), MAIN)), Some(1));
        assert_eq!(hit(&m, &["npm-x", "npm-"], &scopes(PR, None, MAIN)), Some(2));
    }

    /// A hit in the run's own scope beats a newer one on the default branch; a sibling branch's entry never matches.
    #[test]
    fn the_own_scope_comes_first_and_a_sibling_never_matches() {
        let m = rows(vec![c("refs/heads/f", "k-1", 1), c(MAIN, "k-1", 9), c("refs/heads/g", "k-1", 20)]);
        assert_eq!(hit(&m, &["k-1"], &scopes("refs/heads/f", None, MAIN)), Some(1));
        assert_eq!(hit(&m, &["k-1"], &scopes("refs/heads/h", None, MAIN)), Some(2));
        assert_eq!(hit(&m, &["k-1"], &scopes(MAIN, None, MAIN)), Some(2), "main never reads a child branch");
    }

    /// In one scope: the exact key first even when a longer key with that prefix is newer, then the primary key as a
    /// prefix, then each restore key in order, newest first among prefix matches.
    #[test]
    fn exact_then_prefix_then_restore_keys_in_order_newest_first() {
        let m = rows(vec![c(MAIN, "npm-a", 1), c(MAIN, "npm-ab", 9), c(MAIN, "npm-b1", 3), c(MAIN, "npm-b2", 4)]);
        let s = scopes(MAIN, None, MAIN);
        assert_eq!(hit(&m, &["npm-a"], &s), Some(1), "exact beats a newer prefix match");
        assert_eq!(hit(&m, &["npm-x", "npm-b"], &s), Some(4), "newest of the restore key's matches");
        assert_eq!(hit(&m, &["npm-x", "npm-b", "npm-"], &s), Some(4), "restore keys in order");
        assert_eq!(hit(&m, &["npm-x", "npm-"], &s), Some(2), "the newest of all");
        assert_eq!(hit(&m, &["yarn-"], &s), None);
    }

    /// Another version or a reservation never matches.
    #[test]
    fn only_committed_entries_of_the_version_match() {
        let m = rows(vec![
            e(MAIN, "k", "w", 9, CacheState::Committed),
            e(MAIN, "k", "v", 8, CacheState::Reserved { job: JobId(DbId(1)) }),
            c(MAIN, "k", 1),
        ]);
        assert_eq!(hit(&m, &["k"], &scopes(MAIN, None, MAIN)), Some(3));
    }

    #[test]
    fn only_trusted_events_write_the_default_branch() {
        assert!(may_write("push", MAIN, MAIN));
        assert!(may_write("schedule", MAIN, MAIN));
        assert!(!may_write("pull_request_target", MAIN, MAIN));
        assert!(!may_write("issue_comment", MAIN, MAIN));
        assert!(!may_write("workflow_run", MAIN, MAIN));
        assert!(may_write("pull_request", PR, MAIN));
        assert!(may_write("workflow_run", REL, MAIN));
    }

    /// LRU evicts by last use, not by creation, stops as soon as the total fits, and never takes the entry kept.
    #[test]
    fn lru_evicts_least_recently_used_until_it_fits_and_keeps_the_new_entry() {
        let mut m = rows(vec![c(MAIN, "a", 1), c(MAIN, "b", 2), c(MAIN, "c", 3), c(MAIN, "new", 9)]);
        m.get_mut(&CacheId(1)).unwrap().last_used = Timestamp(8);
        let it = || m.iter().map(|(k, v)| (*k, v));
        assert_eq!(lru(it(), CacheId(4), 40), Vec::<CacheId>::new(), "40 bytes fit");
        assert_eq!(lru(it(), CacheId(4), 30), [CacheId(2)], "b was used least recently, not a");
        assert_eq!(lru(it(), CacheId(4), 10), [CacheId(2), CacheId(3), CacheId(1)]);
        assert_eq!(lru(it(), CacheId(4), 0), [CacheId(2), CacheId(3), CacheId(1)], "the new entry stays");
    }

    #[test]
    fn content_range_is_the_toolkits_inclusive_range() {
        assert_eq!(content_range("bytes 0-33554431/*"), Some((0, 33_554_431)));
        assert_eq!(content_range("bytes 5-4/*"), None);
        assert_eq!(content_range("bytes=0-1"), None);
    }

    /// Chunks tile only when they cover every byte once: a gap, an overlap, a short or long end each refuse.
    #[test]
    fn chunks_tile_exactly_or_name_what_is_wrong() {
        assert_eq!(tiles(&[(32, 8), (0, 32)], 40), Ok(()));
        assert_eq!(tiles(&[(0, 32), (33, 7)], 40), Err(Tiling::Gap { at: 32 }));
        assert_eq!(tiles(&[(0, 32), (31, 9)], 40), Err(Tiling::Overlap { at: 31 }));
        assert_eq!(tiles(&[(0, 32)], 40), Err(Tiling::Short { have: 32 }));
        assert_eq!(tiles(&[(0, 32), (32, 9)], 40), Err(Tiling::Long { have: 41 }));
        assert_eq!(tiles(&[], 0), Ok(()));
    }

    /// Ranges as the edge answers them: suffix, open-ended, and closed ranges clamp to the entry; one at or past the end
    /// is unsatisfiable; several ranges or a malformed one take the whole body.
    #[test]
    fn a_range_header_clamps_to_the_entry_or_is_unsatisfiable() {
        assert_eq!(byte_range(None, 100), Ok(None));
        assert_eq!(byte_range(Some("bytes=10-19"), 100), Ok(Some((10, 19))));
        assert_eq!(byte_range(Some("bytes=90-200"), 100), Ok(Some((90, 99))));
        assert_eq!(byte_range(Some("bytes=95-"), 100), Ok(Some((95, 99))));
        assert_eq!(byte_range(Some("bytes=-5"), 100), Ok(Some((95, 99))));
        assert_eq!(byte_range(Some("bytes=-500"), 100), Ok(Some((0, 99))));
        assert_eq!(byte_range(Some("bytes=100-"), 100), Err(Unsatisfiable));
        assert_eq!(byte_range(Some("bytes=0-1,5-6"), 100), Ok(None));
        assert_eq!(byte_range(Some("bytes=9-3"), 100), Ok(None));
        assert_eq!(byte_range(Some("items=0-1"), 100), Ok(None));
    }

    /// A range that starts in one chunk and ends in another reads the first chunk's tail, every chunk between whole,
    /// and the last chunk's head, from chunks listed in any order.
    #[test]
    fn a_range_across_chunk_boundaries_reads_the_right_slices() {
        let chunks = [(64, 36), (0, 32), (32, 32)];
        assert_eq!(pieces(&chunks, 30, 70), [(1, 30, 2), (2, 0, 32), (0, 0, 7)]);
        assert_eq!(pieces(&chunks, 32, 63), [(2, 0, 32)], "exactly one chunk");
        assert_eq!(pieces(&chunks, 31, 32), [(1, 31, 1), (2, 0, 1)], "one byte either side of a boundary");
        assert_eq!(pieces(&chunks, 0, 99).iter().map(|p| p.2).sum::<u64>(), 100);
        assert_eq!(pieces(&chunks, 99, 99), [(0, 35, 1)]);
    }

    #[test]
    fn chunk_keys_sort_by_start_and_parse_back() {
        let ch = ChainId::Actions(AccountId(DbId(3)), 4);
        let (p, a, b) = (chunk_prefix(ch, CacheId(9)), chunk_key(ch, CacheId(9), 9), chunk_key(ch, CacheId(9), 10));
        assert!(a < b);
        assert_eq!(chunk_start(&p, &b), Some(10));
        assert_eq!(chunk_start(&chunk_prefix(ch, CacheId(1)), &b), None, "another entry's chunk");
    }

    /// The capability names one entry of one shard and its size, and lapses: another id, chain, size, or a time past
    /// its expiry verifies nothing.
    #[test]
    fn a_download_capability_names_one_entry_and_expires() {
        let key = ServiceKey([7; 32]);
        let public = key.public();
        let ch = ChainId::Actions(AccountId(DbId(3)), 4);
        let q = sign_download(&key, ch, CacheId(9), 123, 1_000);
        let at = |s: u64| Timestamp(s * 1000);
        assert_eq!(verify_download(&q, &public, ch, CacheId(9), at(999)), Some(123));
        assert_eq!(verify_download(&q, &public, ch, CacheId(9), at(1_000)), None, "expired");
        assert_eq!(verify_download(&q, &public, ch, CacheId(8), at(1)), None, "another entry");
        let other = ChainId::Actions(AccountId(DbId(3)), 5);
        assert_eq!(verify_download(&q, &public, other, CacheId(9), at(1)), None, "another shard");
        let bigger = q.replace("n=123", "n=124");
        assert_eq!(verify_download(&bigger, &public, ch, CacheId(9), at(1)), None, "another size");
        let wrong = ServiceKey([8; 32]).public();
        assert_eq!(verify_download(&q, &wrong, ch, CacheId(9), at(1)), None);
    }
}
