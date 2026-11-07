//! The agent settings endpoints under `/api/graftyard/v1` (`design/AGENT-LAYER.md`, "API reference"): the install's
//! `AgentApp` rows, an org's `agents_enabled` and workspace mode, and a repository's `AgentPolicy`. Bodies parse to
//! the commands the edge commits, and rows render to the reference's shapes. Nothing here is under `/api/v3`.

use graftyard_git::agent::{AgentPolicy, CiMode, LeaseEnforce};
use graftyard_identity::glob::{PathGlob, RefPattern};
use graftyard_identity::{AgentApp, AgentKind, AppId, AuthzFold, AuthzRead, OrgPolicy, WorkspaceMode};
use graftyard_types::DbId;
use graftyard_wire::{FieldError, Reply};
use serde_json::{Value, json};

pub const PREFIX: &str = "/api/graftyard/v1";
const DOC: &str = "graftyard/agents";

/// A route of this module, from the path after `PREFIX`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route<'p> {
    /// `/agents/apps`
    Apps,
    /// `/agents/apps/{app_id}`
    App(AppId),
    /// `/orgs/{org}/agents`
    Org(&'p str),
    /// `/repos/{owner}/{repo}/agents/policy`
    Policy { owner: &'p str, repo: &'p str },
}

pub fn route(rest: &str) -> Option<Route<'_>> {
    let segs: Vec<&str> = rest.strip_prefix('/')?.split('/').collect();
    match segs.as_slice() {
        ["agents", "apps"] => Some(Route::Apps),
        ["agents", "apps", id] => id.parse::<i64>().ok().filter(|i| *i > 0).map(|i| Route::App(AppId(DbId(i)))),
        ["orgs", org, "agents"] => Some(Route::Org(org)),
        ["repos", owner, repo, "agents", "policy"] => Some(Route::Policy { owner, repo }),
        _ => None,
    }
}

fn invalid(resource: &'static str, field: &'static str, code: &'static str) -> Reply {
    let e = FieldError { resource, field, code };
    Reply::json(422, crate::wire::validation_error("Validation Failed", DOC, &[e]))
}

// ---- AgentApp ----

fn kind_str(k: AgentKind) -> &'static str {
    match k {
        AgentKind::GraftyardRun => "graftyard_run",
        AgentKind::ThirdParty => "third_party",
        AgentKind::Mcp => "mcp",
    }
}

/// `PUT /agents/apps/{app_id}` with `{kind, confine?}`.
pub fn parse_app(app: AppId, body: &Value) -> Result<AgentApp, Reply> {
    let kind = match body.get("kind").and_then(Value::as_str) {
        Some("graftyard_run") => AgentKind::GraftyardRun,
        Some("third_party") => AgentKind::ThirdParty,
        Some("mcp") => AgentKind::Mcp,
        Some(_) => return Err(invalid("AgentApp", "kind", "invalid")),
        None => return Err(invalid("AgentApp", "kind", "missing_field")),
    };
    let confine = match body.get("confine") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(RefPattern::parse(s).map_err(|_| invalid("AgentApp", "confine", "invalid"))?),
        Some(_) => return Err(invalid("AgentApp", "confine", "invalid")),
    };
    Ok(AgentApp { app, kind, confine })
}

/// One row: `{app_id, slug, kind, confine}`. `slug` is the App's when the install registered it, else null.
pub fn app_json(install: &AuthzFold, a: &AgentApp) -> Value {
    json!({
        "app_id": a.app.0.0,
        "slug": install.app(a.app).map(|r| String::from(r.slug.clone())),
        "kind": kind_str(a.kind),
        "confine": a.confine.as_ref().map(RefPattern::as_str),
    })
}

/// `GET /agents/apps`: every row, by App id.
pub fn apps_json(install: &AuthzFold) -> Value {
    Value::Array(install.more.more.agent_apps.values().map(|a| app_json(install, a)).collect())
}

// ---- org agents ----

fn workspace_str(w: WorkspaceMode) -> &'static str {
    match w {
        WorkspaceMode::Fork => "fork",
        WorkspaceMode::InRepo => "in_repo",
    }
}

/// `PATCH /orgs/{org}/agents` with `{agents_enabled?, workspace?}`. `code_mode` is AM2's and refused here.
pub fn parse_org(body: &Value) -> Result<(Option<bool>, Option<WorkspaceMode>), Reply> {
    let enabled = match body.get("agents_enabled") {
        None => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err(invalid("OrgAgents", "agents_enabled", "invalid")),
    };
    let workspace = match body.get("workspace").map(|w| w.as_str()) {
        None => None,
        Some(Some("fork")) => Some(WorkspaceMode::Fork),
        Some(Some("in_repo")) => Some(WorkspaceMode::InRepo),
        Some(_) => return Err(invalid("OrgAgents", "workspace", "invalid")),
    };
    if body.get("code_mode").is_some() {
        return Err(invalid("OrgAgents", "code_mode", "invalid"));
    }
    Ok((enabled, workspace))
}

