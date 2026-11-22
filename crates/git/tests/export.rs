//! BF-49's pure half (`build/BUILD-PLAN.md`, M5-WP5): a vault pulls every export endpoint under an install-scoped
//! export token, following `Link` pages, and gets every chain's entries exactly once; then the token's `jti` is revoked
//! and an edge that cached "not revoked" refuses it once that answer is older than `READ_CACHE`.

use std::collections::BTreeMap;

use graftyard_git::export::{self, ChangeBatch, ExportObject, ExportSource, FeedPosition, FeedState, Route, StoredLog};
use graftyard_git::key::R2Key;
use graftyard_git::pack::PackId;
use graftyard_identity::export_token::{self, ExportClaims};
use graftyard_identity::{IdentityCache, Lookup, READ_CACHE, RequestClass, TokensFold, TokensRead as _};
use graftyard_raft::{Command as _, Fold as _, TierSet};
use graftyard_release::{InstallId, Scope};
use graftyard_types::{AccountId, ChainId, DbId, Digest, Index, NetworkId, RepoId, Tier, Timestamp, TokenShard, Ulid};

const SEED: [u8; 32] = [3; 32];
const ROOT: &str = "https://git.acme.example";

/// One follower's view of the install plus the export bucket.
#[derive(Default)]
struct Install {
    logs: BTreeMap<ChainId, Vec<Vec<u8>>>,
    objects: BTreeMap<FeedPosition, Vec<u8>>,
    snapshots: BTreeMap<R2Key, Vec<u8>>,
    packs: BTreeMap<PackId, (NetworkId, Digest, Vec<u8>)>,
    state: FeedState,
}

impl Install {
    fn commit(&mut self, chain: ChainId, n: usize) {
        let log = self.logs.entry(chain).or_default();
        for _ in 0..n {
            let i = log.len() + 1;
            log.push(format!("{chain}#{i}").into_bytes());
        }
    }

    fn commits(&self) -> Vec<(ChainId, Index)> {
        self.logs.iter().map(|(c, l)| (*c, Index(l.len() as u64))).collect()
    }

    /// One feed round, as the consumer runs it: read the state and fold in objects written after it, plan, read the
    /// entries from the follower, check, write the object, then write the state. `crash` stops before the state write.
    fn feed(&mut self, now_ms: u64, crash: bool) -> Option<ChangeBatch> {
        let mut st = self.state.clone();
        let later: Vec<ChangeBatch> = self
            .objects_after(st.last.unwrap_or_default(), usize::MAX)
            .iter()
            .map(|o| export::decode_object(o).unwrap().batch)
            .collect();
        st.recover(&later);
        let batch = export::plan(&st, &self.commits(), now_ms)?;
        let entries = batch
            .chains
            .iter()
            .map(|(c, to)| {
                let from = st.cursors.get(*c).0 + 1;
                (*c, Index(from), self.logs[c][from as usize - 1..to.0 as usize].to_vec())
            })
            .collect();
        let o = ExportObject { batch: batch.clone(), entries };
        export::check_object(&o, &st.cursors).unwrap();
        assert!(self.objects.insert(batch.position(), export::encode_object(&o)).is_none(), "never overwritten");
        st.advance(&batch);
        if !crash {
            self.state = FeedState::decode(&st.encode()).unwrap();
        }
        Some(batch)
    }
}

impl ExportSource for Install {
    fn objects_after(&self, after: FeedPosition, n: usize) -> Vec<Vec<u8>> {
        use std::ops::Bound::{Excluded, Unbounded};
        self.objects.range((Excluded(after), Unbounded)).take(n).map(|(_, o)| o.clone()).collect()
    }
    fn log(&self, chain: ChainId, from: Index, limit: u64) -> Option<StoredLog> {
        let l = self.logs.get(&chain)?;
        let rows = l.iter().enumerate().skip(from.0 as usize - 1).take(limit as usize + 1);
        let rows = rows.map(|(i, b)| (Index(i as u64 + 1), b.clone())).collect();
        Some(StoredLog { first: Index(1), archived: false, to: Index(l.len() as u64), rows })
    }
    fn snapshots(&self, chain: ChainId) -> Vec<R2Key> {
        self.snapshots
            .keys()
            .filter(|k| matches!(k, R2Key::Snapshot { chain: c, .. } if *c == chain))
            .cloned()
            .collect()
    }
    fn object(&self, key: &R2Key) -> Option<Vec<u8>> {
        self.snapshots.get(key).cloned()
    }
    fn pack(&self, network: NetworkId, id: PackId) -> Option<(Digest, Vec<u8>)> {
        self.packs.get(&id).filter(|p| p.0 == network).map(|p| (p.1, p.2.clone()))
    }
}

