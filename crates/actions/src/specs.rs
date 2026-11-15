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
use graftyard_types::DbId;

/// The run's jobs, or the first reason the workflow cannot run: a bad matrix, bad labels, or a job whose `needs`
/// names a job that is not in the workflow (the parser refuses cycles).
pub fn job_specs(w: &Workflow) -> Result<Vec<JobSpec>, String> {
    // The workflow's jobs in an order where every need comes first; `needs` are checked acyclic by the parser.
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
    // Each workflow job's positions in the output, which its dependents' `needs` name.
    let mut at: Vec<Vec<usize>> = vec![Vec::new(); w.jobs.len()];
    let mut out: Vec<JobSpec> = Vec::new();
    for i in order {
        let j = &w.jobs[i];
        let labels: Vec<String> = match j.runs_on.as_ref().map(|n| &n.t) {
            Some(T::Str(s)) => vec![s.clone()],
            Some(T::Seq(v)) => v.iter().filter_map(|n| n.str().map(String::from)).collect(),
            _ => vec!["self-hosted".into()],
        };
        let labels = LabelSet::parse(&labels).map_err(|e| format!("{}: runs-on: {e}", j.id))?;
        let needs: SmallVec<[JobId; 4]> = j
            .needs
            .iter()
            .flatten()
            .filter_map(|n| w.jobs.iter().position(|x| &x.id == n))
            .flat_map(|p| at[p].clone())
            .map(|p| JobId(DbId(p as i64)))
            .collect();
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
        let configs = matrix::expand(j.strategy.as_ref(), j.name.str())
            .map_err(|e| format!("{}: {}", j.id, e.join("; ")))?
            .map(|s| s.configurations)
            .unwrap_or_default();
        let name = j.name.str().map_or_else(|| j.id.clone(), String::from);
        // `environment: prod` or `environment: {name: prod, url: ...}`; an expression is not resolved here, so such a
        // job reads no environment's rows [inference: no gate names an expression environment].
        let environment: Option<Box<str>> = j
            .environment
            .as_ref()
            .and_then(|e| e.str().or_else(|| e.get("name")?.str()))
            .filter(|n| !n.contains("${{"))
            .map(Into::into);
        let mut one = |name: String, m: SmallVec<[(Box<str>, Box<str>); 4]>| {
            at[i].push(out.len());
            out.push(JobSpec {
                id: JobId(DbId(0)),
                name: name.into(),
                labels: labels.clone(),
                needs: needs.clone(),
                matrix: m,
                environment: environment.clone(),
                steps: steps.clone(),
            });
        };
        let with_matrix: Vec<_> = configs.iter().filter(|c| c.matrix != matrix::Data::Null).collect();
        if with_matrix.is_empty() {
            one(name.clone(), SmallVec::new());
        }
        for c in with_matrix {
            let matrix::Data::Map(pairs) = &c.matrix else { continue };
            let m = pairs.iter().map(|(k, v)| (k.as_str().into(), v.json().to_string().into())).collect();
            one(c.name.clone().unwrap_or_else(|| name.clone()), m);
        }
    }
    Ok(out)
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
