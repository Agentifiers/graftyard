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
        let commits = self.commits();
        self.feed_from(now_ms, crash, &commits)
    }

    /// `feed`, planned from `commits`: what the round's reports and probes gave it.
    fn feed_from(&mut self, now_ms: u64, crash: bool, commits: &[(ChainId, Index)]) -> Option<ChangeBatch> {
        let mut st = self.state.clone();
        let later: Vec<ChangeBatch> = self
            .objects_after(st.last.unwrap_or_default(), usize::MAX)
            .iter()
            .map(|o| export::decode_object(o).unwrap().batch)
            .collect();
        st.recover(&later);
        let batch = export::plan(&st, commits, now_ms)?;
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
    // Its own network's pack is served as on main until the network forks to another owner, whose private objects
    // the pack may hold; then only the install's token reads it (DECISIONS "`fork-pack-scope`").
    let own = Route::Pack { network: NetworkId(DbId(1)), id: pack };
    for (forked, scope, ok) in [
        (false, org_claims.scope, true),
        (true, org_claims.scope, false),
        (true, Scope::Owner(AccountId(DbId(41))), false),
        (true, Scope::Install, true),
    ] {
        let covers = |c| match c {
            ChainId::Network(n) => export_token::covers_network(scope, n, org, forked),
            c => export_token::covers(scope, c, owner(c)),
        };
        assert_eq!(export::serve(&own, &covers, &inst, at.0, ROOT).is_ok(), ok, "forked {forked}, {scope:?}");
    }

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

/// The ruling after m3fx49b: a chain reaches the feed whether or not anyone leads it. `Release` and `Ids` have no
/// leader and are probed every round; the repository chains of a site's registry shard are probed once a sweep; only
/// `repo:1` has a leader that reports. Over one sweep and a second, every chain's every entry is in the feed exactly
/// once, and the probed system chains are in the first round's object.
#[test]
fn a_chain_whose_leader_never_reports_still_reaches_the_feed_exactly_once() {
    use graftyard_types::{MemberRegistry, REGISTRY_SHARDS, RegistryRow, SiteId};
    let mut inst = Install::default();
    let led = repo(1);
    let idle: Vec<ChainId> = (2..40).map(repo).chain([ChainId::Names(graftyard_types::NameShard(7))]).collect();
    for c in [led, ChainId::Release, ChainId::Ids].iter().chain(&idle) {
        inst.commit(*c, 3);
    }
    // Every group has a row on each of its three sites; the first names the site the sweep probes it on.
    let sites = [SiteId(0), SiteId(1), SiteId(2)];
    let chains: Vec<ChainId> = inst.logs.keys().copied().collect();
    let rows = |site: SiteId, shard: u8| -> Vec<RegistryRow> {
        chains
            .iter()
            .filter(|c| MemberRegistry::for_chain(site, **c).shard == shard)
            .enumerate()
            .map(|(k, c)| RegistryRow {
                chain: *c,
                do_name: format!("{c}#1#f{}", site.0).into(),
                generation: 1,
                sites: [0, 1, 2].iter().cycle().skip(k % 3).take(3).map(|s| SiteId(*s)).collect(),
            })
            .collect()
    };
    let sweep = u64::from(REGISTRY_SHARDS) * sites.len() as u64 / export::SWEEP_SHARDS;
    let mut now = 1_700_000_000_000u64;
    let mut first = None;
    for n in 0..2 * sweep {
        // The leader commits and reports; nothing else ever reports.
        if n % 5 == 0 {
            inst.commit(led, 1);
        }
        let reports = vec![(led, Index(inst.logs[&led].len() as u64))];
        let mut probed: Vec<ChainId> = export::ROUND_PROBES.to_vec();
        for (i, shard) in export::sweep_slots(n, export::SWEEP_SHARDS, sites.len()) {
            let rows = rows(sites[i], shard);
            probed.extend(export::swept(&rows, sites[i]).into_iter().map(|r| r.chain));
        }
        // A probe reads a follower's commit index, which here is the chain's.
        let probes: Vec<(ChainId, Index)> = probed
            .into_iter()
            .filter(|c| !reports.iter().any(|(r, _)| r == c))
            .map(|c| (c, Index(inst.logs[&c].len() as u64)))
            .collect();
        let commits = export::with_probes(&reports, &probes);
        let b = inst.feed_from(now, n % 23 == 4, &commits);
        first = first.or(b);
        now += export::FEED_EVERY_MS;
    }
    let first = first.expect("the first round wrote an object");
    for c in [ChainId::Release, ChainId::Ids] {
        assert!(first.chains.iter().any(|(x, _)| *x == c), "{c} is in the first round's object: {first:?}");
    }
    let mut pulled: BTreeMap<ChainId, Vec<Vec<u8>>> = BTreeMap::new();
    for o in inst.objects.values() {
        for (c, first, rows) in export::decode_object(o).unwrap().entries {
            let have = pulled.entry(c).or_default();
            assert_eq!(first.0, have.len() as u64 + 1, "{c}: no gap or repeat");
            have.extend(rows);
        }
    }
    assert_eq!(pulled, inst.logs, "every chain, led or not, exactly once");
}

/// A lifeboat's signed request takes its tail, snapshot, and pack pulls only: never the feed listing, which it reads
/// from the export bucket, and never a token revocation.
#[test]
fn a_lifeboat_request_takes_only_tail_snapshot_and_pack_routes() {
    let q = |p: &[(&str, &str)]| p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
    let may = |m: &str, path: &str, query: Vec<(String, String)>| Route::parse(m, path, &query).unwrap().lifeboat_may();
    let base = "/api/graftyard/v1/export";
    assert!(may("GET", &format!("{base}/chains/repo:5/entries"), q(&[("from", "1"), ("limit", "10")])));
    assert!(may("GET", &format!("{base}/chains/repo:5/snapshots"), q(&[("at", "9")])));
    let pack = format!("{base}/packs/{}", graftyard_git::pack::PackId(Ulid(0x0190_0000_0000_0000_0000_0000_0000_0001)));
    assert!(may("GET", &pack, q(&[("network", "5")])));
    assert!(!may("GET", &format!("{base}/batches"), q(&[])));
    assert!(!may("DELETE", &format!("{base}/token"), q(&[])));
}

/// Ruling 1 of M6-WP3 (`decisions/DECISIONS.md`, "`lifeboat-bundle`"): a chain's entries are served from index 1 for
/// as long as its archive objects exist, which is the chain's life (only a deleted repository's purge removes them),
/// so a lifeboat needs no snapshot install. A follower that installed a snapshot names no archive below its install;
/// the edge then finds the object whose range holds the cursor among the chain's archive keys, the narrowest of any
/// that overlap, and never another chain's.
#[test]
fn the_archive_holding_a_cursor_is_found_among_the_chains_keys() {
    let (c, other) = (repo(5), repo(6));
    let arch = |chain: ChainId, f: u64, t: u64| R2Key::Archive {
        chain,
        from: Index(f),
        to: Index(t),
        digest: Digest([f as u8; 32]),
    };
    let keys = vec![arch(c, 1, 40), arch(c, 41, 90), arch(c, 30, 60), arch(other, 1, 200)];
    let at = |i: u64| export::archive_holding(&keys, c, Index(i)).map(|(f, t, _)| (f.0, t.0));
    assert_eq!(at(1), Some((1, 40)));
    assert_eq!(at(35), Some((30, 60)), "the narrowest range that holds 35");
    assert_eq!(at(90), Some((41, 90)));
    assert_eq!(at(91), None, "past every range: no archive, so the page is the stored log's or 410");
    assert_eq!(export::archive_holding(&keys, repo(7), Index(1)), None, "another chain's objects never serve");
}

/// `feed-retired`: a repository whose leader reported its commit and was purged before a round read the tail. With
/// (b) the purge waits for the feed, which then carries every entry before the members go, and the report goes with
/// them; with the feed stalled past `PURGE_FEED_WAIT_MS` the purge goes ahead and names the entries never exported. A
/// round drops a report only on "not joined" from every source and the members' retire proof (a); a live chain whose
/// sources fail, or whose members are only briefly not joined, keeps its report and is read once they answer.
#[test]
fn a_purged_chain_reaches_the_feed_first_and_leaves_no_report_behind() {
    use export::{PURGE_FEED_WAIT_MS, PurgeWait, Unread, forgets, purge_wait, unexported_line};
    let mut inst = Install::default();
    let (purged, stalled, live) = (repo(1), repo(2), repo(3));
    let mut now = 1_700_000_000_000u64;
    for c in [purged, stalled, live] {
        inst.commit(c, 3);
    }
    inst.feed(now, false).unwrap();
    // The last entries, the retire's included: each leader reports, and no round has read them yet.
    for c in [purged, stalled, live] {
        inst.commit(c, 2);
    }
    let mut reports: BTreeMap<ChainId, Index> = inst.commits().into_iter().collect();
    let commit = reports[&purged];
    // (b): the purge's first call finds the feed behind and waits; a round carries the tail; the next call goes.
    let since = now;
    let cursor = |inst: &Install, c| Some(inst.state.cursors.get(c));
    assert_eq!(purge_wait(cursor(&inst, purged), commit, since, now), PurgeWait::Wait { left_ms: PURGE_FEED_WAIT_MS });
    assert_eq!(purge_wait(None, commit, since, now), PurgeWait::Go, "an install with no feed purges at once");
    now += export::FEED_EVERY_MS;
    let planned: Vec<(ChainId, Index)> = reports.iter().filter(|(c, _)| **c == purged).map(|(c, i)| (*c, *i)).collect();
    inst.feed_from(now, false, &planned).unwrap();
    assert_eq!(purge_wait(cursor(&inst, purged), commit, since, now), PurgeWait::Go);
    reports.remove(&purged);
    let carried: Vec<Vec<u8>> = inst
        .objects
        .values()
        .flat_map(|o| export::decode_object(o).unwrap().entries)
        .filter(|(c, ..)| *c == purged)
        .flat_map(|(_, _, rows)| rows)
        .collect();
    assert_eq!(carried, inst.logs[&purged], "every entry of the purged chain, its last ones included, was exported");
    // The cap: the feed never moves for `stalled`. Under the cap the purge waits; at the cap it goes and names 4..=5.
    let commit = reports[&stalled];
    let at = cursor(&inst, stalled);
    assert!(matches!(purge_wait(at, commit, since, since + PURGE_FEED_WAIT_MS - 1), PurgeWait::Wait { left_ms: 1 }));
    let PurgeWait::Late { from, to } = purge_wait(at, commit, since, since + PURGE_FEED_WAIT_MS) else {
        panic!("a stalled feed never holds a purge past the cap")
    };
    assert_eq!((from, to), (Index(4), Index(5)));
    assert_eq!(unexported_line(stalled, from, to), "export feed: repo:2 retired with entries 4..=5 never exported");
    reports.remove(&stalled);
    // (a): a round with `stalled`'s report still held (a purge from a build before this one). Every source not
    // joined and the members' retire proof drop it; anything less keeps it, as it keeps `live`'s.
    assert!(forgets(Unread::Unjoined, true));
    for (unread, retired) in [(Unread::Unjoined, false), (Unread::Missed, true), (Unread::Missed, false)] {
        assert!(!forgets(unread, retired), "{unread:?} retired={retired} keeps the report");
    }
    // The live chain's sources failed for a round (`Missed`): its report is kept, so the next round carries it.
    assert!(!forgets(Unread::Missed, false));
    now += export::FEED_EVERY_MS;
    let planned: Vec<(ChainId, Index)> = reports.iter().map(|(c, i)| (*c, *i)).collect();
    assert_eq!(planned, vec![(live, Index(5))], "only the live chain's report is left to plan");
    let b = inst.feed_from(now, false, &planned).unwrap();
    assert_eq!(b.chains, vec![(live, Index(5))]);
}
