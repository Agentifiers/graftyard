//! `POST` and `GET /repos/{owner}/{repo}/forks` (`design/SHARED.md`, the `rest:repos` row, "forks in their network";
//! "Multi-chain operations", Fork; `build/BUILD-PLAN.md`, AM3-WP2a).
//!
//! A fork's commit point is `CreateFork` on the fork owner's `Authz` chain, under the caller's credential (Authz
//! 0x12.16): the leader refuses a caller who does not administer that owner and a taken name, and records the parent
//! and the network with the name's `Live` (`AuthzEvent::ForkCreate`). The parent was read here through the read gate,
//! so a caller who cannot read it is answered 404, and its network is the one this request fixed (`Repo::network`).
//! The edge then asks the fork owner's `Authz` leader to finish the fork in band (`/finish`, the site's `Audit` batch
//! for that one item, as a transfer does): `RepoOwner` with the network (Names 0x10.4), `ForkAdded` on the network
//! chain (0x21.1), and the fork chain's first touch (repository 32.34), which writes `Genesis`, the first settings
//! and `Created`, and the parent's branches and tags with no pack, since the packs are the network's. It answers 202
//! with the fork, as GHES does, before those finish: GitHub creates a fork asynchronously too. An edge that dies after
//! the commit point leaves the rest to the `Audit` consumer's cursor on that chain.
//!
//! Sent only once `FormatsEnable` enabled every format the finish writes.

use graftyard_api::repos::{DOC_CREATE, defaults, taken};
use graftyard_identity::{AccountKind, Actor, RepoName};
use graftyard_raft::Format;
use graftyard_types::{AccountId, ChainId, NetworkId, RepoId};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;
use crate::api::Read;

const DOC_FORK: &str = "rest/repos/forks#create-a-fork";

/// Every format a fork's commit point and its finish write.
const NEEDS: [Format; 4] = [
    graftyard_identity::AUTHZ_FORMAT_V16,
    graftyard_identity::NAMES_FORMAT_V4,
    graftyard_git::network::NETWORK_FORMAT,
    graftyard_git::issue_ops::call::REPO_FORMAT_FORKS,
];

/// The first format of `NEEDS` that `FormatsEnable` has not enabled on this install.
fn missing(c: &Ctx<'_>) -> Option<Format> {
    NEEDS.iter().find(|f| !c.cfg.enabled.contains(f)).copied()
}

/// Whether forks are enabled on this install: every format of `NEEDS`. Until then no network chain has a fork, so
/// `GET /repos/{o}/{r}` reads nothing more.
pub(crate) fn on(c: &Ctx<'_>) -> bool {
    missing(c).is_none()
}

fn enabled(c: &Ctx<'_>) -> std::result::Result<(), Reply> {
    match missing(c) {
        None => Ok(()),
        Some(f) => Err(crate::api::unavailable(&format!(
            "a fork needs record format {}.{}, which FormatsEnable has not enabled on this install",
            f.family, f.version
        ))),
    }
}

/// `POST` or `GET .../forks` on a repository the read gate let the caller read; `None` for any other path.
pub async fn repo_route(
    c: &Ctx<'_>,
    req: &mut Request,
    rd: &Read,
    who: Option<&graftyard_identity::Principal>,
    tail: &[&str],
) -> Result<Option<Reply>> {
    if tail != ["forks"] {
        return Ok(None);
    }
    if let Err(r) = enabled(c) {
        return Ok(Some(r));
    }
    Ok(Some(match req.method() {
        Method::Post => {
            let body: Value = req.json().await.unwrap_or(Value::Null);
            create(c, rd, who, &body).await?
        }
        Method::Get => list(c, rd).await?,
        _ => Reply::not_found(),
    }))
}

/// The fork's owner and name from the body: `organization` or the caller, and `name` or the parent's.
async fn target(
    c: &Ctx<'_>,
    rd: &Read,
    me: AccountId,
    body: &Value,
) -> Result<std::result::Result<(AccountId, String, RepoName, bool), Reply>> {
    let (owner, login, org) = match body.get("organization").and_then(Value::as_str) {
        Some(o) => match crate::ident::account_of(c, o).await.map_err(Error::RustError)? {
            Some((a, AccountKind::Org)) => (a, o.to_owned(), true),
            _ => return Ok(Err(Reply::error(422, "Invalid organization", DOC_FORK))),
        },
        None => (me, crate::ident::login(c, me).await.unwrap_or_default(), false),
    };
    let name = body.get("name").and_then(Value::as_str).unwrap_or(&rd.at.name);
    let Ok(name) = RepoName::parse(name) else {
        let e = graftyard_wire::FieldError { resource: "Repository", field: "name", code: "invalid" };
        return Ok(Err(Reply::json(422, graftyard_wire::validation_error("Validation Failed", DOC_FORK, &[e]))));
    };
    Ok(Ok((owner, login, name, org)))
}

