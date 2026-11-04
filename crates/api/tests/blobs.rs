//! The `git lfs` and `gh release` session in `lfs/requests.json`, replayed in order through `Host::endpoint` and the
//! LFS, release, archive, and GraphQL handlers, over a repository chain and three in-memory site buckets in three
//! regions, as the edge drives them. Each reply's status must be the one the client got from the capture server. Each
//! body is written to `$GRAFTYARD_REPLIES` for `gh/validate.py`: REST against the pinned OpenAPI, GraphQL against the
//! pinned SDL, and LFS against git-lfs 3.8.0's own schemas.
//!
//! Then the pure halves of BF-25 (a blob one bucket holds is never acknowledged, and reads survive one unreadable
//! bucket) and BF-26 (archive bytes are the same after the cache is deleted and rebuilt, with real `git archive`).

mod common;

use std::collections::BTreeMap;

use common::*;
use futures_executor::block_on;
use graftyard_api::exec::Request;
use graftyard_api::graphql::{query, route};
use graftyard_api::release::{self, AssetGet};
use graftyard_api::view::Repo;
use graftyard_api::wire::{Host, Reply};
use graftyard_api::{lfs, rest};
use graftyard_git::blob::{ArchiveFormat, ArchiveRequest, Archiver, BlobStore, Source, archive, archive_key};
use graftyard_git::issue_ops::{IssueErr, IssueOp, IssueOut};
use graftyard_git::issues::{MergeMethod, RepoReleaseId, RepoSettings, Visibility};
use graftyard_git::mem::MemBucket;
use graftyard_git::repo::RepoEvent;
use graftyard_git::{Bucket, Copies, ObjectMeta, PutNew};
use graftyard_raft::{Acked, EntryBody, TierSet};
use graftyard_types::{
    AccountId, ChainId, DbId, Digest, NetworkId, Region, RequestId, Site, SiteId, SiteLink, SiteTable, Ulid,
};
use graftyard_wire::{Body, Endpoint};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use smallvec::{SmallVec, smallvec};

const CAPTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/lfs/requests.json");
const HOST: &str = "ghes.graftyard.test";

fn sites() -> SiteTable {
    let site = |id: u8, region| Site {
        id: SiteId(id),
        script: format!("s{id}"),
        account: "x".into(),
        region,
        jurisdiction: None,
        bucket: format!("b{id}"),
        release: None,
        link: SiteLink::Binding { member_class: "Member".into() },
        spare: false,
    };
    SiteTable(vec![site(0, Region::Enam), site(1, Region::Weur), site(2, Region::Apac)])
}

fn settings() -> RepoSettings {
    RepoSettings {
        owner: AccountId(DbId(OWNER)),
        enterprise: None,
        visibility: Visibility::Private,
        default_branch: name("refs/heads/main"),
        archived: false,
        description: None,
        homepage: None,
        topics: SmallVec::new(),
        has_issues: true,
        has_wiki: false,
        has_projects: false,
        merge_methods: smallvec![MergeMethod::Merge],
        delete_branch_on_merge: false,
        allow_auto_merge: false,
        allow_update_branch: false,
        check_retention_days: 90,
    }
}

fn sha(b: &[u8]) -> Digest {
    Digest(Sha256::digest(b).into())
}

/// `git archive` stand-in for the replay, whose commits are not real: bytes that depend only on what `git archive`
/// reads.
struct FakeGit;

impl Archiver for FakeGit {
    async fn archive(&self, _: NetworkId, commit: &str, prefix: &str, f: ArchiveFormat) -> Result<Vec<u8>, String> {
        Ok(Sha256::digest(format!("{commit}|{prefix}|{}", f.ext())).to_vec())
    }
}