fn repo(n: i64) -> ChainId {
    ChainId::Repo(RepoId(DbId(n)))
}

fn query_of(link: &str) -> Vec<(String, String)> {
    let url = link.strip_prefix('<').unwrap().split_once(">; rel=\"next\"").unwrap().0;
    assert!(url.starts_with(ROOT), "{url}");
    let q = url.split_once('?').unwrap().1;
    q.split('&').map(|kv| kv.split_once('=').unwrap()).map(|(k, v)| (k.into(), v.into())).collect()
}

/// The edge's gate for one export request: verify the token, then the `jti` row, read through the isolate's
/// identity cache as a read-class request (`READ_CACHE`), from the token shard's fold.
struct Edge {
    jtis: IdentityCache<Ulid, bool>,
}

impl Edge {
    fn admit(&mut self, text: &str, shard: &TokensFold, install: InstallId, now: Timestamp) -> Option<ExportClaims> {
        let claims = export_token::verify(text, install, &graftyard_release::public_key(&SEED), now).ok()?;
        let revoked = match self.jtis.lookup(now, &claims.jti, RequestClass::Read) {
            Lookup::Hit(r) | Lookup::Stale(r) => r,
            Lookup::Fetch => {
                let r = shard.jti_revoked(&claims.jti(), now);
                self.jtis.insert(now, claims.jti, r);
                r
            }
            Lookup::Overloaded => return None,
        };
        (!revoked).then_some(claims)
    }
}

