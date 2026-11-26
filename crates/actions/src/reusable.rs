//! Reusable workflows (`design/SHARED.md`, "Actions", Workflow engine; `decisions/DECISIONS.md`, "`reusable`"): a
//! job-level `uses:` is flattened into the run when the run is created, as GitHub shows a called workflow's jobs as
//! jobs of the caller's run, named `caller / called`.
//!
//! - **Fetch** (`fetch`). The host reads each called file at its ref, pinned to the commit it resolved, through the
//!   closure it passes: `./.github/workflows/x.yml` is the calling file's repository at the calling file's commit,
//!   and `owner/repo/.github/workflows/x.yml@ref` a full commit SHA, else a tag, else a branch [doc: GHES 3.22,
//!   "Reuse workflows": "If a release tag and a branch have the same name, the release tag takes precedence"].
//! - **Expand** (`expand`). Each called job becomes one spec per matrix leg of its own and of its caller. A called job
//!   with no `needs` of its own waits for its caller's `needs`, and a job that needs the caller needs every called job.
//!   At most `LEVELS_MAX` levels, `UNIQUE_MAX` distinct called files, no loop [doc: GHES 3.22, "Reusing workflow
//!   configurations", Limitations; "Reuse workflows", Nesting]. A nested job's permissions stay within its caller's
//!   [doc: same, "permissions can only be maintained or reduced"].
//! - **The plan** (`RunPlan`, format 0x15.9) records what the runner needs later and the specs do not hold: each job's
//!   key in its workflow, its call frame, and its `outputs:` token; each frame's `with:`, the called file's inputs,
//!   its secrets mapping or `inherit`, its caller's `needs` and matrix leg, its `on.workflow_call.outputs`, and the
//!   file's repository, path, ref and commit, which the called jobs' ID tokens name as `job_workflow_ref`.
//! - **Contexts** (`contexts`), at `acquirejob`: `needs` from the needed jobs' conclusions and committed outputs,
//!   a called job's `inputs` from its frame's `with:` over its caller's contexts, and its `secrets` from its frame's
//!   mapping over its caller's secrets, so a chain A > B > C passes C only what B passed it [doc: "Reuse workflows",
//!   Passing secrets to nested workflows].

use std::collections::BTreeMap;
use std::future::Future;

use graftyard_git::refs::{ObjectId, RefName};
use graftyard_identity::PermissionSet;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use smallvec::SmallVec;

use crate::ids::JobId;
use crate::job::Conclusion;
use crate::shard::{ActionsFold, JobSpec, MatrixValue};
use crate::specs::{Declared, needs_at, order, ordinary};
use crate::workflow::{self, Call, Workflow, expr};
use graftyard_types::DbId;

/// Levels of workflows, the caller included [doc: GHES 3.22, "Reuse workflows": "a maximum of four levels of
/// workflows - that is, the top-level caller workflow and up to three levels of reusable workflows"].
pub const LEVELS_MAX: usize = 4;
/// Distinct reusable workflows one file's tree calls [doc: GHES 3.22, "Reusing workflow configurations": "a maximum
/// of 20 unique reusable workflows from a single workflow file"].
pub const UNIQUE_MAX: usize = 20;
/// A job's outputs, in total, at most [doc: GHES 3.22, "Workflow syntax", `jobs.<job_id>.outputs`: "Outputs are
/// Unicode strings, and can be a maximum of 1 MB"]. Past it the job's outputs are not recorded.
pub const OUTPUTS_MAX: usize = 1 << 20;

/// A job's outputs, name and value.
pub type Outputs = Vec<(Box<str>, Box<str>)>;
/// One leg of a job: its name and its matrix.
pub type Leg = (String, SmallVec<[(Box<str>, MatrixValue); 4]>);

/// What a run records beside its jobs when a workflow calls another or a job declares `outputs:` (0x15.9).
/// `jobs` is parallel to the run's jobs, in spec order; positions name jobs of the run by that order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPlan {
    pub jobs: Vec<JobPlan>,
    pub calls: Vec<CallFrame>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobPlan {
    /// The job's id in its own workflow file, which `needs.<key>` and `jobs.<key>` name.
    pub key: Box<str>,
    /// The call the job runs in, by position in `RunPlan::calls`; `None` for the caller's own jobs.
    pub frame: Option<u16>,
    /// The job's `outputs:` as template-token JSON, which the runner evaluates (`JobOutputs`).
    pub outputs: Option<Box<str>>,
}