async fn create(c: &Ctx<'_>, rd: &Read, who: Option<&graftyard_identity::Principal>, body: &Value) -> Result<Reply> {
    let Some(who) = who else { return Ok(Reply::error(401, "Requires authentication", DOC_FORK)) };
    let Actor::User(me) = who.actor else {
        return Ok(Reply::error(403, "Resource not accessible by integration", DOC_FORK));
    };
    let (owner, login, name, org) = match target(c, rd, me.0, body).await? {
        Ok(t) => t,
        Err(r) => return Ok(r),
    };
    // A fork keeps the parent's settings but its own owner and no enterprise of the parent's; the finisher sets the
    // owner and enterprise its owner's `Authz` names (`lifecycle::init_settings`).
    let mut settings: graftyard_git::issues::RepoSettings =
        graftyard_git::tiers::get_rec(&rd.core, "settings", &[]).unwrap_or_else(|| defaults(owner, None));
    settings.owner = owner;
    settings.enterprise = None;
    let network = rd.repo.network();
    let init = serde_json::to_string(&settings)?;
    let cmd = json!({ "cred": who.credential_shard(), "op": create_op(&name, &init, rd.repo.id, network) });
    let done = match crate::ident::commit_owner(c, ChainId::Authz(owner), owner, cmd).await {
        Ok(d) => d,
        Err(x) => return Ok(x),
    };
    match crate::ident::refused(&done).as_deref() {
        None => {}
        Some("Taken") => return Ok(taken(DOC_CREATE)),
        Some("Forbidden") => {
            return Ok(Reply::error(
                403,
                "You do not have permission to create repositories on this account",
                DOC_FORK,
            ));
        }
        Some("Unauthorized") => return Ok(Reply::error(401, "Bad credentials", "rest")),
        Some(e) => return Ok(crate::api::unavailable(&format!("the fork did not commit: {e}"))),
    }
    let Some(id) = done.pointer("/ok/out/Repo/repo").and_then(Value::as_i64) else {
        return Ok(crate::api::unavailable(&format!("CreateFork answered {done}")));
    };
    let repo = RepoId(graftyard_types::DbId(id));
    // Test installs: the edge dies right after the commit point, so only the `Audit` consumer can finish it.
    #[cfg(feature = "test-faults")]
    let killed = c.kill.as_deref() == Some("after_fork");
    #[cfg(not(feature = "test-faults"))]
    let killed = false;
    if !killed {
        let owed = owed(owner, &name, repo, &init, rd.repo.id, network, crate::now());
        if let Err(e) = crate::transfer::finish(c, owner, &owed).await {
            console_error!("fork {id}: in-band finish not started: {e}");
        }
    }
    Ok(Reply::json(202, fork_json(c, rd, id, owner, &login, name.as_str(), org).await))
}

/// `CreateFork`'s op as the edge sends it to the owner's `Authz` leader.
fn create_op(name: &RepoName, init: &str, parent: RepoId, network: NetworkId) -> Value {
    json!({ "CreateFork": { "name": name.as_str(), "init": init, "parent": parent, "network": network } })
}

/// The in-band finish's item (`graftyard_identity::audit::Owed::Fork`), as the commit point's `ForkCreate` names it.
fn owed(
    owner: AccountId,
    name: &RepoName,
    repo: RepoId,
    init: &str,
    parent: RepoId,
    network: NetworkId,
    at: graftyard_types::Timestamp,
) -> Value {
    json!({ "owed": { "Fork": { "owner": owner, "key": name.key(), "name": name.as_str(), "repo": repo,
        "init": init, "parent": parent, "network": network, "at": at } } })
}

/// The fork as GHES's 202 renders it: `fork: true`, `parent` the repository forked and `source` its network's root.
/// `gh repo fork` reads `full_name`, `clone_url` and `parent`.
async fn fork_json(c: &Ctx<'_>, rd: &Read, id: i64, owner: AccountId, login: &str, name: &str, org: bool) -> Value {
    let mut v = crate::transfer::minimal_repository(c, id, owner, login, name, org);
    let parent = parent_json(c, rd);
    let source = source_json(c, rd, &parent).await;
    let m = v.as_object_mut().expect("object");
    m.insert("fork".into(), json!(true));
    m.insert("source".into(), source);
    m.insert("parent".into(), parent);
    v
}