#[test]
fn bf49_vault_pulls_every_endpoint_then_the_revoked_jti_is_refused_within_read_cache() {
    let install_id = InstallId(Ulid(0x0190_0000_0000_0000_0000_0000_0000_0007));
    let mut inst = Install::default();
    let org = AccountId(DbId(40));
    let chains = [repo(1), repo(2), ChainId::Release, ChainId::Tokens(TokenShard(9)), ChainId::Ids];
    // 230 rounds of 10 s with commits on a rotating subset, so the feed spans several minutes and > 2 pages.
    let mut now = 1_700_000_000_000u64;
    for round in 0..230usize {
        for (k, c) in chains.iter().enumerate() {
            if (round + k) % 3 != 0 {
                inst.commit(*c, 1 + (round * 7 + k) % 4);
            }
        }
        // Every 17th round loses its state write; the next round recovers from the objects.
        inst.feed(now, round % 17 == 5).unwrap();
        now += export::FEED_EVERY_MS;
    }
    // A big chain whose entries page more than once.
    inst.commit(repo(3), 2_345);
    inst.feed(now, false).unwrap();
    assert_eq!(inst.feed(now, false), None, "nothing moved, no object");
    let snap = |to| R2Key::Snapshot {
        chain: repo(1),
        tier: Tier::Core,
        from: Index(0),
        to: Index(to),
        digest: Digest([1; 32]),
    };
    inst.snapshots.insert(snap(50), b"snapshot at 50".to_vec());
    inst.snapshots.insert(snap(150), b"snapshot at 150".to_vec());
    let pack = PackId(Ulid(0x0190_0000_0000_0000_0000_0000_0000_0099));
    inst.packs.insert(pack, (NetworkId(DbId(1)), Digest([0xab; 32]), b"PACK....".to_vec()));
    let foreign = PackId(Ulid(pack.0.0 + 1));
    inst.packs.insert(foreign, (NetworkId(DbId(77)), Digest([0xcd; 32]), b"PACK".to_vec()));

    let iat = Timestamp(now - 3_600_000);
    let claims = ExportClaims {
        install: install_id,
        scope: Scope::Install,
        holder: "vault".into(),
        jti: Ulid(0x0190_0000_0000_0000_0000_0000_0000_1234),
        iat,
        exp: Timestamp(iat.0 + 30 * 86_400_000),
    };
    let token = export_token::sign(&claims, &SEED);
    let shard = graftyard_identity::jti_shard(&claims.jti());
    let mut tokens = TokensFold::new(shard);
    let mut edge = Edge { jtis: IdentityCache::new(64) };
    let at = Timestamp(now);
    let get = |edge: &mut Edge, tokens: &TokensFold, path: &str, q: &[(String, String)], at: Timestamp| {
        let c = edge.admit(&token, tokens, install_id, at).ok_or(401u16)?;
        let route = Route::parse("GET", path, q).map_err(|e| e.status())?;
        let covers = |ch| export_token::covers(c.scope, ch, None);
        export::serve(&route, &covers, &inst, at.0, ROOT).map_err(|e| e.status())
    };

    // 1. Batches, page by page through `Link`.
    let mut pulled: BTreeMap<ChainId, Vec<Vec<u8>>> = BTreeMap::new();
    let (mut q, mut pages, mut objects) = (Vec::new(), 0, 0);
    loop {
        let r = get(&mut edge, &tokens, "/api/graftyard/v1/export/batches", &q, at).unwrap();
        pages += 1;
        for o in export::unframe(&r.body).unwrap() {
            let o = export::decode_object(&o).unwrap();
            objects += 1;
            for (c, first, rows) in o.entries {
                let have = pulled.entry(c).or_default();
                assert_eq!(first.0, have.len() as u64 + 1, "{c}: entries follow on with no gap or repeat");
                have.extend(rows);
            }
        }
        match r.next {
            Some(l) => q = query_of(&l),
            None => break,
        }
    }
    assert_eq!((objects, pages), (inst.objects.len(), inst.objects.len().div_ceil(export::BATCHES_PAGE)));
    assert!(pages >= 3);
    assert_eq!(pulled, inst.logs, "the feed carries every chain's every entry, Release and system chains included");

    // 2. Entries of the big chain, page by page.
    let path = format!("/api/graftyard/v1/export/chains/{}/entries", repo(3));
    let (mut q, mut rows, mut pages) = (vec![("from".to_string(), "1".to_string())], Vec::new(), 0);
    loop {
        let r = get(&mut edge, &tokens, &path, &q, at).unwrap();
        pages += 1;
        rows.extend(export::unframe(&r.body).unwrap());
        match r.next {
            Some(l) => q = query_of(&l),
            None => break,
        }
    }
    assert_eq!((pages, &rows), (3, &inst.logs[&repo(3)]));
    let q = vec![("from".to_string(), "9999".to_string())];
    assert_eq!(get(&mut edge, &tokens, &path, &q, at).unwrap().body, Vec::<u8>::new());

    // 3. Snapshots: the newest at or below `at`.
    let path = format!("/api/graftyard/v1/export/chains/{}/snapshots", repo(1));
    let r = get(&mut edge, &tokens, &path, &[("at".into(), "149".into())], at).unwrap();
    let f = export::unframe(&r.body).unwrap();
    assert_eq!((f[0].as_slice(), f[1].as_slice()), (snap(50).to_string().as_bytes(), &b"snapshot at 50"[..]));
    assert_eq!(get(&mut edge, &tokens, &path, &[("at".into(), "49".into())], at), Err(404));

    // 4. A pack, with its checksum.
    let r = get(
        &mut edge,
        &tokens,
        &format!("/api/graftyard/v1/export/packs/{pack}"),
        &[("network".into(), "1".into())],
        at,
    )
    .unwrap();
    assert_eq!((r.body.as_slice(), r.checksum), (&b"PACK...."[..], Some(Digest([0xab; 32]).hex())));

    // An org-scoped token from the same customer key sees its own repositories only, and nothing else exists for it.
    let org_token =
        export_token::sign(&ExportClaims { scope: Scope::Owner(org), holder: "acme".into(), ..claims.clone() }, &SEED);
    let org_claims = export_token::verify(&org_token, install_id, &graftyard_release::public_key(&SEED), at).unwrap();
    let owner = |c: ChainId| match c {
        ChainId::Repo(RepoId(DbId(1 | 2))) | ChainId::Network(NetworkId(DbId(1))) => Some(org),
        _ => Some(AccountId(DbId(41))),
    };
    for (ch, ok) in
        [(repo(1), true), (repo(3), false), (ChainId::Release, false), (ChainId::Tokens(TokenShard(9)), false)]
    {
        let route = Route::Entries { chain: ch, from: Index(1), limit: 10 };
        let covers = |c| export_token::covers(org_claims.scope, c, owner(c));
        assert_eq!(export::serve(&route, &covers, &inst, at.0, ROOT).is_ok(), ok, "{ch}");
    }
    let covers = |c| export_token::covers(org_claims.scope, c, owner(c));
    // The feed names every chain of the install: an org's token never reads it.
    let feed = Route::Batches { after: FeedPosition::default() };
    assert_eq!(export::serve(&feed, &covers, &inst, at.0, ROOT).map(|_| ()).map_err(|e| e.status()), Err(404));
    assert_eq!(
        export::serve(&Route::Pack { network: NetworkId(DbId(77)), id: foreign }, &covers, &inst, at.0, ROOT)
            .map(|_| ())
            .map_err(|e| e.status()),
        Err(404)
    );

    // Revocation: `DELETE export/token` commits `RevokeJti` under the token's own jti.
    let t_revoke = Timestamp(now + 4_000);
    let cmd = export_token::revoke_command(&claims);
    let (events, ()) = cmd.decide(&tokens.scratch_at(t_revoke, TierSet::CORE), &[]).unwrap();
    for e in &events {
        tokens.apply(e);
    }
    // The edge cached "not revoked" at `at`: it may serve on that answer until it is READ_CACHE old, never after.
    let path = "/api/graftyard/v1/export/batches";
    let read_cache = READ_CACHE.as_millis() as u64;
    let mut last_served = None;
    for dt in (0..=read_cache + 2_000).step_by(250) {
        let t = Timestamp(t_revoke.0 + dt);
        if get(&mut edge, &tokens, path, &[], t).is_ok() {
            last_served = Some(t);
        }
    }
    let last_served = last_served.expect("served on the cached answer just after the revocation");
    assert!(last_served.0 <= at.0 + read_cache, "served at {} ms after the cache read", last_served.0 - at.0);
    assert!(last_served.0 - t_revoke.0 <= read_cache, "refused within READ_CACHE of the revocation");
    for dt in [read_cache + 1, read_cache + 60_000, 86_400_000] {
        let t = Timestamp(t_revoke.0 + dt);
        assert_eq!(get(&mut edge, &tokens, path, &[], t).map(|_| ()), Err(401), "{dt} ms after");
    }
    // A fresh isolate refuses it at once.
    let mut cold = Edge { jtis: IdentityCache::new(64) };
    assert_eq!(get(&mut cold, &tokens, path, &[], t_revoke).map(|_| ()), Err(401));
}

#[test]
fn a_batches_cursor_past_retention_is_410_and_the_newest_is_empty() {
    let mut inst = Install::default();
    let t0 = 1_700_000_000_000u64;
    inst.commit(repo(1), 3);
    let b = inst.feed(t0, false).unwrap();
    let route = |after| Route::Batches { after };
    let all = |_| true;
    // An object written in minute m may be deleted from m's start + retention, so that is the last time its cursor reads.
    let now = b.minute * 60_000 + export::EXPORT_RETENTION_MS;
    let r = export::serve(&route(b.position()), &all, &inst, now, ROOT).unwrap();
    assert_eq!((r.body.len(), r.next), (0, None));
    let gone = export::serve(&route(b.position()), &all, &inst, now + 1, ROOT);
    assert_eq!(gone.map(|_| ()).map_err(|e| e.status()), Err(410));
    // No cursor reads from the oldest object kept, whatever the clock.
    let r = export::serve(&route(FeedPosition::default()), &all, &inst, now + 120_000, ROOT).unwrap();
    assert_eq!(export::unframe(&r.body).unwrap().len(), 1);
}