fn keep(file: String, v: Value) {
    if let Ok(dir) = std::env::var("GRAFTYARD_REPLIES") {
        std::fs::write(format!("{dir}/{file}"), serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }
}

/// A REST reply for `validate.py`: the OpenAPI operation, the status, the JSON body (`null` for bytes or none), and a
/// redirect's `location`.
fn keep_rest(case: &str, method: &str, path: &str, r: &Reply) {
    let body = if let Body::Json(v) = &r.body { v.clone() } else { Value::Null };
    let v =
        json!({ "method": method, "path": path, "status": r.status, "body": body, "location": r.header("location") });
    keep(format!("{case}.rest.json"), v);
}

fn keep_lfs(case: &str, schema: &str, r: &Reply) {
    keep(format!("{case}.lfs.json"), json!({ "schema": schema, "body": r.value() }));
}

/// The edge's state for the replay: the chain, the site buckets, the warm site's bucket, the transfer hrefs each batch
/// handed out, and the bytes each LFS object and asset holds.
struct Edge<'a> {
    c: Chain,
    host: Host,
    store: BlobStore<'a, MemBucket>,
    warm: MemBucket,
    hrefs: BTreeMap<Digest, String>,
    /// The objects the last batch gave a transfer action to and the client has not transferred yet. The client made
    /// exactly the transfers its batch reply asked for, so each captured transfer must be one of these, and none may
    /// be left when the next batch comes.
    pending: std::collections::BTreeSet<Digest>,
    lfs_bytes: BTreeMap<Digest, Vec<u8>>,
    asset_bytes: BTreeMap<i64, Vec<u8>>,
    location: Option<String>,
    n: u128,
}

impl Edge<'_> {
    fn commit(&mut self, op: IssueOp) -> Result<Acked<IssueOut>, IssueErr> {
        self.c.commit(WRITER, op).map(ack)
    }

    fn meta(&mut self) -> ObjectMeta {
        self.n += 1;
        ObjectMeta { chain: ChainId::Repo(REPO), request: RequestId(Ulid(self.n)) }
    }
}

fn repo<'a>(c: &'a Chain, host: &'a Host, dir: &'a Dir) -> Repo<'a> {
    c.repo(0, host, dir)
}

/// The id gh used in a path, which it read from an earlier reply: the capture server numbered releases from 1 (v1,
/// then v2), assets from 100 (v1's app.tgz, then notes.txt), and locks from 1. This finds the same object by tag and
/// name in this chain, as gh found it in this server's replies.
fn release_id(r: &Repo<'_>, captured: &str) -> i64 {
    let tag = match captured {
        "1" => "v1",
        "2" => "v2",
        x => panic!("release {x}"),
    };
    r.release_by_tag(tag).unwrap().0.0.0
}

fn asset_id(r: &Repo<'_>, captured: &str) -> i64 {
    let name = match captured {
        "100" => "app.tgz",
        "101" => "notes.txt",
        x => panic!("asset {x}"),
    };
    let (rid, _) = r.release_by_tag("v1").unwrap();
    r.assets_of(rid).into_iter().find(|(_, a)| &*a.rec.name == name).unwrap().0.0.0
}

