//! BF-76's org substitution policy and the vendor's signed pin map on the edge (`design/SHARED.md`, "Toolkit hostname
//! heuristics and SHA pins"; `decisions/DECISIONS.md`, "`pin-map`"):
//!
//! - `PUT /api/graftyard/v1/actions/pin-map` `{document, signature}` (base64): a site admin stores the vendor's map
//!   under `actions/pin-maps/<digest>` in the install's home bucket, verified against `Keys.release` first, and makes
//!   it the install's current map (`actions/pin-maps/current`), whose fork SHAs an unapproved pin's annotation names.
//! - `GET` and `PUT /api/graftyard/v1/orgs/{org}/actions/substitution` `{tags?, pin_map?}`: an org admin's policy
//!   (Authz 0x12.15). A digest is approved only when the install holds that map and it verifies.
//! - `substitute`: the launch service's read of the job's org policy and maps, which chooses the fork for each `uses:`.
//! - `annotations`: what `completejob` commits on the job's check run: the runner's and Graftyard's own.
//!
//! Every map read back is verified again against `Keys.release`, so a changed object resolves nothing.

use graftyard_actions::launch::ActionRef;
use graftyard_actions::pin_map::{self, PinMap, Substitution};
use graftyard_git::issues::CheckAnnotation;
use graftyard_identity::{AUTHZ_FORMAT_V15, ActionsSubstitution, AuthzFold, AuthzOp, AuthzRead as _};
use graftyard_types::{AccountId, ChainId, Digest};
use graftyard_wire::Reply;
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;
use crate::api::unavailable;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
const PREFIX: &str = "actions/pin-maps/";
const CURRENT: &str = "actions/pin-maps/current";
const DOC: &str = "graftyard/actions/pin-map";

/// The install's `Keys.release` key, from the `Release` view this isolate holds.
async fn release_key(c: &Ctx<'_>) -> Option<[u8; 32]> {
    let anchor = c.anchor.as_ref()?;
    Box::pin(graftyard_lookup::view::ensure(&c.bindings(), anchor, Some(c.ctx))).await;
    graftyard_lookup::view::with(|r| r.fold.keys.as_ref().map(|k| k.release.0))
}

/// The install's home bucket: the `Release` group's home site's.
async fn bucket(c: &Ctx<'_>) -> std::result::Result<Bucket, String> {
    let record = c.system_record(ChainId::Release).await?;
    c.env.bucket(&format!("BUCKET_{}", record.home.0)).map_err(|e| e.to_string())
}

/// The map stored under `key` (`<digest>` or `current`), verified against `Keys.release` and, for a digest, that it is
/// that digest's document. `Ok(None)` when no object is there.
async fn stored(
    c: &Ctx<'_>,
    b: &Bucket,
    key: &str,
    want: Option<Digest>,
) -> std::result::Result<Option<(Digest, PinMap)>, String> {
    let Some(o) = b.get(format!("{PREFIX}{key}")).execute().await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let sig = o.custom_metadata().map_err(|e| e.to_string())?.get("signature").cloned().unwrap_or_default();
    let doc = match o.body() {
        Some(body) => body.bytes().await.map_err(|e| e.to_string())?,
        None => return Ok(None),
    };
    let key = release_key(c).await.ok_or("this edge holds no Release view with a release key")?;
    let sig: [u8; 64] = base64::Engine::decode(&B64, sig)
        .ok()
        .and_then(|s| s.try_into().ok())
        .ok_or("the stored pin map has no signature")?;
    let digest = pin_map::digest(&doc);
    if want.is_some_and(|w| w != digest) {
        return Err(format!("the object under {} is another document", digest.hex()));
    }
    pin_map::verify(&doc, &sig, &key, digest).map(|m| Some((digest, m))).map_err(|e| format!("{e:?}"))
}

/// The org's policy and the two maps `pin_map::substitute` reads: the approved one and the install's current one.
pub(crate) struct Policy {
    pub tags: bool,
    pub approved: Option<PinMap>,
    pub known: Option<PinMap>,
}

