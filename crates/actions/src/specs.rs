//! A parsed workflow's jobs as `CreateRun`'s `JobSpec`s (`design/SHARED.md`, "Actions runs and jobs"): each matrix
//! configuration its own job, named as the runner's `MatrixBuilder` names it, with `runs-on` as its labels, its steps
//! as the runner's JSON, and `needs` by position in the list (`ActionsOp::CreateRun`), every need before its job.

use smallvec::SmallVec;

use crate::ids::JobId;
use crate::labels::LabelSet;
use crate::runsvc;
use crate::shard::JobSpec;
use crate::workflow::template::{Node, T};
use crate::workflow::{Workflow, matrix};
use graftyard_identity::PermissionSet;
use graftyard_types::DbId;

/// Each job's declared `GITHUB_TOKEN` permissions, by its position in `job_specs` (`ActionsOp::CreateRunWith`): the
/// job's own `permissions:`, else the workflow's (`crate::token::declared`). A job with neither is left out.
pub type Declared = Vec<(u16, PermissionSet)>;

/// The run's jobs, or the first reason the workflow cannot run: a bad matrix, bad labels, or a job whose `needs`
/// names a job that is not in the workflow (the parser refuses cycles).
pub fn job_specs(w: &Workflow) -> Result<Vec<JobSpec>, String> {
    job_specs_declared(w).map(|(j, _)| j)
}

/// `job_specs` and each job's declared permissions, each matrix leg its own position.
pub fn job_specs_declared(w: &Workflow) -> Result<(Vec<JobSpec>, Declared), String> {
    // Each workflow job's positions in the output, which its dependents' `needs` name.
    let mut at: Vec<Vec<usize>> = vec![Vec::new(); w.jobs.len()];
    let mut out: Vec<JobSpec> = Vec::new();
    let mut declared: Declared = Vec::new();
    for i in order(w)? {
        let j = &w.jobs[i];
        let o = ordinary(w, j)?;
        let needs: SmallVec<[JobId; 4]> = needs_at(w, j, &at).into_iter().map(|p| JobId(DbId(p as i64))).collect();
        for (name, m) in o.legs.clone() {
            at[i].push(out.len());
            if let Some(p) = &o.perms {
                declared.push((out.len() as u16, p.clone()));
            }
            out.push(o.spec(name, m, needs.clone()));
        }
    }
    Ok((out, declared))
}

/// The workflow's jobs in an order where every need comes first; `needs` are checked acyclic by the parser.
pub(crate) fn order(w: &Workflow) -> Result<Vec<usize>, String> {
    let mut order: Vec<usize> = Vec::new();
    let mut placed = vec![false; w.jobs.len()];
    while order.len() < w.jobs.len() {
        let before = order.len();
        for (i, j) in w.jobs.iter().enumerate() {
            let ready =
                j.needs.iter().flatten().all(|n| w.jobs.iter().position(|x| &x.id == n).is_some_and(|p| placed[p]));
            if !placed[i] && ready {
                placed[i] = true;
                order.push(i);
            }
        }
        if order.len() == before {
            return Err("a job's needs name a job that is not in the workflow".into());
        }
    }
    Ok(order)
}

/// The positions job `j`'s `needs` name, from each earlier workflow job's positions `at`.
pub(crate) fn needs_at(w: &Workflow, j: &crate::workflow::Job, at: &[Vec<usize>]) -> Vec<usize> {
    j.needs
        .iter()
        .flatten()
        .filter_map(|n| w.jobs.iter().position(|x| &x.id == n))
        .flat_map(|p| at[p].clone())
        .collect()
}

/// What one workflow job's specs share, and its legs: each matrix configuration's name and matrix, or the job's
/// name alone.
pub(crate) struct Ordinary {
    pub labels: LabelSet,
    pub steps: Box<str>,
    pub environment: Option<Box<str>>,
    pub perms: Option<PermissionSet>,
    pub legs: Vec<crate::reusable::Leg>,
}

impl Ordinary {
    pub fn spec(
        &self,
        name: String,
        matrix: SmallVec<[(Box<str>, Box<str>); 4]>,
        needs: SmallVec<[JobId; 4]>,
    ) -> JobSpec {
        JobSpec {
            id: JobId(DbId(0)),
            name: name.into(),
            labels: self.labels.clone(),
            needs,
            matrix,
            environment: self.environment.clone(),
            steps: self.steps.clone(),
        }
    }
}