/// Answers one captured request as the edge would and returns its status.
fn serve(e: &mut Edge<'_>, case: &str, req: &Value) -> u16 {
    let method = req["method"].as_str().unwrap();
    let (path, q) = req["path"].as_str().unwrap().split_once('?').unwrap_or((req["path"].as_str().unwrap(), ""));
    let authorized = req["headers"].get("Authorization").is_some();
    let body = &req["body"];
    let host = e.host.clone();
    let dir = Dir::new(WRITER);
    match host.endpoint(HOST, path).unwrap() {
        Endpoint::Lfs { owner: "o", repo: "r", rest } => {
            if !authorized {
                return lfs::challenge().status;
            }
            let r = repo(&e.c, &host, &dir);
            match (method, rest) {
                ("POST", "objects/batch") => {
                    assert!(e.pending.is_empty(), "{case}: the last batch asked for transfers the client never made");
                    let reply = lfs::batch(&r, body, true, "Basic x");
                    keep_lfs(case, "http-batch-response-schema.json", &reply);
                    for o in reply.value()["objects"].as_array().unwrap() {
                        let oid = Digest::parse_hex(o["oid"].as_str().unwrap()).unwrap();
                        let href = o.pointer("/actions/upload/href").or(o.pointer("/actions/download/href"));
                        if let Some(h) = href.and_then(Value::as_str) {
                            e.hrefs.insert(oid, h.to_owned());
                            e.pending.insert(oid);
                        }
                    }
                    reply.status
                }
                ("POST", "objects/verify") => {
                    let (oid, size) = lfs::verify_request(body).unwrap();
                    match block_on(e.store.confirm(REPO, oid, size)) {
                        Ok(d) => lfs::verified(&e.commit(lfs::plan_verify(d)).unwrap()).status,
                        Err(_) => lfs::not_verified().status,
                    }
                }
                ("POST", "locks/verify") => {
                    let reply = lfs::verify_locks(&r, Some(AccountId(DbId(WRITER))), true);
                    keep_lfs(case, "http-lock-verify-response-schema.json", &reply);
                    reply.status
                }
                ("GET", "locks") => {
                    let reply = lfs::list_locks(&r, q);
                    keep_lfs(case, "http-lock-list-response-schema.json", &reply);
                    reply.status
                }
                ("POST", "locks") => {
                    let op = lfs::plan_lock(&r, body).unwrap();
                    let a = e.commit(op).unwrap();
                    let reply = lfs::locked(&a, &repo(&e.c, &host, &dir));
                    keep_lfs(case, "http-lock-create-response-schema.json", &reply);
                    reply.status
                }
                ("POST", "locks/1/unlock") => {
                    let (id, _) = r.locks().into_iter().find(|(_, l)| &*l.path == "a.bin").unwrap();
                    let (op, lock) = lfs::plan_unlock(&r, &id.0.0.to_string(), body).unwrap();
                    let reply = lfs::unlocked(&e.commit(op).unwrap(), lock);
                    keep_lfs(case, "http-lock-create-response-schema.json", &reply);
                    reply.status
                }
                _ => panic!("an LFS route the capture did not send: {method} {rest}"),
            }
        }
        // The capture server's hrefs named only the oid; this edge's name the repository too. The client follows the
        // href its batch reply gave, so the replay does.
        Endpoint::Objects(oid) => {
            let oid = Digest::parse_hex(oid).unwrap();
            assert!(e.pending.remove(&oid), "{case}: the client transferred an object its batch reply did not ask for");
            let href = e.hrefs.get(&oid).expect("a batch reply named this object").clone();
            let Some(Endpoint::Objects(p)) =
                host.endpoint(HOST, href.strip_prefix(&format!("https://{HOST}")).unwrap())
            else {
                panic!("{href}")
            };
            assert_eq!(lfs::transfer_path(p), Some((REPO.0.0, oid)));
            match method {
                "PUT" => {
                    let bytes = e.lfs_bytes[&oid].clone();
                    let meta = e.meta();
                    match block_on(e.store.place(REPO, &bytes, Some((oid, bytes.len() as u64)), meta)) {
                        Ok(d) => lfs::uploaded(&d).status,
                        Err(x) => lfs::upload_failed(&x).status,
                    }
                }
                "GET" => {
                    let row = lfs::download(&repo(&e.c, &host, &dir), &oid).unwrap();
                    let got = block_on(e.store.read(REPO, &row.blob)).unwrap().unwrap();
                    assert_eq!(got.bytes, e.lfs_bytes[&oid]);
                    lfs::download_reply(got.bytes).status
                }
                m => panic!("{m}"),
            }
        }
        Endpoint::Graphql => {
            let q = body["query"].as_str().unwrap();
            let request = Request::parse(q, body["variables"].as_object().cloned(), None).unwrap();
            let r = repo(&e.c, &host, &dir);
            let reply = query(&request, route(&request).unwrap().map(|_| &r));
            assert!(reply.get("errors").is_none(), "{case}: {reply}");
            keep(format!("{case}.json"), json!({ "query": q, "variables": body["variables"], "reply": reply }));
            200
        }
        Endpoint::Uploads(p) => {
            let captured = p.strip_prefix("/repos/o/r/releases/").unwrap().strip_suffix("/assets").unwrap();
            let r = repo(&e.c, &host, &dir);
            let rid = release_id(&r, captured);
            let u = match release::check_upload(&r, rid, q, req["headers"]["Content-Type"].as_str()) {
                Ok(u) => u,
                Err(x) => return x.status,
            };
            let bytes = format!("{case} bytes\n").into_bytes();
            let meta = e.meta();
            let placed = match block_on(e.store.place(REPO, &bytes, None, meta)) {
                Ok(d) => d,
                Err(x) => return release::upload_failed(&x).status,
            };
            let a = e.commit(release::plan_upload(u, placed)).unwrap();
            let reply = release::created(&a, &repo(&e.c, &host, &dir));
            e.asset_bytes.insert(reply.value()["id"].as_i64().unwrap(), bytes);
            keep_rest(case, "POST", "/repos/{owner}/{repo}/releases/{release_id}/assets", &reply);
            reply.status
        }
        Endpoint::Codeload { owner: "o", repo: "r", rest } => {
            assert_eq!(e.location.take().as_deref(), Some(&*format!("https://{HOST}{path}")), "the 302 pointed here");
            let r = repo(&e.c, &host, &dir);
            let c = release::codeload(&r, rest).unwrap();
            let ar = ArchiveRequest {
                network: NetworkId(REPO.0),
                commit: &c.commit.hex(),
                prefix: &c.prefix,
                format: c.format,
            };
            let (bytes, _) = block_on(archive(&e.warm, &FakeGit, &ar)).unwrap();
            release::archive_reply(&c, bytes).status
        }
        Endpoint::Rest(p) => {
            let segs: Vec<&str> = p.strip_prefix("/repos/o/r/").unwrap().split('/').collect();
            let r = repo(&e.c, &host, &dir);
            let (reply, op) = match (method, segs.as_slice()) {
                ("POST", ["releases"]) => {
                    let a = e.commit(release::plan_create(&r, body).unwrap()).unwrap();
                    (release::created(&a, &repo(&e.c, &host, &dir)), "/repos/{owner}/{repo}/releases")
                }
                ("GET" | "HEAD", ["releases", "tags", tag]) => {
                    (release::get_by_tag(&r, tag), "/repos/{owner}/{repo}/releases/tags/{tag}")
                }
                ("PATCH", ["releases", id]) => {
                    let op = release::plan_update(&r, release_id(&r, id), body).unwrap();
                    let a = e.commit(op).unwrap();
                    (release::updated(&a, &repo(&e.c, &host, &dir)), "/repos/{owner}/{repo}/releases/{release_id}")
                }
                ("DELETE", ["releases", "assets", id]) => {
                    let a = e.commit(release::plan_delete_asset(&r, asset_id(&r, id)).unwrap()).unwrap();
                    (release::deleted(&a), "/repos/{owner}/{repo}/releases/assets/{asset_id}")
                }
                ("DELETE", ["releases", id]) => {
                    let a = e.commit(release::plan_delete(&r, release_id(&r, id)).unwrap()).unwrap();
                    (release::deleted(&a), "/repos/{owner}/{repo}/releases/{release_id}")
                }
                ("GET", ["releases", "assets", id]) => {
                    let id = asset_id(&r, id);
                    match release::get_asset(&r, id, req["headers"]["Accept"].as_str()) {
                        AssetGet::Reply(x) => (x, "/repos/{owner}/{repo}/releases/assets/{asset_id}"),
                        AssetGet::Bytes(row) => {
                            let got = block_on(e.store.read(REPO, &row.rec.blob)).unwrap().unwrap();
                            assert_eq!(got.bytes, e.asset_bytes[&id], "the bytes the upload placed");
                            return release::asset_bytes(&row, got.bytes).status;
                        }
                    }
                }
                ("GET", [kind @ ("tarball" | "zipball"), reference]) => {
                    let f = if *kind == "tarball" { ArchiveFormat::TarGz } else { ArchiveFormat::Zip };
                    let reply = release::archive_redirect(&r, f, reference);
                    e.location = reply.header("location").map(str::to_owned);
                    (
                        reply,
                        if f == ArchiveFormat::TarGz {
                            "/repos/{owner}/{repo}/tarball/{ref}"
                        } else {
                            "/repos/{owner}/{repo}/zipball/{ref}"
                        },
                    )
                }
                ("DELETE", ["git", "refs", reference]) => {
                    // `--cleanup-tag`: the edge commits the deletion as a push, M1's write path.
                    let u = rest::plan_delete_ref(&r, reference).unwrap();
                    let ev = RepoEvent::Refs { push: RequestId(Ulid(77)), actor: None, updates: vec![u], pack: None };
                    e.c.entry(EntryBody::Events(vec![ev]), TierSet::CORE);
                    (Reply::empty(204), "/repos/{owner}/{repo}/git/refs/{ref}")
                }
                _ => panic!("a REST route the capture did not send: {method} {p}"),
            };
            if method != "HEAD" {
                keep_rest(case, method, op, &reply);
            }
            reply.status
        }
        other => panic!("{other:?}"),
    }
}

