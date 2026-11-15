//! `rest:branches`' branch protection as GHES 3.22 shapes it (`design/SHARED.md`, the `rest:branches` row): `GET`,
//! `PUT`, and `DELETE /repos/{o}/{r}/branches/{branch}/protection`, each on the repository chain's `Rule` rows through
//! `IssueOp::SetRule` (format 32.5). A branch's rule is the one whose pattern is the branch's name, since this route
//! takes no wildcard [doc: GHES 3.22 REST, "Update branch protection", `branch` "cannot contain wildcard characters"].
//!
//! The `PUT` body needs all four of `required_status_checks`, `enforce_admins`, `required_pull_request_reviews`, and
//! `restrictions`, each possibly `null`, as GitHub's. `restrictions` other than `null` is refused 422: who may push is
//! not resolved from logins here [inference: no gate needs it].

use graftyard_git::issue_ops::{IssueErr, IssueOp, IssueOut};
use graftyard_git::issues::{
    AppId, BranchRule, Grouping, MergeMethod, MergeQueueSettings, Protection, RequiredChecks, RequiredReviews,
};
use graftyard_raft::Acked;
use graftyard_types::DbId;
use serde_json::{Value, json};
use smallvec::SmallVec;

use crate::view::Repo;
use crate::wire::Reply;

pub const DOC: &str = "rest/branches/branch-protection#update-branch-protection";

/// The rule `PUT` and `DELETE` edit: the one whose pattern is `branch`.
pub fn rule_of(r: &Repo<'_>, branch: &str) -> Option<BranchRule> {
    r.rules().into_iter().find(|x| &*x.pattern == branch && x.protection.is_some())
}

fn invalid(msg: &str) -> Reply {
    Reply::error(422, msg, DOC)
}

/// `PUT`'s command.
pub fn plan_put(r: &Repo<'_>, branch: &str, body: &Value) -> Result<IssueOp, Reply> {
    let keys = ["required_status_checks", "enforce_admins", "required_pull_request_reviews", "restrictions"];
    if branch.is_empty() || branch.contains('*') || keys.iter().any(|k| body.get(k).is_none()) {
        return Err(invalid("Invalid request."));
    }
    if !body["restrictions"].is_null() {
        return Err(invalid("restrictions are not supported"));
    }
    let checks = match &body["required_status_checks"] {
        Value::Null => None,
        c => {
            let mut names: SmallVec<[(Box<str>, Option<AppId>); 4]> = SmallVec::new();
            for n in c["contexts"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                names.push((n.into(), None));
            }
            for k in c["checks"].as_array().into_iter().flatten() {
                let Some(n) = k["context"].as_str() else { return Err(invalid("Invalid request.")) };
                let app = k["app_id"].as_i64().filter(|a| *a > 0).map(|a| AppId(DbId(a)));
                if !names.iter().any(|(x, _)| &**x == n) {
                    names.push((n.into(), app));
                }
            }
            Some(RequiredChecks { strict: c["strict"].as_bool().unwrap_or(false), checks: names })
        }
    };
    let reviews = match &body["required_pull_request_reviews"] {
        Value::Null => None,
        v => {
            let count = v["required_approving_review_count"].as_u64().unwrap_or(1);
            if count > 6 {
                return Err(invalid("required_approving_review_count must be 0 to 6"));
            }
            Some(RequiredReviews {
                count: count as u8,
                dismiss_stale: v["dismiss_stale_reviews"].as_bool().unwrap_or(false),
                code_owners: v["require_code_owner_reviews"].as_bool().unwrap_or(false),
                last_push: v["require_last_push_approval"].as_bool().unwrap_or(false),
                dismissers: SmallVec::new(),
                bypass: SmallVec::new(),
            })
        }
    };
    let flag = |k: &str| body[k].as_bool().unwrap_or(false);
    let protection = Protection {
        checks,
        reviews,
        push: None,
        enforce_admins: flag("enforce_admins"),
        linear_history: flag("required_linear_history"),
        force_pushes: flag("allow_force_pushes"),
        deletions: flag("allow_deletions"),
        block_creations: flag("block_creations"),
        conversation_resolution: flag("required_conversation_resolution"),
        lock_branch: flag("lock_branch"),
        signatures: false,
        merge_queue: match body.get("merge_queue") {
            Some(Value::Null) => None,
            Some(q) => Some(merge_queue(q)?),
            None => rule_of(r, branch).and_then(|x| x.protection?.merge_queue),
        },
    };
    let id = rule_of(r, branch).map(|x| x.id);
    Ok(IssueOp::SetRule { id, pattern: branch.into(), protection: Some(protection) })
}