impl Policy {
    fn off() -> Policy {
        Policy { tags: false, approved: None, known: None }
    }
}

/// The reads `policy_from` and `annotations_from` make: the edge's (`Live`) and the tests' fake.
#[allow(async_fn_in_trait)]
pub(crate) trait MapReads {
    /// The org's policy; `None` when 0x12.15 is off, the owner has no `Authz` chain, or it did not answer.
    async fn substitution(&self, owner: AccountId) -> Option<ActionsSubstitution>;
    /// The approved map (`approved`, checked against its digest) and the install's current map, each verified
    /// against `Keys.release`; a missing or unreadable map is `None`.
    async fn maps(&self, approved: Option<Digest>) -> (Option<PinMap>, Option<PinMap>);
    async fn login(&self, owner: AccountId) -> Option<String>;
}

struct Live<'a, 'b>(&'a Ctx<'b>);

impl MapReads for Live<'_, '_> {
    async fn substitution(&self, owner: AccountId) -> Option<ActionsSubstitution> {
        if !self.0.cfg.enabled.contains(&AUTHZ_FORMAT_V15) {
            return None;
        }
        match crate::ident::authz_of(self.0, owner).await {
            Ok(f) => f.map(|f| f.actions().more.substitution),
            Err(e) => {
                console_error!("pin map: owner {} Authz: {e}", owner.0.0);
                None
            }
        }
    }
    async fn maps(&self, approved: Option<Digest>) -> (Option<PinMap>, Option<PinMap>) {
        let c = self.0;
        let b = match bucket(c).await {
            Ok(b) => b,
            Err(e) => {
                console_error!("pin map: home bucket: {e}");
                return (None, None);
            }
        };
        let read = |key: String, want: Option<Digest>| {
            let b = &b;
            async move {
                match stored(c, b, &key, want).await {
                    Ok(m) => m.map(|(_, m)| m),
                    Err(e) => {
                        console_error!("pin map {key}: {e}");
                        None
                    }
                }
            }
        };
        let approved = match approved {
            Some(d) => read(d.hex(), Some(d)).await,
            None => None,
        };
        (approved, read("current".into(), None).await)
    }
    async fn login(&self, owner: AccountId) -> Option<String> {
        crate::ident::login(self.0, owner).await
    }
}

/// The policy of the org that owns the job's run (`owner`). A missing or unreadable map is no map: the pin then runs
/// as written, which is GHES's behaviour, and the reason is logged. `known` asks for the install's current map even
/// when the org has neither tags nor an approval: an unapproved pin is told apart from any other SHA only by it.
pub(crate) async fn policy(c: &Ctx<'_>, owner: AccountId, known: bool) -> Policy {
    policy_from(&Live(c), owner, known).await
}

pub(crate) async fn policy_from(r: &impl MapReads, owner: AccountId, known: bool) -> Policy {
    let Some(p) = r.substitution(owner).await else { return Policy::off() };
    if !p.tags && p.pin_map.is_none() && !known {
        return Policy::off();
    }
    let (approved, known) = r.maps(p.pin_map).await;
    Policy { tags: p.tags, approved, known }
}

/// Each action as the launch service resolves it under `p`, with the substitution chosen; one log line per substituted
/// action names both references (the SHAs follow in `launch`'s line once resolved).
pub(crate) fn substitute(p: &Policy, asked: &[ActionRef]) -> Vec<(ActionRef, Substitution)> {
    asked
        .iter()
        .map(|a| {
            let s = pin_map::substitute(&a.owner, &a.repo, &a.version, p.tags, p.approved.as_ref(), p.known.as_ref());
            (pin_map::redirect(a, &s), s)
        })
        .collect()
}

/// The annotations `completejob` commits: the Graftyard annotation for each unapproved v4+ pin a step ran, then the
/// runner's own, capped (`graftyard_actions::annotations::job_annotations`).
pub(crate) async fn annotations(c: &Ctx<'_>, owner: AccountId, b: &Value) -> Vec<CheckAnnotation> {
    annotations_from(&Live(c), owner, b, &format!("https://{}", c.cfg.host)).await
}