/// The capture keeps each LFS object's oid and size but not its bytes, so the replay makes bytes of each size and
/// renames every captured oid to theirs.
fn with_stand_in_objects(capture: &str) -> (Vec<Value>, BTreeMap<Digest, Vec<u8>>) {
    let reqs: Vec<Value> = serde_json::from_str(capture).unwrap();
    let mut objects = BTreeMap::new();
    for r in &reqs {
        for o in r.pointer("/body/objects").and_then(Value::as_array).into_iter().flatten() {
            objects.insert(o["oid"].as_str().unwrap().to_owned(), o["size"].as_u64().unwrap() as usize);
        }
    }
    let mut text = capture.to_owned();
    let mut bytes = BTreeMap::new();
    for (old, size) in objects {
        let b: Vec<u8> = (0..size).map(|i| old.as_bytes()[i % 64] ^ (i % 251) as u8).collect();
        let new = sha(&b);
        text = text.replace(&old, &new.hex());
        bytes.insert(new, b);
    }
    (serde_json::from_str(&text).unwrap(), bytes)
}

fn chain() -> Chain {
    let mut c = Chain::new(3);
    c.push("refs/heads/main", oid(1));
    c.commit(OWNER, IssueOp::Settings(settings())).unwrap();
    c
}

#[test]
fn the_captured_git_lfs_and_gh_release_session_gets_the_statuses_the_clients_got() {
    let (reqs, lfs_bytes) = with_stand_in_objects(&std::fs::read_to_string(CAPTURE).unwrap());
    let sites = sites();
    let bk = [MemBucket::default(), MemBucket::default(), MemBucket::default()];
    let buckets = [(SiteId(0), &bk[0]), (SiteId(1), &bk[1]), (SiteId(2), &bk[2])];
    let mut e = Edge {
        c: chain(),
        host: Host::new(HOST, false).unwrap(),
        store: BlobStore { sites: &sites, buckets: &buckets, copies: Copies::TwoRegions },
        warm: MemBucket::default(),
        hrefs: BTreeMap::new(),
        pending: Default::default(),
        lfs_bytes,
        asset_bytes: BTreeMap::new(),
        location: None,
        n: 0,
    };
    for (i, req) in reqs.iter().enumerate() {
        let case = format!("{i:02}-{}", req["command"].as_str().unwrap().replace([' ', '=', '*', '.'], "_"));
        let got = serve(&mut e, &case, req);
        let want = req["capture_status"].as_u64().unwrap() as u16;
        assert_eq!(got, want, "{case}: {} {}", req["method"], req["path"]);
    }
    assert_eq!(reqs.len(), 65);
    assert!(e.pending.is_empty(), "the last batch asked for transfers the client never made");
    let dir = Dir::new(WRITER);
    let r = repo(&e.c, &e.host, &dir);
    let (v1, rel) = r.release_by_tag("v1").unwrap();
    assert_eq!((&*rel.rec.name, rel.rec.draft), ("T", false), "`gh release edit --title T`");
    let names: Vec<String> = r.assets_of(v1).into_iter().map(|(_, a)| a.rec.name.to_string()).collect();
    assert_eq!(names, ["app.tgz"], "notes.txt deleted, app.tgz clobbered once");
    assert!(r.release_by_tag("v2").is_none() && r.ref_value(&name("refs/tags/v2")).is_none(), "--cleanup-tag");
    assert_eq!(r.locks().len(), 0);
    assert_eq!(e.lfs_bytes.keys().filter(|o| r.lfs_object(o).is_some()).count(), 2, "both objects verified");
}