/// Job `j` of `w` as its specs: labels, steps with `env` merged, environment, declared permissions, and legs.
pub(crate) fn ordinary(w: &Workflow, j: &crate::workflow::Job) -> Result<Ordinary, String> {
    let perms =
        crate::token::declared(w.permissions.as_ref(), j.permissions.as_ref()).map_err(|e| format!("{}: {e}", j.id))?;
    let labels: Vec<String> = match j.runs_on.as_ref().map(|n| &n.t) {
        Some(T::Str(s)) => vec![s.clone()],
        Some(T::Seq(v)) => v.iter().filter_map(|n| n.str().map(String::from)).collect(),
        _ => vec!["self-hosted".into()],
    };
    let labels = LabelSet::parse(&labels).map_err(|e| format!("{}: runs-on: {e}", j.id))?;
    // The job message carries no `environmentVariables`, so the workflow's and the job's `env` reach the runner
    // as each step's own: workflow, then job, then the step's keys, a later key replacing an earlier one, which
    // is the runner's order (decisions/DECISIONS.md, "M4-WP3: job and workflow `env` ride on each step").
    let steps: Vec<_> = j
        .steps
        .iter()
        .map(|s| {
            let mut s = s.clone();
            s.env = [w.env.as_ref(), j.env.as_ref(), s.env.as_ref()].into_iter().flatten().fold(None, merge_env);
            s
        })
        .collect();
    let steps: Box<str> = runsvc::steps_json(&steps).to_string().into();
    // `environment: prod` or `environment: {name: prod, url: ...}`; an expression is not resolved here, so such a
    // job reads no environment's rows [inference: no gate names an expression environment].
    let environment: Option<Box<str>> = j
        .environment
        .as_ref()
        .and_then(|e| e.str().or_else(|| e.get("name")?.str()))
        .filter(|n| !n.contains("${{"))
        .map(Into::into);
    Ok(Ordinary { labels, steps, environment, perms, legs: legs(j)? })
}

/// Job `j`'s legs: each matrix configuration's name and matrix, named as the runner's `MatrixBuilder` names it, or
/// the job's name alone.
pub(crate) fn legs(j: &crate::workflow::Job) -> Result<Vec<crate::reusable::Leg>, String> {
    let configs = matrix::expand(j.strategy.as_ref(), j.name.str())
        .map_err(|e| format!("{}: {}", j.id, e.join("; ")))?
        .map(|s| s.configurations)
        .unwrap_or_default();
    let name = j.name.str().map_or_else(|| j.id.clone(), String::from);
    let with_matrix: Vec<_> = configs.iter().filter(|c| c.matrix != matrix::Data::Null).collect();
    if with_matrix.is_empty() {
        return Ok(vec![(name, SmallVec::new())]);
    }
    Ok(with_matrix
        .into_iter()
        .filter_map(|c| {
            let matrix::Data::Map(pairs) = &c.matrix else { return None };
            let m = pairs.iter().map(|(k, v)| (k.as_str().into(), v.json().to_string().into())).collect();
            Some((c.name.clone().unwrap_or_else(|| name.clone()), m))
        })
        .collect())
}