fn parent_json(c: &Ctx<'_>, rd: &Read) -> Value {
    let org = rd.dir.authz.as_ref().and_then(graftyard_identity::AuthzRead::opened) == Some(AccountKind::Org);
    let mut p = crate::transfer::minimal_repository(c, rd.repo.id.0.0, rd.repo.owner, &rd.at.owner, &rd.at.name, org);
    p["fork"] = json!(rd.repo.network().root() != rd.repo.id);
    p
}

/// The network's root: the parent itself, or, for a fork of a fork, the root's own `minimal-repository`, read by id.
async fn source_json(c: &Ctx<'_>, rd: &Read, parent: &Value) -> Value {
    let root = rd.repo.network().root();
    if root == rd.repo.id {
        return parent.clone();
    }
    member_json(c, crate::repo_by_id(c, root).await.ok(), root, false).await
}

/// A network member's `minimal-repository` with `fork` set, named from its owner's login and `Authz`; only `{"id",
/// "fork"}` when the member, its owner's login or its name does not answer.
async fn member_json(c: &Ctx<'_>, repo: Option<crate::Repo>, id: RepoId, fork: bool) -> Value {
    match repo {
        Some(r) => named_json(c, r, fork).await,
        None => None,
    }
    .unwrap_or_else(|| json!({ "id": id.0.0, "fork": fork }))
}

/// `r`'s `minimal-repository` with `fork` set, or `None` when its owner's login or its name does not answer.
async fn named_json(c: &Ctx<'_>, r: crate::Repo, fork: bool) -> Option<Value> {
    let id = r.id;
    let (login, name, r, a) = crate::api::repo_named(c, r).await?;
    let org = graftyard_identity::AuthzRead::opened(&a) == Some(AccountKind::Org);
    let mut v = crate::transfer::minimal_repository(c, id.0.0, r.owner, &login, &name, org);
    v["fork"] = json!(fork);
    Some(v)
}

/// `GET .../forks`: the repository's direct forks from the network chain, each as `minimal-repository` with
/// `fork: true`, in id order (GHES's default sort is `newest`; ids grow with creation [inference]).
async fn list(c: &Ctx<'_>, rd: &Read) -> Result<Reply> {
    let network = rd.repo.network();
    let forks = match network_forks(c, network, rd.repo.id).await {
        Ok(f) => f,
        Err(e) => return Ok(crate::api::unavailable(&e)),
    };
    let mut out = Vec::new();
    for f in forks.into_iter().rev() {
        let Ok(r) = crate::repo_by_id(c, f).await else { continue };
        let Some(v) = named_json(c, r, true).await else { continue };
        out.push(v);
    }
    Ok(Reply::json(200, Value::Array(out)))
}

/// The network chain's answer to `/read` query `q` under the root's placement `record`; `None` when the chain has no
/// group yet, as for a network with no fork.
async fn network_read(
    c: &Ctx<'_>,
    network: NetworkId,
    record: &graftyard_types::PlacementRecord,
    q: Value,
) -> std::result::Result<Option<Value>, String> {
    let b = c.bindings();
    let q = json!({ "q": q }).to_string();
    let got = graftyard_lookup::call_opt(&b, ChainId::Network(network), record, "/read", q.as_bytes()).await?;
    Ok(got.map(|(v, _, _)| v.get("value").cloned().unwrap_or(Value::Null)))
}

/// The direct forks of `parent` that the network chain lists; none when the chain has no group yet.
pub(crate) async fn network_forks(
    c: &Ctx<'_>,
    network: NetworkId,
    parent: RepoId,
) -> std::result::Result<Vec<RepoId>, String> {
    let root = crate::repo_by_id(c, network.root()).await?;
    let Some(v) = network_read(c, network, &root.record, json!({ "forks": parent.0.0 })).await? else {
        return Ok(Vec::new());
    };
    let ids: Vec<i64> = serde_json::from_value(v.get("forks").cloned().unwrap_or(Value::Null))
        .map_err(|e| format!("{network:?} forks: {e}"))?;
    Ok(ids.into_iter().map(|i| RepoId(graftyard_types::DbId(i))).collect())
}

/// Every member of `network`, ascending, from one network chain read under the root's placement `record`: the root
/// and the forks its chain lists. The root alone when the chain has no group, or when a member of the release before
/// `{"members"}` answers without the key.
pub(crate) async fn network_members(
    c: &Ctx<'_>,
    network: NetworkId,
    record: &graftyard_types::PlacementRecord,
) -> std::result::Result<Vec<RepoId>, String> {
    let root = vec![network.root()];
    let Some(v) = network_read(c, network, record, json!({ "members": true })).await? else { return Ok(root) };
    let Some(m) = v.get("members").cloned() else { return Ok(root) };
    let ids: Vec<i64> = serde_json::from_value(m).map_err(|e| format!("{network:?} members: {e}"))?;
    Ok(ids.into_iter().map(|i| RepoId(graftyard_types::DbId(i))).collect())
}