fn store<'a>(sites: &'a SiteTable, buckets: &'a [(SiteId, &'a MemBucket)]) -> BlobStore<'a, MemBucket> {
    BlobStore { sites, buckets, copies: Copies::TwoRegions }
}

fn meta() -> ObjectMeta {
    ObjectMeta { chain: ChainId::Repo(REPO), request: RequestId(Ulid(1)) }
}

fn v1(c: &mut Chain) -> RepoReleaseId {
    let (host, dir) = (Host::new(HOST, false).unwrap(), Dir::new(WRITER));
    let op = release::plan_create(&c.repo(0, &host, &dir), &json!({ "tag_name": "v1" })).unwrap();
    match c.commit(WRITER, op).unwrap() {
        IssueOut::Id(id) => RepoReleaseId(id),
        o => panic!("{o:?}"),
    }
}

/// BF-25, pure half: neither the upload's 201 nor `verify`'s 200 exists while one bucket holds the bytes, and the
/// fold names no such blob.
#[test]
fn bf25_a_blob_one_bucket_holds_is_never_acknowledged() {
    let sites = sites();
    let bk = [MemBucket::default(), MemBucket::default(), MemBucket::default()];
    let buckets = [(SiteId(0), &bk[0]), (SiteId(1), &bk[1]), (SiteId(2), &bk[2])];
    let s = store(&sites, &buckets);
    let host = Host::new(HOST, false).unwrap();
    let dir = Dir::new(WRITER);
    let mut c = chain();
    let rid = v1(&mut c);
    // An upload while two buckets refuse puts: `place` returns no `DurableBlob`, so no `PutAsset` can be built, and the
    // reply is a 503 with nothing committed.
    bk[1].fail_puts(true);
    bk[2].fail_puts(true);
    let up = release::check_upload(&c.repo(0, &host, &dir), rid.0.0, "name=app.tgz", Some("application/gzip")).unwrap();
    let err = block_on(s.place(REPO, b"asset", None, meta())).unwrap_err();
    assert_eq!(release::upload_failed(&err).status, 503);
    assert!(c.repo(0, &host, &dir).assets_of(rid).is_empty());
    // LFS: the transfer `PUT` fails the same way, and `verify` finds one bucket holding it and answers 404, so git-lfs
    // uploads again. A download batch reports the object missing.
    let obj = b"lfs object".to_vec();
    let oid = sha(&obj);
    let put = block_on(s.place(REPO, &obj, Some((oid, obj.len() as u64)), meta()));
    assert_eq!(lfs::upload_failed(&put.unwrap_err()).status, 503);
    let only = block_on(bk[0].put_new(&graftyard_git::R2Key::Blob { repo: REPO, oid }.to_string(), &obj, oid, &[]));
    assert!(matches!(only, Ok(PutNew::Created | PutNew::Exists(_))));
    assert!(block_on(s.confirm(REPO, oid, obj.len() as u64)).is_err(), "one bucket is not durable");
    assert_eq!(lfs::not_verified().status, 404);
    let batch = json!({ "operation": "download", "objects": [{ "oid": oid.hex(), "size": obj.len() }] });
    let reply = lfs::batch(&c.repo(0, &host, &dir), &batch, true, "Basic x");
    assert_eq!(reply.value()["objects"][0]["error"]["code"], 404);
    // Once a second region's bucket holds it, `verify` commits and answers 200, and the 201 of an upload follows.
    bk[1].fail_puts(false);
    let d = block_on(s.place(REPO, &obj, Some((oid, obj.len() as u64)), meta())).unwrap();
    assert!(d.blob().sites.len() >= 2);
    let d = block_on(s.confirm(REPO, oid, obj.len() as u64)).unwrap();
    c.commit(WRITER, lfs::plan_verify(d)).unwrap();
    assert!(c.repo(0, &host, &dir).lfs_object(&oid).is_some());
    let asset = block_on(s.place(REPO, b"asset", None, meta())).unwrap();
    let out = c.commit(WRITER, release::plan_upload(up, asset)).unwrap();
    assert_eq!(release::created(&ack(out), &c.repo(0, &host, &dir)).status, 201);
    // Every blob a committed row names is in at least two buckets of two regions.
    let r = c.repo(0, &host, &dir);
    for blob in r.assets_of(rid).iter().map(|(_, a)| a.rec.blob).chain(r.lfs_object(&oid).map(|l| l.blob)) {
        let key = graftyard_git::R2Key::Blob { repo: REPO, oid: blob.oid }.to_string();
        let held = bk.iter().filter(|b| block_on(b.get(&key)).unwrap().is_some()).count();
        assert!(held >= 2, "{held} buckets hold {key}");
    }
}