/// One caller job's matrix leg's call of a reusable workflow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallFrame {
    /// The caller job's id in the calling file.
    pub key: Box<str>,
    /// The frame the caller job runs in.
    pub parent: Option<u16>,
    /// The called file's repository, `owner/repo`; `None` is the run's own repository.
    pub repo: Option<Box<str>>,
    pub path: Box<str>,
    /// `refs/tags/v1`, `refs/heads/main`, or a full SHA, as `job_workflow_ref` names it.
    pub r#ref: Box<str>,
    pub sha: [u8; 20],
    /// The caller's `with:`, template-token JSON.
    pub with: Option<Box<str>>,
    /// The called file's `on.workflow_call.inputs`, as the parser converts them (type, default, required).
    pub inputs: Box<str>,
    pub inherit: bool,
    /// The caller's `secrets:` mapping, template-token JSON.
    pub secrets: Option<Box<str>>,
    /// The caller job's `needs`, by position.
    pub needs: Vec<u16>,
    /// The caller job's matrix leg.
    pub matrix: SmallVec<[(Box<str>, MatrixValue); 4]>,
    /// The called file's `on.workflow_call.outputs`, template-token JSON.
    pub outputs: Option<Box<str>>,
}

impl CallFrame {
    /// `job_workflow_ref`: `owner/repo/path@ref`, `repository` being the run's for a call in its own repository.
    pub fn workflow_ref(&self, repository: &str) -> String {
        format!("{}/{}@{}", self.repo.as_deref().unwrap_or(repository), self.path, self.r#ref)
    }
}

/// Where a workflow file was read: its repository (`None`, the run's own), path, ref, and commit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Origin {
    pub repo: Option<String>,
    pub path: String,
    pub r#ref: String,
    pub sha: [u8; 20],
}

impl Origin {
    fn key(&self) -> String {
        let hex: String = self.sha.iter().map(|b| format!("{b:02x}")).collect();
        format!("{}\n{}\n{hex}", self.repo.as_deref().unwrap_or("").to_ascii_lowercase(), self.path)
    }
}

/// A called file the host reads: in `repo` (`None`, the run's own) at `path`, at a pinned commit or a version the
/// host resolves (`resolve_version`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Want {
    pub repo: Option<String>,
    pub path: String,
    pub version: Version,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Version {
    At { r#ref: String, sha: [u8; 20] },
    Named(String),
}

/// What the host read for a `Want`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub origin: Origin,
    pub src: String,
}