pub(crate) async fn annotations_from(
    r: &impl MapReads,
    owner: AccountId,
    b: &Value,
    web: &str,
) -> Vec<CheckAnnotation> {
    use graftyard_actions::annotations as an;
    let runner = an::from_complete(b);
    let steps = an::step_actions(b);
    let mut first = Vec::new();
    if steps.iter().any(|(name, _)| name.split_once('/').is_some_and(|(o, r)| pin_map::fork_of(o, r).is_some())) {
        // The current map is read whatever the policy: with tags off and no approval, the gate's state, it is the only
        // thing that makes a pin `Unapproved` (BF-76 r1, check run 1972519, had no Graftyard annotation).
        let p = policy_from(r, owner, true).await;
        let org = r.login(owner).await.unwrap_or_default();
        for (name, version) in steps {
            let Some((o, rp)) = name.split_once('/') else { continue };
            if let Substitution::Unapproved { fork, fork_sha: Some(fork_sha) } =
                pin_map::substitute(o, rp, &version, p.tags, p.approved.as_ref(), p.known.as_ref())
            {
                first.push(an::pin_map_annotation(&name, &version, &fork, &fork_sha, web, &org));
            }
        }
    }
    an::job_annotations(first, runner)
}

// ---- routes ----

pub async fn graftyard_route(c: &Ctx<'_>, req: &mut Request, rest: &str) -> Result<Option<Reply>> {
    let segs: Vec<&str> = rest.trim_matches('/').split('/').collect();
    let org = match segs.as_slice() {
        ["actions", "pin-map"] => None,
        ["orgs", org, "actions", "substitution"] => Some(*org),
        _ => return Ok(None),
    };
    if !c.cfg.enabled.contains(&AUTHZ_FORMAT_V15) {
        return Ok(Some(Reply::not_found()));
    }
    let Some(who) = crate::authenticate(c, req).await? else {
        return Ok(Some(Reply::error(401, "Requires authentication", "rest")));
    };
    let user = match who.actor {
        graftyard_identity::Actor::User(u) => u,
        _ => return Ok(Some(Reply::error(403, "Must have admin rights to this resource.", DOC))),
    };
    let method = req.method();
    let body: Value = match method {
        Method::Put => serde_json::from_slice(&req.bytes().await?).unwrap_or(Value::Null),
        Method::Get => Value::Null,
        _ => return Ok(Some(Reply::error(405, "Method Not Allowed", DOC))),
    };
    let Some(org) = org else {
        // The install's map: a site admin's, as `/agents/apps` is.
        let install = graftyard_identity::install_account();
        let f = match crate::ident::authz_of(c, install).await {
            Ok(Some(f)) => f,
            Ok(None) => return Ok(Some(Reply::not_found())),
            Err(e) => return Ok(Some(unavailable(&e))),
        };
        if !f.is_admin(user) {
            return Ok(Some(Reply::error(403, "Must have admin rights to this resource.", DOC)));
        }
        if method != Method::Put {
            return Ok(Some(Reply::error(405, "Method Not Allowed", DOC)));
        }
        return Ok(Some(put_map(c, &body).await));
    };
    let owner = match crate::account_of(c, org).await {
        Ok(Some(a)) => a,
        Ok(None) => return Ok(Some(Reply::not_found())),
        Err(e) => return Ok(Some(unavailable(&e))),
    };
    let Some(record) = crate::placement(c, owner).await.map_err(Error::RustError)? else {
        return Ok(Some(Reply::not_found()));
    };
    let Some(f) = crate::authz_fold(c, owner, &record).await else {
        return Ok(Some(unavailable("the owner's Authz chain did not answer; retry")));
    };
    let org_kind = f.opened() == Some(graftyard_identity::AccountKind::Org);
    if !org_kind || !f.is_admin(user) {
        return Ok(Some(Reply::not_found()));
    }
    if method == Method::Get {
        return Ok(Some(Reply::json(200, policy_json(&f))));
    }
    let tags = match body.get("tags") {
        None => None,
        Some(Value::Bool(t)) => Some(*t),
        Some(_) => return Ok(Some(invalid("tags"))),
    };
    let pin_map = match body.get("pin_map") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(h)) => {
            let Some(d) = Digest::parse_hex(h) else { return Ok(Some(invalid("pin_map"))) };
            // Only a map the install holds, and that verifies, can be approved.
            let b = match bucket(c).await {
                Ok(b) => b,
                Err(e) => return Ok(Some(unavailable(&e))),
            };
            match stored(c, &b, &d.hex(), Some(d)).await {
                Ok(Some(_)) => Some(Some(d)),
                Ok(None) => return Ok(Some(invalid("pin_map"))),
                Err(e) => return Ok(Some(Reply::error(422, &format!("the pin map does not verify: {e}"), DOC))),
            }
        }
        Some(_) => return Ok(Some(invalid("pin_map"))),
    };
    let op = AuthzOp::SetActionsSubstitution { tags, pin_map };
    if let Err(r) = crate::secrets::commit_as(c, owner, &record, who.credential_shard(), op).await {
        return Ok(Some(r));
    }
    Ok(Some(match crate::authz_fold(c, owner, &record).await {
        Some(f) => Reply::json(200, policy_json(&f)),
        None => unavailable("the owner's Authz chain did not answer; retry"),
    }))
}