/// `inner`'s keys over `outer`'s. An `env` that is one expression, not a mapping, cannot be merged by key, so the
/// innermost one wins whole.
fn merge_env(outer: Option<Node>, inner: &Node) -> Option<Node> {
    let (Some(T::Map(o)), T::Map(i)) = (outer.as_ref().map(|n| &n.t), &inner.t) else { return Some(inner.clone()) };
    let mut m: Vec<(Node, Node)> =
        o.iter().filter(|(k, _)| !i.iter().any(|(ik, _)| ik.str() == k.str())).cloned().collect();
    m.extend(i.iter().cloned());
    Some(Node { t: T::Map(m), ..inner.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matrix_job_and_its_dependent_need_every_leg() {
        let src = "on: push\njobs:\n  report:\n    needs: build\n    runs-on: [self-hosted, linux]\n    steps:\n      - run: echo done\n  build:\n    runs-on: ubuntu-latest\n    strategy:\n      matrix:\n        os: [one, two]\n    steps:\n      - run: echo ${{ matrix.os }}\n";
        let p = crate::workflow::parse("ci.yml", src);
        assert!(p.ok(), "{:?}", p.errors);
        let specs = job_specs(&p.workflow).unwrap();
        let names: Vec<&str> = specs.iter().map(|s| &*s.name).collect();
        // `report` is written first but needs both legs of `build`, so it comes after them and needs positions 0, 1.
        assert_eq!(names, ["build (one)", "build (two)", "report"]);
        assert_eq!(&specs[2].needs[..], &[JobId(DbId(0)), JobId(DbId(1))]);
        assert!(specs[0].needs.is_empty());
        assert_eq!(&*specs[0].matrix[0].1, "\"one\"");
        assert_eq!(specs[0].environment, None);
        assert!(specs[2].labels.fits(&LabelSet::parse(["self-hosted", "linux", "x64"]).unwrap()));
    }

    /// `gh-token`: each job's declared permissions sit at its own position, every matrix leg of a job included, and a
    /// job's own `permissions:` wins over the workflow's; a job with neither declares none.
    #[test]
    fn each_jobs_declared_permissions_sit_at_its_own_positions() {
        use graftyard_identity::Access;
        let src = "on: push\npermissions: read-all\njobs:\n  last:\n    needs: legs\n    runs-on: x\n    permissions: {issues: write}\n    steps: [{run: a}]\n  legs:\n    runs-on: x\n    strategy:\n      matrix:\n        n: [1, 2]\n    steps: [{run: b}]\n";
        let p = crate::workflow::parse("ci.yml", src);
        assert!(p.ok(), "{:?}", p.errors);
        let (specs, d) = job_specs_declared(&p.workflow).unwrap();
        let names: Vec<&str> = specs.iter().map(|s| &*s.name).collect();
        assert_eq!(names, ["legs (1)", "legs (2)", "last"]);
        let at: Vec<u16> = d.iter().map(|(i, _)| *i).collect();
        assert_eq!(at, [0, 1, 2]);
        assert_eq!(d[0].1.0.get("contents"), Some(&Access::Read), "a leg takes the workflow's");
        assert_eq!(d[1].1, d[0].1);
        assert_eq!((d[2].1.0.get("issues"), d[2].1.0.get("contents")), (Some(&Access::Write), None), "the job's own");
        let none = crate::workflow::parse("ci.yml", "on: push\njobs:\n  a:\n    runs-on: x\n    steps: [{run: a}]\n");
        assert_eq!(job_specs_declared(&none.workflow).unwrap().1, Vec::new());
    }

    /// The environment a job's secrets come from, in both of GitHub's forms; an expression names none.
    #[test]
    fn a_jobs_environment_is_its_name_in_either_form() {
        let src = "on: push\njobs:\n  a:\n    runs-on: x\n    environment: prod\n    steps: [{run: a}]\n  b:\n    runs-on: x\n    environment:\n      name: staging\n      url: https://x\n    steps: [{run: b}]\n  c:\n    runs-on: x\n    environment: ${{ inputs.env }}\n    steps: [{run: c}]\n";
        let p = crate::workflow::parse("ci.yml", src);
        assert!(p.ok(), "{:?}", p.errors);
        let envs: Vec<_> = job_specs(&p.workflow).unwrap().into_iter().map(|s| s.environment).collect();
        assert_eq!(envs, [Some("prod".into()), Some("staging".into()), None]);
    }

    /// BF-37 run 1: a job-level `env` never reached the runner. Each step's environment is workflow, then job, then
    /// step keys, the innermost value winning.
    #[test]
    fn workflow_and_job_env_reach_each_step_innermost_wins() {
        let src = "on: push\nenv:\n  A: w\n  B: w\njobs:\n  j:\n    runs-on: x\n    env:\n      B: j\n      C: j\n    steps:\n      - run: one\n        env:\n          C: s\n      - run: two\n";
        let p = crate::workflow::parse("ci.yml", src);
        assert!(p.ok(), "{:?}", p.errors);
        let steps: serde_json::Value = serde_json::from_str(&job_specs(&p.workflow).unwrap()[0].steps).unwrap();
        let env = |i: usize| -> Vec<(String, String)> {
            let mut v: Vec<_> = steps[i]["environment"]["map"]
                .as_array()
                .unwrap()
                .iter()
                .map(|kv| (kv["Key"].as_str().unwrap().into(), kv["Value"].as_str().unwrap().into()))
                .collect();
            v.sort();
            v
        };
        let kv = |a: &[(&str, &str)]| a.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
        assert_eq!(env(0), kv(&[("A", "w"), ("B", "j"), ("C", "s")]));
        assert_eq!(env(1), kv(&[("A", "w"), ("B", "j"), ("C", "j")]));
    }
}
