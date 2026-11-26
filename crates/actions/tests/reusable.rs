//! BF-72's reusable workflows (`graftyard_actions::reusable`; `decisions/DECISIONS.md`, "`reusable`") on real members:
//! a caller whose job `uses:` a workflow in another repository at a tag, with `with:` inputs, `secrets: inherit`, and
//! an output a later job reads, created as one run on an Actions shard. Each test fails on one plausible bug: a called
//! job that does not run or runs before its caller's needs, an input read from the wrong job or untyped, `inherit` that
//! passes nothing or an explicit mapping that passes everything, a chain A > B > C that hands C what B did not pass, an
//! output taken from a job rather than the called file's `on.workflow_call.outputs`, a matrix call's output from the
//! wrong leg, a branch picked over a tag of the same name, `job_workflow_ref` naming the caller, the GHES limits on
//! levels, files and loops, a nested job raising its permissions, and a reader older than 0x15.9 taking the run.

mod common;

use std::future::Future;

use common::Group;
use graftyard_actions::reusable::{self, Fetched, Origin, Version, Want};
use graftyard_actions::shard::{
    ACTIONS_FORMAT, ACTIONS_FORMAT_CLAIMS, ACTIONS_FORMAT_HEAD_REF, ACTIONS_FORMAT_REUSABLE, ACTIONS_FORMAT_TOKENS,
    ActionsActor, ActionsCmd, ActionsFold, ActionsOp, ActionsOut, WorkflowRun,
};
use graftyard_actions::{Conclusion, JobEvent, JobId, LabelSet, RunId, RunnerId, workflow};
use graftyard_git::refs::{ObjectId, RefName};
use graftyard_types::{AccountId, ChainId, DbId, IdBlock, Index, RepoId, Timestamp};
use serde_json::{Value, json};

const OWNER: AccountId = AccountId(DbId(100));
const REPO: RepoId = RepoId(DbId(7));

/// The caller, in `octo/app` at `main`.
const CALLER: &str = r#"on: workflow_dispatch
permissions:
  contents: read
  id-token: write
jobs:
  build:
    runs-on: [self-hosted]
    outputs:
      target: ${{ steps.t.outputs.target }}
      n: ${{ steps.t.outputs.n }}
    steps:
      - id: t
        run: echo "target=prod-7" >> "$GITHUB_OUTPUT"
  call:
    needs: build
    uses: octo/shared/.github/workflows/deploy.yml@v1
    with:
      target: ${{ needs.build.outputs.target }}
      dry: true
      count: ${{ needs.build.outputs.n }}
    secrets: inherit
  after:
    needs: call
    runs-on: [self-hosted]
    steps:
      - run: echo "${{ needs.call.outputs.url }}"
"#;

/// `octo/shared`'s `deploy.yml` at tag `v1`.
const DEPLOY: &str = r#"on:
  workflow_call:
    inputs:
      target:
        type: string
        required: true
      dry:
        type: boolean
        default: false
      count:
        type: number
      region:
        type: string
        default: eu
    outputs:
      url:
        value: ${{ jobs.deploy.outputs.link }}
jobs:
  prep:
    runs-on: [self-hosted]
    outputs:
      url: ${{ steps.p.outputs.url }}
    steps:
      - id: p
        run: echo "url=wrong" >> "$GITHUB_OUTPUT"
  deploy:
    needs: prep
    runs-on: [self-hosted]
    outputs:
      link: ${{ steps.d.outputs.link }}
    steps:
      - id: d
        run: echo "url=https://${{ inputs.target }}" >> "$GITHUB_OUTPUT"
"#;

fn oid(n: u8) -> [u8; 20] {
    let mut o = [0u8; 20];
    o[0] = n;
    o[19] = 1;
    o
}

fn hex(o: [u8; 20]) -> String {
    o.iter().map(|b| format!("{b:02x}")).collect()
}

/// An install's repositories for the fetch: each `(repo, ref, sha)` and each `(repo, sha, path) -> src`.
struct Install {
    refs: Vec<(&'static str, &'static str, [u8; 20])>,
    files: Vec<(&'static str, [u8; 20], &'static str, String)>,
}