/// A `uses:` as the file it names, from the file `from` that calls it. A path is `.github/workflows/<file>.yml` or
/// `.yaml`, with no subdirectory [doc: "Reuse workflows": "Subdirectories of the workflows directory are not
/// supported"].
pub fn want(uses: &str, from: &Origin) -> Result<Want, String> {
    let bad = || format!("invalid workflow reference '{uses}'");
    let file_ok = |p: &str| {
        p.strip_prefix(".github/workflows/")
            .is_some_and(|f| !f.is_empty() && !f.contains('/') && (f.ends_with(".yml") || f.ends_with(".yaml")))
    };
    if let Some(path) = uses.strip_prefix("./") {
        if path.contains('@') || !file_ok(path) {
            return Err(bad());
        }
        let version = Version::At { r#ref: from.r#ref.clone(), sha: from.sha };
        return Ok(Want { repo: from.repo.clone(), path: path.into(), version });
    }
    let (left, version) = uses.split_once('@').ok_or_else(bad)?;
    let mut parts = left.splitn(3, '/');
    let (Some(owner), Some(repo), Some(path)) = (parts.next(), parts.next(), parts.next()) else { return Err(bad()) };
    if owner.is_empty() || repo.is_empty() || version.is_empty() || version.starts_with("refs/") || !file_ok(path) {
        return Err(bad());
    }
    Ok(Want { repo: Some(format!("{owner}/{repo}")), path: path.into(), version: Version::Named(version.into()) })
}

/// A named version as the ref `job_workflow_ref` names and its commit: a full SHA as itself (the host checks the
/// commit exists), else a tag, else a branch.
pub fn resolve_version(v: &str, ref_value: impl Fn(&RefName) -> Option<ObjectId>) -> Option<(String, ObjectId)> {
    if let Some(sha) = ObjectId::parse_hex(v.to_ascii_lowercase().as_bytes()) {
        return Some((sha.hex(), sha));
    }
    ["refs/tags/", "refs/heads/"].iter().find_map(|p| {
        let n = RefName::parse(format!("{p}{v}").as_bytes()).ok()?;
        ref_value(&n).map(|sha| (format!("{p}{v}"), sha))
    })
}

/// The ref and commit a call into another repository pins (`want`'s `Version::Named` `version`), from the rows a host
/// read for it: `core` holds its `settings` and `collaborator` rows and the `ref` rows the version names, as a narrow
/// `tarball/{version}` read answers them. Refused unless an anonymous caller may read the repository, as the edge's
/// dispatch and the launch service read another repository (`decisions/OPEN-QUESTIONS.md`, F90), then pinned by
/// `resolve_version`. The host then reads the file at the commit, so a full SHA that names no commit fails there.
pub fn pin_other(
    name: &str,
    version: &Version,
    repo: graftyard_types::RepoId,
    owner: graftyard_types::AccountId,
    core: &dyn graftyard_fold::Read,
) -> Result<(String, [u8; 20]), String> {
    let access = graftyard_git::issue_ops::Access { repo, owner, core };
    if graftyard_identity::authorize_read(None, None, &[], &access) != graftyard_identity::Decision::Allow {
        return Err(format!("{name}: not found or not readable"));
    }
    let v = match version {
        Version::At { r#ref, sha } => return Ok((r#ref.clone(), *sha)),
        Version::Named(v) => v,
    };
    let at = |n: &RefName| graftyard_git::tiers::get_rec(core, "ref", &[graftyard_git::tiers::ref_key(n)]);
    let (r, sha) = resolve_version(v, at).ok_or_else(|| format!("{name}@{v}: no such version"))?;
    Ok((r, sha.0))
}

/// A repository other than the run's that a call names, as the host read it for `pin_other`: its id and owner, the
/// rows of a narrow `tarball/{version}` read, and what the host needs to read its git cache (`at`).
pub struct Other<A> {
    pub id: graftyard_types::RepoId,
    pub owner: graftyard_types::AccountId,
    pub core: graftyard_fold::MemStore,
    pub at: A,
}

/// What a host reads for `called_file`.
#[allow(async_fn_in_trait, reason = "one host per crate, called on its own thread, never boxed")]
pub trait CallReads {
    type At;
    /// `owner/name` resolved by name on the install and read for `version` (`Other`), `None` when no such repository.
    async fn other(&self, owner: &str, name: &str, version: &str) -> Result<Option<Other<Self::At>>, String>;
    /// The text of `path` at commit `sha` in `repo` (`None`, the run's own) from its git cache, `None` when it is not
    /// there.
    async fn blob(&self, repo: Option<&Other<Self::At>>, sha: [u8; 20], path: &str) -> Result<Option<String>, String>;
}

/// A called workflow file for a host's plan, as the edge's dispatch reads one (F132): `./` in the run's repository at
/// the caller's commit, or `owner/repo` resolved on the install and read as an anonymous caller (`pin_other`), its
/// version a full SHA, else a tag, else a branch, then the file at the pinned commit.
pub async fn called_file<R: CallReads>(reads: &R, w: Want) -> Result<Fetched, String> {
    let other;
    let (at, (r#ref, sha)) = match (&w.repo, &w.version) {
        (None, Version::At { r#ref, sha }) => (None, (r#ref.clone(), *sha)),
        (Some(name), version) => {
            let (o, n) = name.split_once('/').ok_or("owner/repo")?;
            let v = match version {
                Version::At { sha, .. } => ObjectId(*sha).hex(),
                Version::Named(v) => v.clone(),
            };
            other = reads.other(o, n, &v).await?.ok_or_else(|| format!("{name}: not found or not readable"))?;
            let pinned = pin_other(name, version, other.id, other.owner, &other.core)?;
            (Some(&other), pinned)
        }
        (None, Version::Named(_)) => return Err("a call in this repository names no version".into()),
    };
    let src = reads.blob(at, sha, &w.path).await?.ok_or_else(|| format!("{}: not found", w.path))?;
    Ok(Fetched { origin: Origin { repo: w.repo, path: w.path, r#ref, sha }, src })
}

/// The called files a workflow's tree needs, read once each.
#[derive(Default)]
pub struct Called {
    files: Vec<(Origin, Workflow)>,
    /// (the calling file's key, its `uses:`) to the file it named.
    by: BTreeMap<(String, String), usize>,
}

impl Called {
    fn find(&self, from: &Origin, uses: &str) -> Option<&(Origin, Workflow)> {
        self.by.get(&(from.key(), uses.to_string())).map(|i| &self.files[*i])
    }
}

/// Every file `top`'s tree calls, read through `get`, breadth first and each once. Stops descending at
/// `LEVELS_MAX`, so a loop or a deeper tree reads no more than that and `expand` names the fault.
pub async fn fetch<F, Fut>(top: &Workflow, origin: &Origin, mut get: F) -> Result<Called, String>
where
    F: FnMut(Want) -> Fut,
    Fut: Future<Output = Result<Fetched, String>>,
{
    let mut called = Called::default();
    let mut queue: std::collections::VecDeque<(Option<usize>, usize)> = [(None, 1)].into();
    while let Some((at, depth)) = queue.pop_front() {
        let (from, w) = match at {
            None => (origin.clone(), top),
            Some(i) => (called.files[i].0.clone(), &called.files[i].1),
        };
        let uses: Vec<String> = w.jobs.iter().filter_map(|j| j.call.as_ref().map(|c| c.uses.clone())).collect();
        if depth >= LEVELS_MAX {
            continue;
        }
        for u in uses {
            let k = (from.key(), u.clone());
            if called.by.contains_key(&k) {
                continue;
            }
            let got = get(want(&u, &from)?).await.map_err(|e| format!("{u}: {e}"))?;
            if let Some(i) = called.files.iter().position(|(o, _)| o.key() == got.origin.key()) {
                called.by.insert(k, i);
                continue;
            }
            if called.files.len() == UNIQUE_MAX {
                return Err(format!("{u}: more than {UNIQUE_MAX} reusable workflows in one workflow's tree"));
            }
            let p = workflow::parse(&got.origin.path, &got.src);
            if !p.ok() {
                return Err(format!("{u}: {}", p.errors.join("; ")));
            }
            called.files.push((got.origin, p.workflow));
            called.by.insert(k, called.files.len() - 1);
            queue.push_back((Some(called.files.len() - 1), depth + 1));
        }
    }
    Ok(called)
}

/// A run's jobs, their declared permissions by position, and the plan when the run needs one.
#[derive(Clone, Debug, PartialEq)]
pub struct Expanded {
    pub jobs: Vec<JobSpec>,
    pub declared: Declared,
    pub plan: Option<RunPlan>,
}

/// `top`'s jobs with every call flattened from `called`; the first reason the tree cannot run otherwise.
pub fn expand(top: &Workflow, origin: &Origin, called: &Called) -> Result<Expanded, String> {
    let mut b = Build { jobs: Vec::new(), declared: Vec::new(), plan: RunPlan::default() };
    let mut ancestry = vec![origin.key()];
    b.add(
        top,
        origin,
        &Frame { at: None, needs: &[], inherited: None, cap: None, prefix: "" },
        &mut ancestry,
        1,
        called,
    )?;
    let needs_plan = !b.plan.calls.is_empty() || b.plan.jobs.iter().any(|j| j.outputs.is_some());
    Ok(Expanded { jobs: b.jobs, declared: b.declared, plan: needs_plan.then_some(b.plan) })
}

struct Build {
    jobs: Vec<JobSpec>,
    declared: Declared,
    plan: RunPlan,
}

/// The call a file's jobs are added under.
struct Frame<'a> {
    at: Option<u16>,
    /// Its caller's needs, which a job with none of its own waits for.
    needs: &'a [usize],
    /// The caller's declared permissions, which a job declaring none takes.
    inherited: Option<&'a PermissionSet>,
    /// The caller's permissions, which no job here may pass.
    cap: Option<&'a PermissionSet>,
    prefix: &'a str,
}

impl Build {
    /// Adds `w`'s jobs under frame `fr`; every position added, in order.
    fn add(
        &mut self,
        w: &Workflow,
        origin: &Origin,
        fr: &Frame<'_>,
        ancestry: &mut Vec<String>,
        depth: usize,
        called: &Called,
    ) -> Result<Vec<usize>, String> {
        let mut at: Vec<Vec<usize>> = vec![Vec::new(); w.jobs.len()];
        let mut all = Vec::new();
        for i in order(w)? {
            let j = &w.jobs[i];
            let mut needs = needs_at(w, j, &at);
            if needs.is_empty() {
                needs = fr.needs.to_vec();
            }
            let declared = crate::token::declared(w.permissions.as_ref(), j.permissions.as_ref())
                .map_err(|e| format!("{}: {e}", j.id))?
                .or_else(|| fr.inherited.cloned());
            if let (Some(d), Some(cap)) = (&declared, fr.cap)
                && !d.within(cap)
            {
                return Err(format!("{}: a nested job asks for permissions its caller does not grant", j.id));
            }
            let Some(c) = &j.call else {
                let o = ordinary(w, j)?;
                let ids: SmallVec<[JobId; 4]> = needs.iter().map(|p| JobId(DbId(*p as i64))).collect();
                for (name, m) in o.legs.clone() {
                    let pos = self.jobs.len();
                    at[i].push(pos);
                    if let Some(p) = &declared {
                        self.declared.push((pos as u16, p.clone()));
                    }
                    self.jobs.push(o.spec(format!("{}{name}", fr.prefix), m, ids.clone()));
                    let outputs = j.outputs.as_ref().map(|n| n.json().to_string().into());
                    self.plan.jobs.push(JobPlan { key: j.id.as_str().into(), frame: fr.at, outputs });
                }
                all.extend(at[i].iter().copied());
                continue;
            };
            if depth >= LEVELS_MAX {
                return Err(format!("{}: more than {LEVELS_MAX} levels of workflows", c.uses));
            }
            let (co, cw) =
                called.find(origin, &c.uses).ok_or_else(|| format!("{}: the workflow was not read", c.uses))?;
            if ancestry.contains(&co.key()) {
                return Err(format!("{}: a loop in the workflow tree", c.uses));
            }
            let inputs = check_call(c, cw)?;
            let cap = declared.clone().unwrap_or_else(crate::token::default_permissions);
            let pin = |n: &Option<workflow::template::Node>| n.as_ref().map(|n| n.json().to_string().into());
            for (leg, m) in crate::specs::legs(j)? {
                let f = u16::try_from(self.plan.calls.len()).map_err(|_| "too many calls")?;
                self.plan.calls.push(CallFrame {
                    key: j.id.as_str().into(),
                    parent: fr.at,
                    repo: co.repo.as_deref().map(Into::into),
                    path: co.path.as_str().into(),
                    r#ref: co.r#ref.as_str().into(),
                    sha: co.sha,
                    with: pin(&c.with),
                    inputs: inputs.to_string().into(),
                    inherit: c.inherit_secrets,
                    secrets: pin(&c.secrets),
                    needs: needs.iter().map(|p| *p as u16).collect(),
                    matrix: m,
                    outputs: pin(&cw.call_outputs),
                });
                ancestry.push(co.key());
                let prefix = format!("{}{leg} / ", fr.prefix);
                let sub = Frame {
                    at: Some(f),
                    needs: &needs,
                    inherited: declared.as_ref(),
                    cap: Some(&cap),
                    prefix: &prefix,
                };
                let added = self.add(cw, co, &sub, ancestry, depth + 1, called);
                ancestry.pop();
                at[i].extend(added?);
            }
            all.extend(at[i].iter().copied());
        }
        Ok(all)
    }
}

/// A call checked against the file it names, as GitHub fails the run: the file takes `workflow_call`, every `with:`
/// key is a declared input, every required input without a default is given, and an explicit `secrets:` names only
/// declared secrets and every required one. The called file's declared inputs.
fn check_call(c: &Call, cw: &Workflow) -> Result<Value, String> {
    let ev = cw
        .events
        .as_ref()
        .and_then(|e| e.get("workflow_call"))
        .ok_or_else(|| format!("{}: not a reusable workflow, its on: takes no workflow_call", c.uses))?;
    let mut inputs = ev.get("inputs").cloned().unwrap_or_else(|| json!({}));
    // A `type: number` input and its literal default, which the oracle's JSON leaves out.
    for (k, d) in cw.call_inputs.as_ref().and_then(|n| n.map()).into_iter().flatten() {
        if let (Some(k), Some("number")) = (k.str(), d.get("type").and_then(|t| t.str()))
            && let Some(o) = inputs.get_mut(k).and_then(Value::as_object_mut)
        {
            o.insert("type".into(), Value::from("number"));
            if let Some(workflow::template::T::Num(n)) = d.get("default").map(|n| &n.t) {
                o.insert("default".into(), expr::num(*n));
            }
        }
    }
    let keys = |n: &Option<workflow::template::Node>| -> Vec<String> {
        n.as_ref().and_then(|n| n.map()).into_iter().flatten().filter_map(|(k, _)| k.str().map(String::from)).collect()
    };
    let with = keys(&c.with);
    let declared = inputs.as_object().cloned().unwrap_or_default();
    for k in &with {
        if !declared.keys().any(|d| d.eq_ignore_ascii_case(k)) {
            return Err(format!("{}: invalid input, {k} is not defined in the referenced workflow", c.uses));
        }
    }
    for (name, d) in &declared {
        if d["required"] == json!(true)
            && d.get("default").is_none()
            && !with.iter().any(|k| k.eq_ignore_ascii_case(name))
        {
            return Err(format!("{}: input {name} is required, but not provided", c.uses));
        }
    }
    if !c.inherit_secrets {
        let declared = ev.get("secrets").and_then(Value::as_object).cloned().unwrap_or_default();
        let given = keys(&c.secrets);
        for k in &given {
            if !declared.keys().any(|d| d.eq_ignore_ascii_case(k)) {
                return Err(format!("{}: invalid secret, {k} is not defined in the referenced workflow", c.uses));
            }
        }
        for (name, d) in &declared {
            if d["required"] == json!(true) && !given.iter().any(|k| k.eq_ignore_ascii_case(name)) {
                return Err(format!("{}: secret {name} is required, but not provided", c.uses));
            }
        }
    }
    Ok(inputs)
}

// ---- contexts at acquirejob

/// What a planned run's job is given beyond the rest of its message: the `needs` and `inputs` contexts (plain JSON),
/// its secrets, and its `outputs:` token for the runner to evaluate.
#[derive(Clone, Debug, PartialEq)]
pub struct JobContexts {
    pub needs: Value,
    pub inputs: Value,
    pub secrets: Vec<(String, String)>,
    pub outputs: Option<Value>,
}

/// `job`'s contexts, when its run has a plan: `github` is the job's `github` context as plain JSON, `vars` and
/// `secrets` what the host opened for the run's repository and the job's environment.
pub fn contexts(
    f: &ActionsFold,
    job: JobId,
    github: &Value,
    vars: &[(String, String)],
    secrets: &[(String, String)],
) -> Option<JobContexts> {
    let row = f.job(job)?;
    let run = f.run(row.run)?;
    let plan = f.run_plan(row.run)?;
    let pos = run.jobs.iter().position(|j| *j == job)?;
    let jp = plan.jobs.get(pos)?;
    let r = Reader { f, ids: &run.jobs, plan, github, vars, top: secrets };
    let needs: Vec<usize> = row.spec.needs.iter().filter_map(|n| r.ids.iter().position(|x| x == n)).collect();
    Some(JobContexts {
        needs: r.results(jp.frame, &needs),
        inputs: jp.frame.map_or_else(|| json!({}), |fr| r.inputs(fr)),
        secrets: jp.frame.map_or_else(|| secrets.to_vec(), |fr| r.secrets(fr)),
        outputs: jp.outputs.as_deref().and_then(|o| serde_json::from_str(o).ok()),
    })
}

/// The call `job` runs in, innermost first, whose file its ID token names (`job_workflow_ref`).
pub fn job_frame(f: &ActionsFold, job: JobId) -> Option<&CallFrame> {
    let row = f.job(job)?;
    let plan = f.run_plan(row.run)?;
    let pos = f.run(row.run)?.jobs.iter().position(|j| *j == job)?;
    plan.calls.get(usize::from(plan.jobs.get(pos)?.frame?))
}

struct Reader<'a> {
    f: &'a ActionsFold,
    ids: &'a [JobId],
    plan: &'a RunPlan,
    github: &'a Value,
    vars: &'a [(String, String)],
    top: &'a [(String, String)],
}

fn word(c: Option<Conclusion>) -> &'static str {
    match c {
        Some(Conclusion::Success) => "success",
        Some(Conclusion::Cancelled) => "cancelled",
        Some(Conclusion::Skipped) => "skipped",
        Some(_) => "failure",
        None => "",
    }
}

/// A group of jobs' result, as `needs.<id>.result` reads a matrix job's: `failure` if any failed, else `cancelled`
/// if any was, else `skipped` if all were, else `success`.
fn result(cs: &[Option<Conclusion>]) -> &'static str {
    let ws: Vec<&str> = cs.iter().map(|c| word(*c)).collect();
    for w in ["failure", "", "cancelled"] {
        if ws.contains(&w) {
            return w;
        }
    }
    if ws.iter().all(|w| *w == "skipped") { "skipped" } else { "success" }
}

/// Later legs' non-empty outputs over earlier ones, the legs in completion order [doc: "Reuse workflows": a matrix
/// call's output is "the output set by the last successful completing reusable workflow of the matrix which actually
/// sets a value"].
fn merge(into: &mut Map<String, Value>, from: Map<String, Value>) {
    for (k, v) in from {
        if v.as_str() != Some("") && !v.is_null() {
            into.insert(k, v);
        }
    }
}

impl Reader<'_> {
    fn frame_of(&self, p: usize) -> Option<u16> {
        self.plan.jobs.get(p).and_then(|j| j.frame)
    }

    /// Position `p`'s name at level `level` (a frame, or the caller's own jobs): its key when it runs at that level,
    /// or the key of the call at that level it runs under, with that call's frame; `None` when it is not under it.
    fn key_at(&self, p: usize, level: Option<u16>) -> Option<(&str, Option<u16>)> {
        let mut fr = self.frame_of(p);
        if fr == level {
            return Some((&self.plan.jobs[p].key, None));
        }
        while let Some(f) = fr {
            let c = self.plan.calls.get(usize::from(f))?;
            if c.parent == level {
                return Some((&c.key, Some(f)));
            }
            fr = c.parent;
        }
        None
    }

    /// The `needs` (or `jobs`) context at `level` over positions `ps`: each key's `result` and `outputs`.
    fn results(&self, level: Option<u16>, ps: &[usize]) -> Value {
        let mut groups: Vec<(String, Vec<usize>, Vec<u16>)> = Vec::new();
        for p in ps {
            let Some((key, frame)) = self.key_at(*p, level) else { continue };
            let g = match groups.iter().position(|g| g.0 == key) {
                Some(i) => &mut groups[i],
                None => {
                    groups.push((key.into(), Vec::new(), Vec::new()));
                    groups.last_mut().expect("pushed")
                }
            };
            g.1.push(*p);
            if let Some(f) = frame
                && !g.2.contains(&f)
            {
                g.2.push(f);
            }
        }
        let mut out = Map::new();
        for (key, ps, frames) in groups {
            let cs: Vec<Option<Conclusion>> = ps
                .iter()
                .map(|p| self.ids.get(*p).and_then(|id| self.f.job(*id)).and_then(|j| j.state.completed()))
                .collect();
            let mut outputs = Map::new();
            if frames.is_empty() {
                for p in &ps {
                    let got = self.ids.get(*p).and_then(|id| self.f.job_outputs(*id)).into_iter().flatten();
                    merge(&mut outputs, got.map(|(k, v)| (k.to_string(), Value::from(&**v))).collect());
                }
            } else {
                // The legs in the order they completed, so the last to complete that set a value wins.
                let mut frames: Vec<(u64, u16)> = frames.into_iter().map(|f| (self.completed_at(f), f)).collect();
                frames.sort();
                for (_, f) in frames {
                    merge(&mut outputs, self.call_outputs(f));
                }
            }
            out.insert(key, json!({ "result": result(&cs), "outputs": outputs }));
        }
        Value::Object(out)
    }

    /// When frame `f`'s last job completed (0 while one runs).
    fn completed_at(&self, f: u16) -> u64 {
        let mut last = 0;
        for p in (0..self.plan.jobs.len()).filter(|p| self.key_at(*p, Some(f)).is_some()) {
            match self.ids.get(p).and_then(|id| self.f.job(*id)).map(|j| &j.state) {
                Some(crate::job::JobState::Completed { at, .. }) => last = last.max(at.0),
                _ => return 0,
            }
        }
        last
    }

    /// Frame `f`'s `on.workflow_call.outputs`, each `value` evaluated over the `jobs` it ran.
    fn call_outputs(&self, f: u16) -> Map<String, Value> {
        let c = &self.plan.calls[usize::from(f)];
        let under: Vec<usize> = (0..self.plan.jobs.len()).filter(|p| self.key_at(*p, Some(f)).is_some()).collect();
        let mut ctx = self.base();
        ctx.insert("jobs".into(), self.results(Some(f), &under));
        ctx.insert("inputs".into(), self.inputs(f));
        let defs = c.outputs.as_deref().and_then(|o| serde_json::from_str::<Value>(o).ok()).unwrap_or(Value::Null);
        pairs(&defs)
            .into_iter()
            .map(|(k, d)| {
                let v = pairs(&d).into_iter().find(|(n, _)| n == "value").map(|(_, v)| v).unwrap_or(Value::Null);
                (k, Value::from(expr::coerce_string(&eval(&v, &ctx))))
            })
            .collect()
    }

    /// The contexts every level has: `github` and `vars`.
    fn base(&self) -> Map<String, Value> {
        let mut ctx = Map::new();
        ctx.insert("github".into(), self.github.clone());
        ctx.insert("vars".into(), self.vars.iter().map(|(k, v)| (k.clone(), Value::from(v.as_str()))).collect());
        ctx
    }

    /// The contexts frame `f`'s caller job evaluates its `with:` and `secrets:` in.
    fn caller(&self, f: u16) -> Map<String, Value> {
        let c = &self.plan.calls[usize::from(f)];
        let mut ctx = self.base();
        let needs: Vec<usize> = c.needs.iter().map(|p| usize::from(*p)).collect();
        ctx.insert("needs".into(), self.results(c.parent, &needs));
        ctx.insert("inputs".into(), c.parent.map_or_else(|| json!({}), |p| self.inputs(p)));
        let m: Map<String, Value> =
            c.matrix.iter().map(|(k, v)| (k.to_string(), serde_json::from_str(v).unwrap_or(Value::Null))).collect();
        ctx.insert("matrix".into(), Value::Object(m));
        ctx.insert("strategy".into(), json!({}));
        ctx
    }

    /// Frame `f`'s `inputs`: each declared default, then each `with:` value, typed as declared.
    fn inputs(&self, f: u16) -> Value {
        let c = &self.plan.calls[usize::from(f)];
        let defs: Map<String, Value> = serde_json::from_str(&c.inputs).unwrap_or_default();
        let mut out = Map::new();
        for (k, d) in &defs {
            if let Some(v) = d.get("default") {
                out.insert(k.clone(), v.clone());
            }
        }
        let with = c.with.as_deref().and_then(|w| serde_json::from_str::<Value>(w).ok()).unwrap_or(Value::Null);
        let ctx = self.caller(f);
        for (k, v) in pairs(&with) {
            let (name, ty) = defs
                .iter()
                .find(|(d, _)| d.eq_ignore_ascii_case(&k))
                .map_or((k.clone(), "string"), |(d, x)| (d.clone(), x["type"].as_str().unwrap_or("string")));
            out.insert(name, typed(eval(&v, &ctx), ty));
        }
        Value::Object(out)
    }

    /// Frame `f`'s secrets: all its caller's with `inherit`, else each mapped one evaluated over its caller's
    /// secrets, else none. An empty value is left out, as the runner masks no empty secret.
    fn secrets(&self, f: u16) -> Vec<(String, String)> {
        let c = &self.plan.calls[usize::from(f)];
        let parent = c.parent.map_or_else(|| self.top.to_vec(), |p| self.secrets(p));
        if c.inherit {
            return parent;
        }
        let map = c.secrets.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok()).unwrap_or(Value::Null);
        let mut ctx = self.caller(f);
        ctx.insert("secrets".into(), parent.iter().map(|(k, v)| (k.clone(), Value::from(v.as_str()))).collect());
        pairs(&map)
            .into_iter()
            .map(|(k, v)| (k, expr::coerce_string(&eval(&v, &ctx))))
            .filter(|(_, v)| !v.is_empty())
            .collect()
    }
}

/// A mapping token's pairs, keys as strings.
fn pairs(token: &Value) -> Vec<(String, Value)> {
    token["map"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|kv| Some((kv["Key"].as_str()?.to_string(), kv["Value"].clone())))
        .collect()
}

/// A template token's value over `ctx`: a literal as itself, an expression evaluated. An expression that does not
/// evaluate is null, which the runner shows as an empty string [inference: the runner fails the step instead, and no
/// gate reads that case].
fn eval(token: &Value, ctx: &Map<String, Value>) -> Value {
    let Some(e) = token.as_object().filter(|o| o.get("type") == Some(&json!(3))).and_then(|o| o["expr"].as_str())
    else {
        return if token.is_object() { Value::Null } else { token.clone() };
    };
    let names: Vec<&String> = ctx.keys().collect();
    let allowed = expr::Allowed::new(&names);
    match expr::parse(e, &allowed) {
        Ok(Some(a)) => expr::evaluate(&a, ctx, &|_| None).unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// An input as its declared type: a `boolean` or `number` input given an expression's string (`${{ needs.x.outputs.y
/// }}`, always a string) takes its value, and a `string` input given a number or boolean its text [inference: the
/// runner's own `with:` evaluation types inputs this way; no gate reads a mismatched type].
fn typed(v: Value, ty: &str) -> Value {
    match (ty, &v) {
        ("boolean", Value::String(s)) if s == "true" || s == "false" => Value::Bool(s == "true"),
        ("number", Value::String(s)) => s.trim().parse::<f64>().map_or(v.clone(), expr::num),
        ("string", Value::Bool(_) | Value::Number(_)) => Value::from(expr::coerce_string(&v)),
        _ => v,
    }
}

/// A plan fits a run of `jobs` jobs: one row per job, each frame a call before it, each call's parent before it, and
/// each call's needs inside the run.
pub fn check_plan(plan: &RunPlan, jobs: usize) -> Result<(), String> {
    if plan.jobs.len() != jobs {
        return Err(format!("a plan of {} jobs for a run of {jobs}", plan.jobs.len()));
    }
    let calls = plan.calls.len();
    if plan.jobs.iter().any(|j| j.frame.is_some_and(|f| usize::from(f) >= calls)) {
        return Err("a job's call is not in the plan".into());
    }
    for (i, c) in plan.calls.iter().enumerate() {
        if c.parent.is_some_and(|p| usize::from(p) >= i) || c.needs.iter().any(|n| usize::from(*n) >= jobs) {
            return Err(format!("call {i} names a call or job outside the plan"));
        }
    }
    Ok(())
}

/// Whether a workflow needs a plan: a job calls a reusable workflow or declares `outputs:`.
pub fn needs_plan(w: &Workflow) -> bool {
    w.jobs.iter().any(|j| j.call.is_some() || j.outputs.is_some())
}

/// The run a host creates for workflow `w` read at `origin`, when it needs a plan: its called files read through
/// `get` and its jobs expanded. `Ok(None)` for a workflow that needs none, whose jobs `crate::specs` makes as before,
/// and for one whose jobs only declare `outputs:` where 0x15.9 is not enabled (`enabled`), which runs as it did.
pub async fn plan_run<F, Fut>(w: &Workflow, origin: &Origin, enabled: bool, get: F) -> Result<Option<Expanded>, String>
where
    F: FnMut(Want) -> Fut,
    Fut: Future<Output = Result<Fetched, String>>,
{
    let calls = w.jobs.iter().any(|j| j.call.is_some());
    if !needs_plan(w) || (!enabled && !calls) {
        return Ok(None);
    }
    if !enabled {
        return Err("a reusable workflow needs Actions format 0x15.9, which is not enabled".into());
    }
    let called = fetch(w, origin, get).await?;
    expand(w, origin, &called).map(Some)
}