/// BF-25, pure half: with any one bucket unreadable or corrupt, every asset and LFS download still succeeds.
#[test]
fn bf25_reads_succeed_with_any_one_bucket_unreadable() {
    let sites = sites();
    let bk = [MemBucket::default(), MemBucket::default(), MemBucket::default()];
    let buckets = [(SiteId(0), &bk[0]), (SiteId(1), &bk[1]), (SiteId(2), &bk[2])];
    let s = store(&sites, &buckets);
    let host = Host::new(HOST, false).unwrap();
    let dir = Dir::new(WRITER);
    let mut c = chain();
    let rid = v1(&mut c);
    let mut blobs = Vec::new();
    for i in 0..10u8 {
        let bytes = vec![i; 1000 + usize::from(i)];
        let up = release::check_upload(&c.repo(0, &host, &dir), rid.0.0, &format!("name=a{i}"), None).unwrap();
        let d = block_on(s.place(REPO, &bytes, None, meta())).unwrap();
        c.commit(WRITER, release::plan_upload(up, d)).unwrap();
        let lfs_bytes = vec![i ^ 0x55; 2000];
        let d = block_on(s.place(REPO, &lfs_bytes, Some((sha(&lfs_bytes), 2000)), meta())).unwrap();
        c.commit(WRITER, lfs::plan_verify(d)).unwrap();
        blobs.push((bytes, lfs_bytes));
    }
    // The third put may still be running when `place` returns; let it land so every bucket is a candidate to fail.
    for (bytes, lfs_bytes) in &blobs {
        for b in [bytes, lfs_bytes] {
            for x in &bk {
                let _ = block_on(x.put_new(
                    &graftyard_git::R2Key::Blob { repo: REPO, oid: sha(b) }.to_string(),
                    b,
                    sha(b),
                    &[],
                ));
            }
        }
    }
    let mut reads = 0;
    for (down, unreadable) in bk.iter().enumerate() {
        unreadable.fail_gets(true);
        let r = c.repo(0, &host, &dir);
        for (id, a) in r.assets_of(rid) {
            let AssetGet::Bytes(row) = release::get_asset(&r, id.0.0, Some("application/octet-stream")) else {
                panic!()
            };
            let got = block_on(s.read(REPO, &row.rec.blob)).unwrap().unwrap();
            assert_ne!(got.site, SiteId(down as u8));
            assert_eq!(sha(&got.bytes), a.rec.blob.oid);
            reads += 1;
        }
        for (_, lfs_bytes) in &blobs {
            let row = lfs::download(&r, &sha(lfs_bytes)).unwrap();
            assert_eq!(&block_on(s.read(REPO, &row.blob)).unwrap().unwrap().bytes, lfs_bytes);
            reads += 1;
        }
        unreadable.fail_gets(false);
    }
    assert_eq!(reads, 60);
    // A corrupt copy is skipped, too.
    let (_, a) = c.repo(0, &host, &dir).assets_of(rid).remove(0);
    let key = graftyard_git::R2Key::Blob { repo: REPO, oid: a.rec.blob.oid }.to_string();
    block_on(bk[0].delete(std::slice::from_ref(&key))).unwrap();
    block_on(bk[0].put_new(&key, b"rot", sha(b"rot"), &[])).unwrap();
    let got = block_on(s.read(REPO, &a.rec.blob)).unwrap().unwrap();
    assert_eq!(sha(&got.bytes), a.rec.blob.oid);
}