/// The fork fields of `GET /repos/{o}/{r}` (`design/SHARED.md`, the `rest:repos` row; `build/BUILD-PLAN.md`,
/// AM3-WP2a): the repository's direct forks, every fork of its network, and whether it is a fork itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ForkFields {
    pub(crate) direct: u64,
    pub(crate) total: u64,
    pub(crate) fork: bool,
}

impl ForkFields {
    /// From the network chain's answer to `{"forks": <repo>}` (`None`: no group, or a read that failed, which reads
    /// as no forks) and whether the repository's chain holds a `fork` row. A member of the release before `total`
    /// answers only `forks`, which reads as the network's total.
    pub(crate) fn of(answer: Option<&Value>, fork: bool) -> ForkFields {
        let direct = answer.and_then(|v| v.get("forks")).and_then(Value::as_array).map_or(0, |f| f.len() as u64);
        let total = answer.and_then(|v| v.get("total")).and_then(Value::as_u64).unwrap_or(direct);
        ForkFields { direct, total, fork }
    }

    /// Whether the reply differs from a repository with no forks, whose bytes and ETag stay main's.
    pub(crate) fn moves(self) -> bool {
        self != ForkFields::default()
    }

    /// The ETag's resource: main's `repos/{id}`, or with the counts once they move, so a conditional GET held from
    /// before a fork is not answered 304 with a stale count.
    pub(crate) fn resource(self, id: RepoId) -> String {
        if self.moves() {
            format!("repos/{}/forks/{}/{}", id.0.0, self.direct, self.total)
        } else {
            format!("repos/{}", id.0.0)
        }
    }

    /// Sets `forks`, `forks_count`, `network_count` and `fork` on a `full-repository` body. `network_count` counts the
    /// forks of the whole network, as GitHub's does [inference].
    pub(crate) fn render(self, body: &mut Value) {
        body["forks"] = json!(self.direct);
        body["forks_count"] = json!(self.direct);
        body["network_count"] = json!(self.total);
        body["fork"] = json!(self.fork);
    }
}

/// `GET /repos/{o}/{r}`'s reply `r` with the fork fields, once forks are enabled (`on`). One network chain read gives
/// the counts; a fork also renders `parent` and `source` (the network's root) as `minimal-repository`. A repository
/// with no forks that is no fork keeps `r` as it is, body and ETag. A read that fails renders the counts as 0, and
/// the fork's `parent` and `source` as only what its row names.
pub(crate) async fn with_fork_fields(c: &Ctx<'_>, rd: &Read, mut r: Reply, index: u64) -> Reply {
    let row = graftyard_git::tiers::fork(&rd.core);
    let network = graftyard_git::tiers::network(&rd.core, rd.repo.id);
    let root_id = network.root();
    let root = if root_id == rd.repo.id { Ok(rd.repo.clone()) } else { crate::repo_by_id(c, root_id).await };
    let answer = match &root {
        Ok(root) => network_read(c, network, &root.record, json!({ "forks": rd.repo.id.0.0 })).await,
        Err(e) => Err(e.clone()),
    };
    let answer = answer.unwrap_or_else(|e| {
        console_error!("{network:?}: the fork counts of {:?} did not answer: {e}", rd.repo.id);
        None
    });
    let f = ForkFields::of(answer.as_ref(), row.is_some());
    if !f.moves() {
        return r;
    }
    let graftyard_wire::Body::Json(body) = &mut r.body else { return r };
    f.render(body);
    if let Some(row) = row {
        let source = member_json(c, root.ok(), root_id, false).await;
        if let Some((parent, _)) = row.fork_of {
            body["parent"] = if parent == root_id {
                source.clone()
            } else {
                member_json(c, crate::repo_by_id(c, parent).await.ok(), parent, true).await
            };
        }
        body["source"] = source;
    }
    let tag = graftyard_wire::etag(&f.resource(rd.repo.id), graftyard_types::Index(index));
    r.with("etag", tag)
}

#[cfg(test)]
mod tests {
    use graftyard_identity::audit::Owed;
    use graftyard_identity::{AuthzCmd, AuthzOp, RepoName};
    use graftyard_types::{AccountId, DbId, NetworkId, RepoId, Timestamp};
    use serde_json::json;