/// The merge queue a `PUT` sets, a Graftyard extension (`decisions/DECISIONS.md`, "M4-WP3: BF-37's merge queue
/// setting and `ActionsTrigger`"): `merge_queue` holds GHES's `merge_queue` ruleset rule's `parameters`, all seven
/// required, and `null` removes the queue. A body without the key keeps the rule's queue, as a GHES branch protection
/// `PUT` leaves a ruleset's queue alone; GHES itself ignores the key, so a GHES client's `PUT` behaves as on GHES.
fn merge_queue(q: &Value) -> Result<MergeQueueSettings, Reply> {
    let bad = || invalid("Invalid merge_queue.");
    let n = |k: &str, max: u64| q[k].as_u64().filter(|v| *v <= max).ok_or_else(bad);
    let grouping = match q["grouping_strategy"].as_str() {
        Some("ALLGREEN") => Grouping::AllGreen,
        Some("HEADGREEN") => Grouping::HeadGreen,
        _ => return Err(bad()),
    };
    let method = match q["merge_method"].as_str() {
        Some("MERGE") => MergeMethod::Merge,
        Some("SQUASH") => MergeMethod::Squash,
        Some("REBASE") => MergeMethod::Rebase,
        _ => return Err(bad()),
    };
    // GHES's bounds: a timeout of 1 to 360 minutes, at most 100 entries to build or merge, and a wait of at most 360.
    Ok(MergeQueueSettings {
        check_timeout_minutes: n("check_response_timeout_minutes", 360)?.max(1) as u16,
        grouping,
        max_build: n("max_entries_to_build", 100)?.max(1) as u8,
        min_merge: n("min_entries_to_merge", 100)? as u8,
        max_merge: n("max_entries_to_merge", 100)?.max(1) as u8,
        min_wait_minutes: n("min_entries_to_merge_wait_minutes", 360)? as u16,
        method,
    })
}

/// `DELETE`'s command, or 404 for a branch with no rule.
pub fn plan_delete(r: &Repo<'_>, branch: &str) -> Result<IssueOp, Reply> {
    let rule = rule_of(r, branch).ok_or_else(|| Reply::error(404, "Branch not protected", DOC))?;
    Ok(IssueOp::SetRule { id: Some(rule.id), pattern: rule.pattern, protection: None })
}

/// `GET`: the pinned `protected-branch` object, or 404 "Branch not protected".
pub fn get(r: &Repo<'_>, branch: &str) -> Reply {
    match rule_of(r, branch) {
        Some(rule) => Reply::json(200, protected_branch(r, branch, rule.protection.as_ref().expect("a protection"))),
        None => Reply::error(404, "Branch not protected", DOC),
    }
}

/// The reply of a committed `SetRule`: `PUT`'s 200 from the rule after the entry applied, or `DELETE`'s 204, whose
/// entry deleted the rule's row.
pub fn saved(acked: &Acked<IssueOut>, after: &Repo<'_>, branch: &str) -> Reply {
    let IssueOut::Id(id) = acked.out else { return Reply::error(500, "Internal Server Error", "rest") };
    match after.rules().into_iter().find(|x| x.id == id) {
        Some(BranchRule { protection: Some(p), .. }) => Reply::json(200, protected_branch(after, branch, &p)),
        _ => Reply::empty(204),
    }
}

/// A refused `SetRule`.
pub fn refused(e: &IssueErr) -> Reply {
    match e {
        IssueErr::Unauthorized(_) => Reply::error(401, "Bad credentials", DOC),
        IssueErr::Forbidden | IssueErr::Agent(_) => Reply::error(403, "Must have admin rights to Repository.", DOC),
        IssueErr::NotFound => Reply::error(404, "Branch not protected", DOC),
        IssueErr::NeedIdBlock | IssueErr::Unavailable => Reply::error(503, "Service Unavailable", "rest"),
        _ => invalid("Validation Failed"),
    }
}

fn protected_branch(r: &Repo<'_>, branch: &str, p: &Protection) -> Value {
    let url = format!("{}/repos/{}/{}/branches/{branch}/protection", r.host.api(), r.at.owner, r.at.name);
    let enabled = |sub: &str, on: bool| json!({ "url": format!("{url}/{sub}"), "enabled": on });
    let bare = |on: bool| json!({ "enabled": on });
    let mut out = json!({
        "url": url,
        "enforce_admins": enabled("enforce_admins", p.enforce_admins),
        "required_signatures": enabled("required_signatures", p.signatures),
        "required_linear_history": bare(p.linear_history),
        "allow_force_pushes": bare(p.force_pushes),
        "allow_deletions": bare(p.deletions),
        "block_creations": bare(p.block_creations),
        "required_conversation_resolution": bare(p.conversation_resolution),
        "lock_branch": bare(p.lock_branch),
        "allow_fork_syncing": bare(false),
    });
    if let Some(c) = &p.checks {
        out["required_status_checks"] = json!({
            "url": format!("{url}/required_status_checks"),
            "strict": c.strict,
            "contexts": c.checks.iter().map(|(n, _)| &**n).collect::<Vec<_>>(),
            "contexts_url": format!("{url}/required_status_checks/contexts"),
            "checks": c.checks.iter().map(|(n, a)| json!({ "context": &**n, "app_id": a.map(|a| a.0.0) })).collect::<Vec<_>>(),
        });
    }
    if let Some(v) = &p.reviews {
        out["required_pull_request_reviews"] = json!({
            "url": format!("{url}/required_pull_request_reviews"),
            "dismiss_stale_reviews": v.dismiss_stale,
            "require_code_owner_reviews": v.code_owners,
            "require_last_push_approval": v.last_push,
            "required_approving_review_count": v.count,
        });
    }
    out
}