/// `org_agents` for `org`'s policy. `code_mode` is false until AM2 builds it.
pub fn org_json(org: &str, p: &OrgPolicy) -> Value {
    json!({
        "agents_enabled": p.agents_enabled,
        "workspace": workspace_str(p.agents_workspace),
        "workspace_org": format!("{org}-agents"),
        "code_mode": false,
    })
}

// ---- repository policy ----

/// `PUT /repos/{o}/{r}/agents/policy` with a whole `policy`; an absent field takes its default.
pub fn parse_policy(body: &Value) -> Result<AgentPolicy, Reply> {
    let bad = |f: &'static str| invalid("AgentPolicy", f, "invalid");
    let strs = |k: &'static str| -> Result<Vec<Box<str>>, Reply> {
        match body.get(k) {
            None => Ok(Vec::new()),
            Some(Value::Array(xs)) => xs.iter().map(|x| x.as_str().map(Box::from).ok_or_else(|| bad(k))).collect(),
            Some(_) => Err(bad(k)),
        }
    };
    let ci = match body.get("ci").map(Value::as_str) {
        None | Some(Some("approve")) => CiMode::Approve,
        Some(Some("auto_read_only")) => CiMode::AutoReadOnly,
        Some(_) => return Err(bad("ci")),
    };
    let leases = match body.get("leases").map(Value::as_str) {
        None | Some(Some("advisory")) => LeaseEnforce::Advisory,
        Some(Some("agents_exclusive")) => LeaseEnforce::AgentsExclusive,
        Some(_) => return Err(bad("leases")),
    };
    let extra_human_approvals = match body.get("extra_human_approvals") {
        None => 0,
        Some(v) => v.as_u64().and_then(|n| u8::try_from(n).ok()).ok_or_else(|| bad("extra_human_approvals"))?,
    };
    let extra_paths = strs("extra_paths")?;
    if extra_paths.iter().any(|g| PathGlob::parse(g).is_err()) {
        return Err(bad("extra_paths"));
    }
    Ok(AgentPolicy { ci, labels: strs("labels")?.into_iter().collect(), extra_paths, extra_human_approvals, leases })
}

pub fn policy_json(p: &AgentPolicy) -> Value {
    json!({
        "ci": match p.ci { CiMode::Approve => "approve", CiMode::AutoReadOnly => "auto_read_only" },
        "labels": p.labels.iter().map(|l| &**l).collect::<Vec<_>>(),
        "extra_paths": p.extra_paths.iter().map(|l| &**l).collect::<Vec<_>>(),
        "extra_human_approvals": p.extra_human_approvals,
        "leases": match p.leases { LeaseEnforce::Advisory => "advisory", LeaseEnforce::AgentsExclusive => "agents_exclusive" },
    })
}

/// The reply to a refused settings command: its leader's status and GitHub's error body.
pub fn refused(status: u16, err: &str) -> Reply {
    match status {
        401 => Reply::error(401, "Bad credentials", "rest"),
        403 => Reply::error(403, "Must have admin rights to this resource.", DOC),
        404 => Reply::not_found(),
        _ if err.contains("NotOwnApp") => {
            Reply::error(422, "graftyard_run and mcp rows name an App the install registered", DOC)
        }
        _ if err.contains("NotAnOrg") => Reply::error(422, "agents settings are an organization's", DOC),
        _ => Reply::error(422, err, DOC),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_parse_only_the_reference_paths() {
        assert_eq!(route("/agents/apps"), Some(Route::Apps));
        assert_eq!(route("/agents/apps/12"), Some(Route::App(AppId(DbId(12)))));
        assert_eq!(route("/agents/apps/x"), None);
        assert_eq!(route("/orgs/acme/agents"), Some(Route::Org("acme")));
        assert_eq!(route("/repos/o/r/agents/policy"), Some(Route::Policy { owner: "o", repo: "r" }));
        assert_eq!(route("/repos/o/r/agents"), None);
    }

    #[test]
    fn bodies_round_trip_and_refuse_what_the_reference_does_not_name() {
        let a =
            parse_app(AppId(DbId(5)), &json!({ "kind": "third_party", "confine": "refs/heads/claude/**" })).unwrap();
        assert_eq!(a.confine.as_ref().map(RefPattern::as_str), Some("refs/heads/claude/**"));
        assert_eq!(parse_app(AppId(DbId(5)), &json!({})).unwrap_err().status, 422);
        assert_eq!(parse_app(AppId(DbId(5)), &json!({ "kind": "robot" })).unwrap_err().status, 422);
        assert_eq!(parse_org(&json!({ "workspace": "in_repo" })).unwrap(), (None, Some(WorkspaceMode::InRepo)));
        assert_eq!(parse_org(&json!({ "code_mode": true })).unwrap_err().status, 422);
        let body = json!({ "ci": "auto_read_only", "labels": ["agent"], "extra_paths": ["build.rs"],
            "extra_human_approvals": 1, "leases": "agents_exclusive" });
        let p = parse_policy(&body).unwrap();
        assert_eq!(policy_json(&p), body);
        assert_eq!(parse_policy(&json!({})).unwrap(), AgentPolicy::default());
        assert_eq!(parse_policy(&json!({ "extra_human_approvals": 300 })).unwrap_err().status, 422);
    }
}