    /// The edge's two bodies decode as the site reads them: a `CreateFork` naming the parent and its network, which a
    /// fork of a fork takes from its parent (not the parent's own id), and the in-band finish's `Owed::Fork`.
    #[test]
    fn a_forks_commit_and_finish_bodies_decode_as_the_site_reads_them() {
        let (parent, network) = (RepoId(DbId(41)), NetworkId(DbId(40)));
        let name = RepoName::parse("tool").unwrap();
        let cmd =
            json!({ "cred": { "account": 7, "key": null }, "op": super::create_op(&name, "{}", parent, network) });
        let cmd: AuthzCmd = serde_json::from_value(cmd).unwrap();
        assert_eq!(cmd.op, AuthzOp::CreateFork { name: name.clone(), init: "{}".into(), parent, network });

        let owner = AccountId(DbId(7));
        let v = super::owed(owner, &name, RepoId(DbId(42)), "{}", parent, network, Timestamp(9));
        let owed: Owed = serde_json::from_value(v["owed"].clone()).unwrap();
        let want = Owed::Fork {
            owner,
            key: name.key(),
            name,
            repo: RepoId(DbId(42)),
            init: "{}".into(),
            parent,
            network,
            at: Timestamp(9),
        };
        assert_eq!(owed, want);
    }

    /// A repository with no forks renders main's three zeros and `fork: false`, and keeps main's ETag resource; no
    /// group reads the same as a group that lists no fork of it.
    #[test]
    fn no_forks_leaves_the_repository_body_and_etag_resource_as_main() {
        let id = RepoId(DbId(40));
        let main = json!({ "forks": 0, "forks_count": 0, "network_count": 0, "fork": false });
        for answer in [None, Some(json!({ "forks": [], "total": 0 })), Some(json!({ "forks": [] }))] {
            let f = super::ForkFields::of(answer.as_ref(), false);
            assert_eq!(f, super::ForkFields::default());
            assert!(!f.moves());
            assert_eq!(f.resource(id), "repos/40");
            let mut body = main.clone();
            f.render(&mut body);
            assert_eq!(body, main);
        }
    }

    /// Root 40, its fork 41, and 41's fork 42: the root and the middle fork each have one direct fork in a network of
    /// two; the deepest is a fork with none. Each one's ETag resource names its counts, and the answers differ.
    #[test]
    fn a_fork_of_a_fork_counts_once_directly_and_twice_in_the_network() {
        let (root, mid, deep) = (RepoId(DbId(40)), RepoId(DbId(41)), RepoId(DbId(42)));
        let mut fold = graftyard_git::network::NetworkFold::new(NetworkId::of_root(root));
        fold.forks.insert(mid, root);
        fold.forks.insert(deep, mid);
        // The site's `{"forks": parent}` answer (`workers/site/src/family.rs`, `Network::read_json`).
        let answer = |p: RepoId| {
            json!({ "forks": fold.forks_of(p).map(|r| r.0.0).collect::<Vec<_>>(),
            "total": fold.forks.len() })
        };
        let fields = |r: RepoId, fork: bool| super::ForkFields::of(Some(&answer(r)), fork);

        let f = fields(root, false);
        assert_eq!(f, super::ForkFields { direct: 1, total: 2, fork: false });
        let mut body = json!({ "forks": 0, "forks_count": 0, "network_count": 0, "fork": false });
        f.render(&mut body);
        assert_eq!(body, json!({ "forks": 1, "forks_count": 1, "network_count": 2, "fork": false }));
        assert_eq!(f.resource(root), "repos/40/forks/1/2");

        let f = fields(mid, true);
        assert_eq!(f, super::ForkFields { direct: 1, total: 2, fork: true });
        let mut body = json!({ "fork": false });
        f.render(&mut body);
        assert_eq!(body, json!({ "forks": 1, "forks_count": 1, "network_count": 2, "fork": true }));

        // A fork with no forks of its own still moves: `fork: true`, and a tag of its own.
        let f = fields(deep, true);
        assert_eq!(f, super::ForkFields { direct: 0, total: 2, fork: true });
        assert!(f.moves());
        assert_eq!(f.resource(deep), "repos/42/forks/0/2");

        // A member of the release before `total` answers the direct forks alone, read as the network's total.
        assert_eq!(
            super::ForkFields::of(Some(&json!({ "forks": [41] })), false),
            super::ForkFields { direct: 1, total: 1, fork: false }
        );
        // A read that failed renders zeros; a fork still renders as one.
        assert_eq!(super::ForkFields::of(None, true), super::ForkFields { direct: 0, total: 0, fork: true });
    }
}