/// Real `git archive` in a throwaway repository with a fixed commit time, removed on drop.
struct Git(std::path::PathBuf);

impl Drop for Git {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Git {
    fn run(&self, args: &[&str]) -> Vec<u8> {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&self.0)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        out.stdout
    }
}

impl Archiver for Git {
    async fn archive(&self, _: NetworkId, commit: &str, prefix: &str, f: ArchiveFormat) -> Result<Vec<u8>, String> {
        let format = match f {
            ArchiveFormat::TarGz => "tar.gz",
            ArchiveFormat::Zip => "zip",
        };
        Ok(self.run(&["archive", &format!("--format={format}"), &format!("--prefix={prefix}"), commit]))
    }
}

/// BF-26, pure half: `gh release download --archive=tar.gz`'s bytes, then every `ArchiveCache` object deleted, then
/// the same download: the bytes match, for both formats.
#[test]
fn bf26_archive_bytes_match_after_the_cache_is_deleted() {
    let dir = std::env::temp_dir().join(format!("graftyard-bf26-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let git = Git(dir.clone());
    git.run(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    git.run(&["add", "."]);
    git.run(&["-c", "user.name=a", "-c", "user.email=a@b", "commit", "-q", "-m", "one"]);
    let head = String::from_utf8(git.run(&["rev-parse", "HEAD"])).unwrap().trim().to_owned();
    let commit = graftyard_git::refs::ObjectId::parse_hex(head.as_bytes()).unwrap();

    let mut c = chain();
    c.push("refs/tags/v1", commit);
    let host = Host::new(HOST, false).unwrap();
    let d = Dir::new(WRITER);
    let warm = MemBucket::default();
    let download = |rest: &str| {
        let r = c.repo(0, &host, &d);
        let cl = release::codeload(&r, rest).unwrap();
        let req = ArchiveRequest {
            network: NetworkId(REPO.0),
            commit: &cl.commit.hex(),
            prefix: &cl.prefix,
            format: cl.format,
        };
        let key = archive_key(&req).to_string();
        let (bytes, src) = block_on(archive(&warm, &git, &req)).unwrap();
        (release::archive_reply(&cl, bytes), src, key)
    };
    for fmt in ["legacy.tar.gz", "legacy.zip"] {
        let path = format!("{fmt}/refs/tags/v1");
        let redirect = release::archive_redirect(&c.repo(0, &host, &d), ArchiveFormat::parse(&fmt[7..]).unwrap(), "v1");
        assert!(redirect.header("location").unwrap().ends_with(&format!("/_codeload/o/r/{path}")));
        let (first, src, key) = download(&path);
        assert_eq!(src, Source::Built);
        assert!(key.starts_with("archive-cache/7/"), "{key}");
        let (cached, src, _) = download(&path);
        assert_eq!((src, &cached.body), (Source::Cache, &first.body));
        block_on(warm.delete(&warm.keys())).unwrap();
        let (rebuilt, src, _) = download(&path);
        assert_eq!(src, Source::Built);
        assert_eq!(rebuilt.body, first.body, "{fmt}: the bytes match the earlier download");
        assert_eq!(rebuilt.header("content-disposition"), Some(&*format!("attachment; filename=o-r-v1.{}", &fmt[7..])));
        let Body::Bytes(b) = &first.body else { panic!() };
        assert!(b.len() > 100);
    }
}

/// `meta-endpoint`, `rest:meta`, the root document, `/status`, and `rest:rate-limit`, which no capture here sent (gh's
/// `GET /meta` is in `gh/rest.txt`).
#[test]
fn meta_root_status_and_rate_limit_answer_as_ghes() {
    let host = Host::new(HOST, false).unwrap();
    let m = rest::meta();
    assert_eq!(m.value()["installed_version"], "3.22.0");
    keep_rest("meta", "GET", "/meta", &m);
    let root = rest::root(&host);
    assert_eq!(root.value()["rate_limit_url"], format!("https://{HOST}/api/v3/rate_limit"));
    keep_rest("root", "GET", "/", &root);
    let Body::Bytes(b) = rest::status().body else { panic!() };
    assert!(b.starts_with(b"GitHub lives!"));
    let off = rest::rate_limit(&graftyard_wire::RateLimits::Off, graftyard_wire::ApiVersion::V2022_11_28);
    assert_eq!(off.status, 404);
    keep_rest("rate_limit_off", "GET", "/rate_limit", &off);
    let w = graftyard_wire::Window { limit: 5000, used: 1, reset: 1_800_000_000 };
    // The pinned `rate-limit-overview` requires `core` and `search`; managed installs count both.
    let on = graftyard_wire::RateLimits::On(vec![("core", w), ("search", w), ("graphql", w)]);
    let on = rest::rate_limit(&on, graftyard_wire::ApiVersion::V2022_11_28);
    assert_eq!(on.status, 200);
    keep_rest("rate_limit_on", "GET", "/rate_limit", &on);
}

#[test]
fn a_release_s_replies_follow_the_pinned_schema_for_drafts_and_assets() {
    let mut c = chain();
    let host = Host::new(HOST, false).unwrap();
    let d = Dir::new(WRITER);
    let r = c.repo(0, &host, &d);
    let draft = release::plan_create(&r, &json!({ "tag_name": "v9", "draft": true, "body": "notes" })).unwrap();
    let out = c.commit(WRITER, draft).unwrap();
    let reply = release::created(&ack(out.clone()), &c.repo(0, &host, &d));
    assert_eq!(reply.status, 201);
    assert!(reply.value()["tarball_url"].is_null() && reply.value()["published_at"].is_null());
    keep_rest("draft", "POST", "/repos/{owner}/{repo}/releases", &reply);
    let IssueOut::Id(id) = out else { panic!() };
    assert_eq!(release::get_by_tag(&c.repo(0, &host, &d), "v9").status, 404, "a draft is not found by tag");
    assert_eq!(release::get(&c.repo(0, &host, &d), id.0).status, 200);
    keep_rest(
        "draft_by_id",
        "GET",
        "/repos/{owner}/{repo}/releases/{release_id}",
        &release::get(&c.repo(0, &host, &d), id.0),
    );
    // A second release on one tag, and an upload of a name the release has, are 422 validation errors.
    let r = c.repo(0, &host, &d);
    let again = c.commit(WRITER, release::plan_create(&r, &json!({ "tag_name": "v9" })).unwrap()).unwrap_err();
    let reply = release::refused(&again);
    assert_eq!(reply.status, 422);
    keep_rest("exists", "POST", "/repos/{owner}/{repo}/releases", &reply);
    let missing = release::plan_create(&c.repo(0, &host, &d), &json!({})).unwrap_err();
    assert_eq!(missing.status, 422);
    keep_rest("missing_tag", "POST", "/repos/{owner}/{repo}/releases", &missing);
    let unknown = release::get_asset(&c.repo(0, &host, &d), 999, None);
    let AssetGet::Reply(nf) = unknown else { panic!() };
    assert_eq!(nf.status, 404);
    keep_rest("asset_404", "GET", "/repos/{owner}/{repo}/releases/assets/{asset_id}", &nf);
}