impl Install {
    fn get(&self, w: Want, run_repo: &str) -> Result<Fetched, String> {
        let repo = w.repo.clone().unwrap_or_else(|| run_repo.into());
        let (r, sha) = match &w.version {
            Version::At { r#ref, sha } => (r#ref.clone(), *sha),
            Version::Named(v) => {
                let refs: Vec<_> = self.refs.iter().filter(|(rr, _, _)| *rr == repo).collect();
                let (r, sha) = reusable::resolve_version(v, |n: &RefName| {
                    refs.iter().find(|(_, x, _)| x.as_bytes() == n.as_bytes()).map(|(_, _, s)| ObjectId(*s))
                })
                .ok_or("no such ref")?;
                (r, sha.0)
            }
        };
        let src = self
            .files
            .iter()
            .find(|(fr, fs, fp, _)| *fr == repo && *fs == sha && *fp == w.path)
            .map(|f| f.3.clone())
            .ok_or_else(|| format!("{repo}/{}@{} not found", w.path, hex(sha)))?;
        Ok(Fetched { origin: Origin { repo: w.repo, path: w.path, r#ref: r, sha }, src })
    }
}

fn block_on<T>(f: impl Future<Output = T>) -> T {
    let mut f = std::pin::pin!(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match f.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(t) => t,
        std::task::Poll::Pending => panic!("the fetch closure here is ready at once"),
    }
}

fn top() -> Origin {
    Origin { repo: None, path: ".github/workflows/ci.yml".into(), r#ref: "refs/heads/main".into(), sha: oid(1) }
}

fn plan(inst: &Install, src: &str) -> Result<reusable::Expanded, String> {
    let p = workflow::parse(".github/workflows/ci.yml", src);
    assert!(p.ok(), "{:?}", p.errors);
    block_on(reusable::plan_run(&p.workflow, &top(), true, |w| {
        let r = inst.get(w, "octo/app");
        async move { r }
    }))
    .map(|e| e.expect("a planned run"))
}

fn install() -> Install {
    Install {
        refs: vec![
            // A branch named like the tag, at another commit: the tag wins (GHES).
            ("octo/shared", "refs/heads/v1", oid(9)),
            ("octo/shared", "refs/tags/v1", oid(2)),
        ],
        files: vec![
            ("octo/shared", oid(2), ".github/workflows/deploy.yml", DEPLOY.into()),
            ("octo/shared", oid(9), ".github/workflows/deploy.yml", DEPLOY.replace("https://", "branch://")),
        ],
    }
}

struct Shard {
    g: Group<ActionsCmd>,
    now: Timestamp,
    next: u128,
}

impl Shard {
    fn new(reads: Vec<graftyard_raft::Format>) -> Shard {
        let now = Timestamp(10_000_000);
        let mut g = Group::new(ChainId::Actions(OWNER, 3), reads, ActionsFold::default(), now);
        g.id_block(now, IdBlock::whole(0, 9).unwrap());
        Shard { g, now, next: 1 }
    }
    fn cmd(&mut self, actor: ActionsActor, op: ActionsOp) -> Option<Result<ActionsOut, String>> {
        self.next += 1;
        self.now = Timestamp(self.now.0 + 10);
        self.g
            .commit(self.now, self.next, ActionsCmd { actor, op })
            .map(|r| r.map(|a| a.out).map_err(|e| format!("{e:?}")))
    }
    fn f(&self) -> &ActionsFold {
        self.g.fold()
    }
    fn create(&mut self, e: &reusable::Expanded) -> RunId {
        self.create_as(e, 4, None)
    }
    /// The run of trigger index `at`, started by `actor` (BF-73's `actor`, as a site or the edge sends it).
    fn create_as(&mut self, e: &reusable::Expanded, at: u64, actor: Option<(AccountId, &str)>) -> RunId {
        let run = WorkflowRun {
            id: RunId(DbId(0)),
            repo: REPO,
            workflow: ".github/workflows/ci.yml".into(),
            event: "workflow_dispatch".into(),
            head_sha: oid(1),
            head_ref: "refs/heads/main".into(),
            trigger: (ChainId::Repo(REPO), Index(at)),
            attempt: 1,
            concurrency: None,
            fork: false,
        };
        let call = graftyard_actions::runsvc::with_plan(
            graftyard_actions::runsvc::create_run_call(&run, &e.jobs, &e.declared, true, None, None, actor),
            e.plan.as_ref(),
        );
        let (_, op) = graftyard_actions::runsvc::create_run_op(&call).unwrap();
        assert!(matches!(op, ActionsOp::CreateRunPlanned { .. }), "{op:?}");
        match self.cmd(ActionsActor::Operator, op) {
            Some(Ok(ActionsOut::Run(r))) => r,
            other => panic!("not created: {other:?}"),
        }
    }
    fn job(&self, run: RunId, name: &str) -> JobId {
        self.f().run_jobs(run).into_iter().find(|j| &*j.spec.name == name).unwrap_or_else(|| panic!("{name}")).spec.id
    }
    /// Runs `job` to `Finished(c)` with `outputs`, as `completejob` does.
    fn finish(&mut self, job: JobId, c: Conclusion, outputs: &[(&str, &str)]) {
        let runner = match self.cmd(ActionsActor::Admin(OWNER), ActionsOp::PutRunner(runner())) {
            Some(Ok(ActionsOut::Runner(r))) => r,
            other => panic!("{other:?}"),
        };
        for (actor, ev) in [
            (ActionsActor::Runner(runner), JobEvent::Matched(runner)),
            (ActionsActor::Runner(runner), JobEvent::Acquired),
        ] {
            let v = self.f().job(job).unwrap().version;
            assert!(matches!(self.cmd(actor, ActionsOp::Job { job, version: v, event: ev }), Some(Ok(_))));
        }
        let version = self.f().job(job).unwrap().version;
        let outputs = outputs.iter().map(|(k, v)| (Box::from(*k), Box::from(*v))).collect();
        let op = ActionsOp::FinishWith { job, version, conclusion: c, outputs };
        assert!(matches!(self.cmd(ActionsActor::Job(job), op), Some(Ok(_))), "finished");
    }
}

fn runner() -> graftyard_actions::shard::RunnerRecord {
    graftyard_actions::shard::RunnerRecord {
        scope: graftyard_actions::shard::RunnerScope::Repo(REPO),
        name: "r".into(),
        labels: LabelSet::parse(["self-hosted", "linux", "x"]).unwrap(),
        key: Box::new([]),
        ephemeral: true,
        hosted: false,
        version: "2.338.2".into(),
    }
}

const READS: [graftyard_raft::Format; 4] =
    [ACTIONS_FORMAT, ACTIONS_FORMAT_TOKENS, ACTIONS_FORMAT_CLAIMS, ACTIONS_FORMAT_REUSABLE];

fn secrets() -> Vec<(String, String)> {
    vec![("DEPLOY_KEY".into(), "k-123".into()), ("OTHER".into(), "o-456".into())]
}

fn ctx(sh: &Shard, job: JobId) -> reusable::JobContexts {
    let github = json!({ "repository": "octo/app", "ref": "refs/heads/main" });
    reusable::contexts(sh.f(), job, &github, &[], &secrets()).expect("a planned job")
}

/// The whole row in memory: the called file is read at the tag (not the same-named branch), its jobs run after the
/// caller's `needs` as `call / prep` and `call / deploy`, take the caller's typed inputs and every secret, and the
/// later job reads the called file's output, which maps `deploy`'s, not `prep`'s.
#[test]
fn a_called_workflow_runs_with_its_inputs_and_secrets_and_its_output_reaches_a_later_job() {
    let e = plan(&install(), CALLER).unwrap();
    let names: Vec<&str> = e.jobs.iter().map(|j| &*j.name).collect();
    assert_eq!(names, ["build", "call / prep", "call / deploy", "after"]);
    // `prep` has no needs of its own, so it waits for its caller's; `deploy` needs `prep`; `after` needs the call.
    let needs: Vec<Vec<i64>> = e.jobs.iter().map(|j| j.needs.iter().map(|n| n.0.0).collect()).collect();
    assert_eq!(needs, [vec![], vec![0], vec![1], vec![1, 2]]);
    let plan = e.plan.as_ref().unwrap();
    assert_eq!(plan.calls.len(), 1);
    assert_eq!((&*plan.calls[0].r#ref, plan.calls[0].sha), ("refs/tags/v1", oid(2)), "the tag, not the branch");
    // The caller's `permissions:` reach the called jobs, which declare none.
    assert_eq!(e.declared.iter().map(|(p, _)| *p).collect::<Vec<_>>(), [0, 1, 2, 3]);

    let mut sh = Shard::new(READS.to_vec());
    let run = sh.create(&e);
    let (build, prep, deploy, after) =
        (sh.job(run, "build"), sh.job(run, "call / prep"), sh.job(run, "call / deploy"), sh.job(run, "after"));
    assert!(matches!(sh.f().job(prep).unwrap().state, graftyard_actions::JobState::Pending { .. }));
    sh.finish(build, Conclusion::Success, &[("n", "3"), ("target", "prod-7")]);
    assert!(matches!(sh.f().job(prep).unwrap().state, graftyard_actions::JobState::Queued { .. }), "released");

    let c = ctx(&sh, prep);
    // `count` is a number input given an expression's string; `dry` a boolean given a literal.
    assert_eq!(c.inputs, json!({ "target": "prod-7", "dry": true, "count": 3, "region": "eu" }));
    assert_eq!(c.secrets, secrets(), "inherit passes every secret");
    assert_eq!(c.needs, json!({}), "a called job's needs are its own file's, and prep has none");
    let o = ctx(&sh, build).outputs.expect("build's outputs token");
    assert_eq!(o["map"][0]["Key"], json!("target"));

    sh.finish(prep, Conclusion::Success, &[("url", "wrong")]);
    let d = ctx(&sh, deploy);
    assert_eq!(d.needs, json!({ "prep": { "result": "success", "outputs": { "url": "wrong" } } }));
    // `on.workflow_call.outputs.url` maps `deploy`'s `link`, so `url` is the call's whatever its jobs name.
    sh.finish(deploy, Conclusion::Success, &[("link", "https://prod-7")]);
    let a = ctx(&sh, after);
    assert_eq!(a.needs, json!({ "call": { "result": "success", "outputs": { "url": "https://prod-7" } } }));
    assert_eq!(a.inputs, json!({}));
    assert_eq!(a.secrets, secrets(), "a caller's job reads its own secrets");

    // F118: the called job's token names the called file at the tag and its commit; the caller's jobs their own.
    let frame = reusable::job_frame(sh.f(), deploy).expect("deploy runs in the call");
    assert_eq!(frame.workflow_ref("octo/app"), "octo/shared/.github/workflows/deploy.yml@refs/tags/v1");
    assert!(reusable::job_frame(sh.f(), build).is_none() && reusable::job_frame(sh.f(), after).is_none());
    // Read as both hosts read a token's facts (`claim_facts`), so the frame, actor and number ride the same path.
    let claims = |sh: &Shard, job: JobId| {
        let cf = sh.f().claim_facts(job, None).expect("the job");
        let f =
            cf.facts("https://graft.example", "octo/app", OWNER, graftyard_identity::Visibility::Private, None, false);
        graftyard_actions::oidc::claims(&f, None, 1, "j")
    };
    let c = claims(&sh, deploy);
    assert_eq!(c["job_workflow_ref"], json!("octo/shared/.github/workflows/deploy.yml@refs/tags/v1"));
    assert_eq!(c["job_workflow_sha"], json!(hex(oid(2))));
    assert_eq!(c["workflow_ref"], json!("octo/app/.github/workflows/ci.yml@refs/heads/main"), "the caller's");
    assert_eq!(c["repository"], json!("octo/app"));
    assert_eq!(claims(&sh, build)["job_workflow_ref"], c["workflow_ref"]);
}

/// BF-73's rows ride a planned run (0x15.8 with 0x15.9): a called job's token and `github.*` carry the caller run's
/// actor and its run number, and a planned run counts among its workflow's runs, so a plain run after it takes the
/// next number.
#[test]
fn a_planned_run_carries_its_actor_and_run_number() {
    let e = plan(&install(), CALLER).unwrap();
    let mut sh = Shard::new(READS.to_vec());
    let first = sh.create_as(&e, 4, Some((AccountId(DbId(41)), "octocat")));
    let second = sh.create_as(&e, 5, Some((AccountId(DbId(42)), "hubot")));
    assert_eq!((sh.f().run_number(first), sh.f().run_number(second)), (Some(1), Some(2)));
    let deploy = sh.job(second, "call / deploy");
    let cf = sh.f().claim_facts(deploy, None).expect("the job");
    let f = cf.facts("https://graft.example", "octo/app", OWNER, graftyard_identity::Visibility::Private, None, false);
    let c = graftyard_actions::oidc::claims(&f, None, 1, "j");
    assert_eq!((&c["actor"], &c["actor_id"], &c["run_number"]), (&json!("hubot"), &json!("42"), &json!("2")));
    assert_eq!(c["job_workflow_ref"], json!("octo/shared/.github/workflows/deploy.yml@refs/tags/v1"));
    // A plain run of the same workflow numbers after both planned ones.
    let run = WorkflowRun {
        id: RunId(DbId(0)),
        repo: REPO,
        workflow: ".github/workflows/ci.yml".into(),
        event: "workflow_dispatch".into(),
        head_sha: oid(1),
        head_ref: "refs/heads/main".into(),
        trigger: (ChainId::Repo(REPO), Index(6)),
        attempt: 1,
        concurrency: None,
        fork: false,
    };
    let call = graftyard_actions::runsvc::create_run_call(
        &run,
        &e.jobs,
        &e.declared,
        true,
        None,
        None,
        Some((AccountId(DbId(41)), "octocat")),
    );
    let (_, op) = graftyard_actions::runsvc::create_run_op(&call).unwrap();
    assert!(matches!(op, ActionsOp::CreateRunAs { .. }), "{op:?}");
    let Some(Ok(ActionsOut::Run(third))) = sh.cmd(ActionsActor::Operator, op) else { panic!("not created") };
    assert_eq!(sh.f().run_number(third), Some(3));
}

/// BF-78 with `reusable`: a pushed workflow that calls a reusable one is one `CreateRunPush` (0x15.12) that also
/// records the plan, so its called jobs run with it and the run keeps its `github.event`.
#[test]
fn a_planned_push_run_keeps_its_plan_and_its_event() {
    let e = plan(&install(), CALLER).unwrap();
    let mut sh = Shard::new([READS.as_slice(), &[graftyard_actions::shard::ACTIONS_FORMAT_PUSH]].concat());
    let run = WorkflowRun {
        id: RunId(DbId(0)),
        repo: REPO,
        workflow: ".github/workflows/ci.yml".into(),
        event: "push".into(),
        head_sha: oid(1),
        head_ref: "refs/heads/main".into(),
        trigger: (ChainId::Repo(REPO), Index(4)),
        attempt: 1,
        concurrency: None,
        fork: false,
    };
    let call = graftyard_actions::runsvc::with_plan(
        graftyard_actions::runsvc::push_run_call(
            &run,
            &e.jobs,
            &e.declared,
            true,
            Some((AccountId(DbId(41)), "octocat")),
            &json!({"ref": "refs/heads/main"}),
        ),
        e.plan.as_ref(),
    );
    let (_, op) = graftyard_actions::runsvc::create_run_op(&call).unwrap();
    assert!(matches!(op, ActionsOp::CreateRunPush { plan: Some(_), .. }), "{op:?}");
    let Some(Ok(ActionsOut::Run(r))) = sh.cmd(ActionsActor::Operator, op) else { panic!("not created") };
    assert_eq!(sh.f().run_plan(r), e.plan.as_ref(), "the plan is recorded beside the push run");
    assert_eq!(sh.f().run_event(r), Some(r#"{"ref":"refs/heads/main"}"#));
    assert_eq!(sh.f().run_actor(r).map(|(_, l)| l), Some("octocat"));
}

/// An explicit `secrets:` passes only what it maps, under the called file's names, and a chain A > B > C passes C
/// only what B passed it, though B inherited everything from A.
#[test]
fn explicit_secrets_pass_only_their_mapping_down_a_chain() {
    let a = "on: push\njobs:\n  b:\n    uses: octo/shared/.github/workflows/b.yml@main\n    secrets: inherit\n  r:\n    uses: octo/shared/.github/workflows/r.yml@main\n    secrets:\n      inner: ${{ secrets.DEPLOY_KEY }}\n  m:\n    uses: octo/shared/.github/workflows/c.yml@main\n    secrets:\n      token: ${{ secrets.OTHER }}\n";
    let b = "on:\n  workflow_call:\njobs:\n  own:\n    runs-on: [x]\n    steps: [{run: a}]\n  c:\n    uses: ./.github/workflows/c.yml\n    secrets:\n      token: ${{ secrets.DEPLOY_KEY }}\n";
    let c = "on:\n  workflow_call:\n    secrets:\n      token:\n        required: true\njobs:\n  leaf:\n    runs-on: [x]\n    steps: [{run: a}]\n";
    // `r.yml` takes the key as `inner` and passes it on as `token`: C reads what B named, which A never named.
    let r = "on:\n  workflow_call:\n    secrets:\n      inner:\njobs:\n  c:\n    uses: ./.github/workflows/c.yml\n    secrets:\n      token: ${{ secrets.inner }}\n";
    let inst = Install {
        refs: vec![("octo/shared", "refs/heads/main", oid(3))],
        files: vec![
            ("octo/shared", oid(3), ".github/workflows/b.yml", b.into()),
            ("octo/shared", oid(3), ".github/workflows/c.yml", c.into()),
            ("octo/shared", oid(3), ".github/workflows/r.yml", r.into()),
        ],
    };
    let e = plan(&inst, a).unwrap();
    let names: Vec<&str> = e.jobs.iter().map(|j| &*j.name).collect();
    assert_eq!(names, ["b / own", "b / c / leaf", "r / c / leaf", "m / leaf"]);
    // `./` inside `b.yml` is `octo/shared` at `b.yml`'s own commit and ref.
    let calls = &e.plan.as_ref().unwrap().calls;
    assert_eq!(
        (calls[1].repo.as_deref(), &*calls[1].r#ref, calls[1].sha),
        (Some("octo/shared"), "refs/heads/main", oid(3))
    );
    let mut sh = Shard::new(READS.to_vec());
    let run = sh.create(&e);
    let s = |sh: &Shard, n: &str| ctx(sh, sh.job(run, n)).secrets;
    assert_eq!(s(&sh, "b / own"), secrets());
    assert_eq!(s(&sh, "b / c / leaf"), vec![("token".to_string(), "k-123".to_string())]);
    assert_eq!(s(&sh, "r / c / leaf"), vec![("token".to_string(), "k-123".to_string())]);
    assert_eq!(s(&sh, "m / leaf"), vec![("token".to_string(), "o-456".to_string())]);
}

/// A matrix call is one frame per leg, each named by its leg and given its own matrix value as input; the call's
/// output is the last leg to complete that set one, not the first or the last in the matrix.
#[test]
fn a_matrix_call_runs_a_frame_per_leg_and_its_output_is_the_last_set() {
    let a = "on: push\njobs:\n  d:\n    strategy:\n      matrix:\n        env: [dev, prod, qa]\n    uses: ./.github/workflows/w.yml\n    with:\n      env: ${{ matrix.env }}\n  z:\n    needs: d\n    runs-on: [x]\n    steps: [{run: a}]\n";
    let w = "on:\n  workflow_call:\n    inputs:\n      env:\n        type: string\n    outputs:\n      v:\n        value: ${{ jobs.j.outputs.v }}\njobs:\n  j:\n    runs-on: [x]\n    outputs:\n      v: ${{ steps.s.outputs.v }}\n    steps: [{id: s, run: a}]\n";
    let inst = Install { refs: vec![], files: vec![("octo/app", oid(1), ".github/workflows/w.yml", w.into())] };
    let e = plan(&inst, a).unwrap();
    let names: Vec<&str> = e.jobs.iter().map(|j| &*j.name).collect();
    assert_eq!(names, ["d (dev) / j", "d (prod) / j", "d (qa) / j", "z"]);
    let mut sh = Shard::new(READS.to_vec());
    let run = sh.create(&e);
    let (dev, prod) = (sh.job(run, "d (dev) / j"), sh.job(run, "d (prod) / j"));
    assert_eq!(ctx(&sh, dev).inputs, json!({ "env": "dev" }));
    assert_eq!(ctx(&sh, prod).inputs, json!({ "env": "prod" }));
    // prod completes first, then dev, then qa with no value: dev's, the last to complete that set one, wins.
    let qa = sh.job(run, "d (qa) / j");
    sh.finish(prod, Conclusion::Success, &[("v", "from-prod")]);
    sh.finish(dev, Conclusion::Success, &[("v", "from-dev")]);
    sh.finish(qa, Conclusion::Success, &[]);
    let z = ctx(&sh, sh.job(run, "z"));
    assert_eq!(z.needs, json!({ "d": { "result": "success", "outputs": { "v": "from-dev" } } }));
}

/// GHES's limits: four levels and no fifth, 20 distinct files and no 21st, no loop, a called file that takes
/// `workflow_call`, declared inputs only and every required one, and a nested job no wider than its caller.
#[test]
fn the_tree_keeps_ghes_limits() {
    let call = |to: &str| format!("on:\n  workflow_call:\njobs:\n  n:\n    uses: ./.github/workflows/{to}.yml\n");
    let leaf = "on:\n  workflow_call:\njobs:\n  leaf:\n    runs-on: [x]\n    steps: [{run: a}]\n".to_string();
    let chain = |depth: usize| {
        let mut files = Vec::new();
        for i in 1..depth {
            let body = if i + 1 == depth { leaf.clone() } else { call(&format!("w{}", i + 1)) };
            files.push((
                "octo/app",
                oid(1),
                Box::leak(format!(".github/workflows/w{i}.yml").into_boxed_str()) as &str,
                body,
            ));
        }
        Install { refs: vec![], files }
    };
    let top = "on: push\njobs:\n  n:\n    uses: ./.github/workflows/w1.yml\n";
    let four = plan(&chain(4), top).unwrap();
    assert_eq!(four.jobs.iter().map(|j| &*j.name).collect::<Vec<_>>(), ["n / n / n / leaf"]);
    let five = plan(&chain(5), top).unwrap_err();
    assert!(five.contains("more than 4 levels"), "{five}");

    let looped = Install {
        refs: vec![],
        files: vec![
            ("octo/app", oid(1), ".github/workflows/w1.yml", call("w2")),
            ("octo/app", oid(1), ".github/workflows/w2.yml", call("w1")),
        ],
    };
    let err = plan(&looped, "on: push\njobs:\n  n:\n    uses: ./.github/workflows/w1.yml\n").unwrap_err();
    assert!(err.contains("loop"), "{err}");

    let wide = |n: usize| {
        let files = (0..n)
            .map(|i| {
                (
                    "octo/app",
                    oid(1),
                    Box::leak(format!(".github/workflows/l{i}.yml").into_boxed_str()) as &str,
                    leaf.clone(),
                )
            })
            .collect();
        let jobs: String = (0..n).map(|i| format!("  c{i}:\n    uses: ./.github/workflows/l{i}.yml\n")).collect();
        (Install { refs: vec![], files }, format!("on: push\njobs:\n{jobs}"))
    };
    let (i20, s20) = wide(20);
    assert_eq!(plan(&i20, &s20).unwrap().jobs.len(), 20);
    let (i21, s21) = wide(21);
    assert!(plan(&i21, &s21).unwrap_err().contains("more than 20"));
    // The same file called twice counts once.
    let (i1, _) = wide(1);
    let twice =
        "on: push\njobs:\n  a:\n    uses: ./.github/workflows/l0.yml\n  b:\n    uses: ./.github/workflows/l0.yml\n";
    assert_eq!(plan(&i1, twice).unwrap().plan.unwrap().calls.len(), 2);

    let not_callable = Install {
        refs: vec![],
        files: vec![(
            "octo/app",
            oid(1),
            ".github/workflows/p.yml",
            "on: push\njobs:\n  j:\n    runs-on: [x]\n    steps: [{run: a}]\n".into(),
        )],
    };
    assert!(
        plan(&not_callable, "on: push\njobs:\n  n:\n    uses: ./.github/workflows/p.yml\n")
            .unwrap_err()
            .contains("workflow_call")
    );
    let typed = Install { refs: vec![], files: vec![("octo/app", oid(1), ".github/workflows/t.yml", "on:\n  workflow_call:\n    inputs:\n      need:\n        type: string\n        required: true\njobs:\n  j:\n    runs-on: [x]\n    steps: [{run: a}]\n".into())] };
    assert!(
        plan(
            &typed,
            "on: push\njobs:\n  n:\n    uses: ./.github/workflows/t.yml\n    with:\n      need: x\n      extra: y\n"
        )
        .unwrap_err()
        .contains("extra is not defined")
    );
    assert!(
        plan(&typed, "on: push\njobs:\n  n:\n    uses: ./.github/workflows/t.yml\n")
            .unwrap_err()
            .contains("need is required")
    );

    let raises = Install { refs: vec![], files: vec![("octo/app", oid(1), ".github/workflows/r.yml", "on:\n  workflow_call:\njobs:\n  j:\n    runs-on: [x]\n    permissions:\n      contents: write\n    steps: [{run: a}]\n".into())] };
    let caller = "on: push\njobs:\n  n:\n    permissions:\n      contents: read\n    uses: ./.github/workflows/r.yml\n";
    assert!(plan(&raises, caller).unwrap_err().contains("permissions its caller does not grant"));
    assert!(plan(&raises, &caller.replace("contents: read", "contents: write")).is_ok());

    for bad in [
        "octo/shared/.github/workflows/x.yml",
        "octo/shared/.github/workflows/sub/x.yml@v1",
        "./.github/workflows/x.yml@v1",
        "octo/shared/.github/workflows/x.yml@refs/heads/main",
    ] {
        assert!(reusable::want(bad, &self::top()).is_err(), "{bad}");
    }
}

/// A member older than 0x15.9 refuses the planned run before it decides, and a fold with no plan row keeps 0x15.7's
/// bytes; a follower refuses a plan that does not fit its run and outputs for a job that did not finish.
#[test]
fn a_planned_run_needs_a_reader_of_0x15_9_and_old_folds_keep_their_bytes() {
    let e = plan(&install(), CALLER).unwrap();
    let mut old = Shard::new(vec![ACTIONS_FORMAT, ACTIONS_FORMAT_TOKENS, ACTIONS_FORMAT_HEAD_REF]);
    let before = postcard::to_allocvec(old.f()).unwrap();
    let call = graftyard_actions::runsvc::with_plan(
        graftyard_actions::runsvc::create_run_call(&dummy_run(), &e.jobs, &e.declared, true, None, None, None),
        e.plan.as_ref(),
    );
    let (_, op) = graftyard_actions::runsvc::create_run_op(&call).unwrap();
    assert_eq!(
        graftyard_raft::Command::format(&ActionsCmd { actor: ActionsActor::Operator, op: op.clone() }),
        ACTIONS_FORMAT_REUSABLE
    );
    assert!(old.cmd(ActionsActor::Operator, op).is_none_or(|r| r.is_err()), "refused unread");
    assert!(old.f().runs.is_empty());
    // A plain run on the same reader writes 0x15.7's fold bytes.
    let plain =
        graftyard_actions::runsvc::create_run_call(&dummy_run(), &e.jobs[..1], &Vec::new(), false, None, None, None);
    let (_, op) = graftyard_actions::runsvc::create_run_op(&plain).unwrap();
    assert!(matches!(old.cmd(ActionsActor::Operator, op), Some(Ok(_))));
    assert_ne!(postcard::to_allocvec(old.f()).unwrap(), before);
    let rows = &old.f().more.more.more;
    assert_eq!(
        postcard::to_allocvec(rows).unwrap(),
        postcard::to_allocvec(&rows.heads).unwrap(),
        "no plan row written"
    );

    let sh = Shard::new(READS.to_vec());
    use graftyard_raft::{Fold, Scratch, TierSet};
    let sc = sh.f().scratch_at(Timestamp(1), TierSet::CORE);
    let ev = graftyard_actions::shard::ActionsEvent::JobOutputs { job: JobId(DbId(1)), outputs: vec![] };
    assert!(sc.check(&ev).is_err());
    let bad = reusable::RunPlan { jobs: vec![], calls: vec![] };
    assert!(reusable::check_plan(&bad, 1).is_err() && reusable::check_plan(&e.plan.clone().unwrap(), 4).is_ok());
    let _ = RunnerId(DbId(0));
}

/// A dispatch's run: a `push` run's create is 0x15.12's (BF-78), whatever else it carries.
fn dummy_run() -> WorkflowRun {
    WorkflowRun {
        id: RunId(DbId(0)),
        repo: REPO,
        workflow: ".github/workflows/ci.yml".into(),
        event: "workflow_dispatch".into(),
        head_sha: oid(1),
        head_ref: "refs/heads/main".into(),
        trigger: (ChainId::Repo(REPO), Index(5)),
        attempt: 1,
        concurrency: None,
        fork: false,
    }
}

/// `completejob`'s outputs as the runner sends them: `VariableValue`s, a secret one left out.
#[test]
fn completejob_outputs_read_the_runners_variable_values() {
    let b = json!({ "conclusion": "succeeded", "outputs": {
        "url": { "value": "https://x" }, "tok": { "value": "s", "isSecret": true }, "n": { "value": "" } } });
    let got: Vec<(String, String)> =
        graftyard_actions::runsvc::outputs(&b).into_iter().map(|(k, v)| (k.into(), v.into())).collect();
    assert_eq!(got, [("n".to_string(), String::new()), ("url".into(), "https://x".into())]);
    assert_eq!(graftyard_actions::runsvc::outputs(&json!({})), Vec::new());
    let _: Value = Value::Null;
}

/// `owner/name`, its id, its visibility, and its refs.
type OtherRepo = (&'static str, i64, graftyard_git::issues::Visibility, Vec<(&'static str, [u8; 20])>);

/// An install's repositories as `CallReads` reads them (F132): each `owner/name` with its id, visibility, refs and
/// files, the rows a narrow `tarball` read answers being its `settings` and `ref` rows; and the run's own files.
struct Rows {
    repos: Vec<OtherRepo>,
    files: Vec<(Option<&'static str>, [u8; 20], &'static str, String)>,
    asked: std::cell::RefCell<Vec<String>>,
}

impl reusable::CallReads for Rows {
    type At = &'static str;
    async fn other(&self, o: &str, n: &str, v: &str) -> Result<Option<reusable::Other<&'static str>>, String> {
        self.asked.borrow_mut().push(format!("{o}/{n}@{v}"));
        let Some((name, id, vis, refs)) = self.repos.iter().find(|r| r.0.eq_ignore_ascii_case(&format!("{o}/{n}")))
        else {
            return Ok(None);
        };
        use graftyard_fold::{Store, Stored, Val};
        let blob = |b: Vec<u8>| vec![Val::Blob(b.into())];
        let settings = graftyard_git::issues::RepoSettings {
            owner: OWNER,
            enterprise: None,
            visibility: *vis,
            default_branch: RefName::parse(b"refs/heads/main").unwrap(),
            archived: false,
            description: None,
            homepage: None,
            topics: Default::default(),
            has_issues: true,
            has_wiki: false,
            has_projects: false,
            merge_methods: Default::default(),
            delete_branch_on_merge: false,
            allow_auto_merge: false,
            allow_update_branch: false,
            check_retention_days: 90,
        };
        let mut core = graftyard_fold::MemStore::default();
        let row = |cols| Stored { written: Index(1), cols };
        core.put("settings", Vec::new(), row(blob(postcard::to_allocvec(&settings).unwrap())));
        for (r, sha) in refs {
            let key = vec![graftyard_git::tiers::ref_key(&RefName::parse(r.as_bytes()).unwrap())];
            core.put("ref", key, row(blob(postcard::to_allocvec(&ObjectId(*sha)).unwrap())));
        }
        Ok(Some(reusable::Other { id: RepoId(DbId(*id)), owner: OWNER, core, at: name }))
    }
    async fn blob(
        &self,
        at: Option<&reusable::Other<&'static str>>,
        sha: [u8; 20],
        p: &str,
    ) -> Result<Option<String>, String> {
        let at = at.map(|o| o.at);
        Ok(self.files.iter().find(|f| f.0 == at && f.1 == sha && f.2 == p).map(|f| f.3.clone()))
    }
}

/// F132: the trigger and the edge's dispatch read a called file through the same `reusable::called_file`. A push
/// whose workflow calls `octo/shared`'s file at `v1` plans it pinned to the tag, not the branch of the same name; a
/// full SHA pins that commit; a `./` call reads the run's own repository at the caller's commit and resolves no
/// repository; a private repository, which an anonymous caller may not read, and an unknown one refuse the tree
/// with "not found or not readable"; a version no ref names refuses it too.
#[test]
fn a_called_file_in_another_repository_is_pinned_as_the_dispatch_pins_it() {
    use graftyard_git::issues::Visibility::{Private, Public};
    let reads = Rows {
        repos: vec![
            (
                "octo/shared",
                21,
                Public,
                vec![("refs/heads/main", oid(9)), ("refs/heads/v1", oid(9)), ("refs/tags/v1", oid(2))],
            ),
            ("octo/secret", 22, Private, vec![("refs/tags/v1", oid(4))]),
        ],
        files: vec![
            (Some("octo/shared"), oid(2), ".github/workflows/deploy.yml", DEPLOY.into()),
            (Some("octo/shared"), oid(9), ".github/workflows/deploy.yml", DEPLOY.replace("https://", "branch://")),
            (Some("octo/shared"), oid(5), ".github/workflows/deploy.yml", DEPLOY.replace("https://", "sha://")),
            (Some("octo/secret"), oid(4), ".github/workflows/deploy.yml", DEPLOY.into()),
            (None, oid(1), ".github/workflows/local.yml", DEPLOY.into()),
        ],
        asked: Default::default(),
    };
    let fetch = |uses: &str| block_on(reusable::called_file(&reads, reusable::want(uses, &top()).unwrap()));
    let got = fetch("octo/shared/.github/workflows/deploy.yml@v1").unwrap();
    assert_eq!(
        (got.origin.repo.as_deref(), &*got.origin.r#ref, got.origin.sha),
        (Some("octo/shared"), "refs/tags/v1", oid(2))
    );
    assert!(got.src.contains("https://"), "the tag's file, not the branch's");
    let sha = hex(oid(5));
    let got = fetch(&format!("octo/shared/.github/workflows/deploy.yml@{sha}")).unwrap();
    assert_eq!((&*got.origin.r#ref, got.origin.sha), (sha.as_str(), oid(5)));
    let got = fetch("./.github/workflows/local.yml").unwrap();
    assert_eq!((got.origin.repo, &*got.origin.r#ref, got.origin.sha), (None, "refs/heads/main", oid(1)));
    assert_eq!(reads.asked.borrow().len(), 2, "a ./ call resolves no repository");
    for (uses, why) in [
        ("octo/secret/.github/workflows/deploy.yml@v1", "octo/secret: not found or not readable"),
        ("octo/none/.github/workflows/deploy.yml@v1", "octo/none: not found or not readable"),
        ("octo/shared/.github/workflows/deploy.yml@v2", "octo/shared@v2: no such version"),
    ] {
        assert_eq!(fetch(uses).unwrap_err(), why, "{uses}");
    }
    // The whole tree, as the trigger plans it: the called jobs run, pinned to the tag.
    let p = workflow::parse(".github/workflows/ci.yml", CALLER);
    let e =
        block_on(reusable::plan_run(&p.workflow, &top(), true, |w| reusable::called_file(&reads, w))).unwrap().unwrap();
    let call = &e.plan.as_ref().unwrap().calls[0];
    assert_eq!((call.repo.as_deref(), &*call.r#ref, call.sha), (Some("octo/shared"), "refs/tags/v1", oid(2)));
    assert!(e.jobs.iter().any(|j| &*j.name == "call / deploy"));
}