fn invalid(field: &str) -> Reply {
    Reply::json(
        422,
        json!({ "message": "Validation Failed", "errors": [{ "resource": "ActionsSubstitution", "field": field,
                 "code": "invalid" }], "documentation_url": DOC }),
    )
}

fn policy_json(f: &AuthzFold) -> Value {
    let p = f.actions().more.substitution;
    json!({ "tags": p.tags, "pin_map": p.pin_map.map(|d| d.hex()) })
}

/// Stores the vendor's map once its signature verifies under `Keys.release`, and makes it the install's current map.
async fn put_map(c: &Ctx<'_>, body: &Value) -> Reply {
    let (Some(doc), Some(sig)) = (body["document"].as_str(), body["signature"].as_str()) else {
        return invalid("document");
    };
    let (Ok(doc), Some(sig_bytes)) = (
        base64::Engine::decode(&B64, doc),
        base64::Engine::decode(&B64, sig).ok().and_then(|s| <[u8; 64]>::try_from(s).ok()),
    ) else {
        return invalid("signature");
    };
    let Some(key) = release_key(c).await else {
        return unavailable("this edge holds no Release view with a release key; retry");
    };
    let digest = pin_map::digest(&doc);
    if let Err(e) = pin_map::verify(&doc, &sig_bytes, &key, digest) {
        return Reply::error(422, &format!("the pin map does not verify under the release key: {e:?}"), DOC);
    }
    let b = match bucket(c).await {
        Ok(b) => b,
        Err(e) => return unavailable(&e),
    };
    let meta = std::collections::HashMap::from([("signature".to_owned(), sig.to_owned())]);
    for key in [format!("{PREFIX}{}", digest.hex()), CURRENT.to_owned()] {
        if let Err(e) = b.put(key, doc.clone()).custom_metadata(meta.clone()).execute().await {
            return unavailable(&e.to_string());
        }
    }
    Reply::json(201, json!({ "digest": digest.hex() }))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use futures_util::FutureExt;
    use graftyard_actions::pin_map::{Pin, PinMap};
    use graftyard_git::issues::AnnotationLevel;
    use graftyard_identity::ActionsSubstitution;
    use graftyard_types::{AccountId, DbId, Digest};
    use serde_json::json;

    use super::{MapReads, annotations_from};

    const UP: &str = "ea165f8d65b6e75b540449e92b4886f43607fa02";
    const FORK: &str = "aeb6043fcd99c3bdff3a1b86b98a3d66bf60897e";

    /// The org's policy, the install's current map (the vendor's, never approved here), and how often the maps were read.
    struct Fake {
        policy: ActionsSubstitution,
        maps_read: Cell<u8>,
    }

    fn map() -> PinMap {
        PinMap {
            version: 1,
            pins: vec![Pin {
                upstream: "actions/upload-artifact".into(),
                upstream_sha: UP.into(),
                fork: "graftyard-actions/upload-artifact".into(),
                fork_sha: FORK.into(),
                tag: "v4.6.2".into(),
            }],
        }
    }

    impl MapReads for Fake {
        async fn substitution(&self, _: AccountId) -> Option<ActionsSubstitution> {
            Some(self.policy)
        }
        async fn maps(&self, approved: Option<Digest>) -> (Option<PinMap>, Option<PinMap>) {
            self.maps_read.set(self.maps_read.get() + 1);
            (approved.map(|_| map()), Some(map()))
        }
        async fn login(&self, _: AccountId) -> Option<String> {
            Some("m3org".into())
        }
    }

    fn complete(action: &str, reference: &str) -> serde_json::Value {
        json!({ "conclusion": "failed", "stepResults": [
            { "action_name": action, "ref": reference, "annotations": [
                { "level": "FAILURE", "message": "@actions/artifact v2.0.0+ ... not currently supported on GHES.",
                  "startLine": 18, "endLine": 18 } ] } ] })
    }

    fn run(policy: ActionsSubstitution, body: &serde_json::Value) -> (Vec<graftyard_git::issues::CheckAnnotation>, u8) {
        let f = Fake { policy, maps_read: Cell::new(0) };
        let a =
            annotations_from(&f, AccountId(DbId(7100003)), body, "https://g.example").now_or_never().expect("ready");
        (a, f.maps_read.get())
    }

    /// BF-76 r1 (check run 1972519): with tag substitution off and no approval, the gate's state, a step pinned to the
    /// upstream commit of a pin in the install's current map got only the runner's annotation, because the policy read
    /// returned before it read the current map. Now the current map is read whatever the org's policy, and Graftyard's
    /// failure annotation, naming the approval page and the fork's SHA, comes before the runner's.
    #[test]
    fn an_unapproved_pin_gets_the_pin_map_annotation_with_tags_off_and_nothing_approved() {
        let (a, read) =
            run(ActionsSubstitution { tags: false, pin_map: None }, &complete("actions/upload-artifact", UP));
        assert_eq!(read, 1, "the current map is read with tags off and nothing approved");
        assert_eq!(a.len(), 2, "{a:?}");
        assert_eq!(a[0].title.as_deref(), Some("Pinned action needs the Graftyard pin map"));
        assert_eq!(a[0].level, AnnotationLevel::Failure);
        assert!(
            a[0].message.contains("https://g.example/organizations/m3org/settings/actions/pin-map"),
            "{}",
            a[0].message
        );
        assert!(a[0].message.contains(&format!("graftyard-actions/upload-artifact@{FORK}")), "{}", a[0].message);
        assert_eq!(a[1].start_line, 18, "the runner's own annotation follows");

        // Tags on changes nothing for a SHA pin; an approval of a map holding the pin does, and so does a tag.
        let (a, _) = run(ActionsSubstitution { tags: true, pin_map: None }, &complete("actions/upload-artifact", UP));
        assert_eq!(a.len(), 2);
        let (a, _) = run(
            ActionsSubstitution { tags: false, pin_map: Some(Digest([5; 32])) },
            &complete("actions/upload-artifact", UP),
        );
        assert_eq!(a.len(), 1, "an approved pin runs the fork: no Graftyard annotation");
        let (a, _) =
            run(ActionsSubstitution { tags: false, pin_map: None }, &complete("actions/upload-artifact", "v4"));
        assert_eq!(a.len(), 1, "a tag is not a pin");
        // A job that ran no forked action reads no map.
        let (a, read) = run(ActionsSubstitution { tags: false, pin_map: None }, &complete("actions/checkout", UP));
        assert_eq!((a.len(), read), (1, 0));
    }
}
