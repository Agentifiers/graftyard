//! One `graft-runners` pass (`design/SHARED.md`, "Actions", Hosted runners; `design/DESIGN.md`, Shape,
//! `graft-runners`). A pass reads every owner's demand, then for each owner:
//!
//! 1. **Stale.** Deletes each `stale` runner's registration, then destroys its Sandbox. The install refuses the delete
//!    while the runner holds a job (`graftyard_actions::hosted::may_delete`), and then the Sandbox stays: a runner
//!    matched after the demand read keeps its job.
//! 2. **Start.** For each `start` entry, registers one runner (`decide` refuses past the owner's cap, so two
//!    overlapping passes never start more than it) and starts its Sandbox, `standard-3`, or `standard-4` for a 4-core
//!    label. It starts none once the runner account's hosted vCPU would pass `RUNNER_VCPU_STOP_PCT`.
//!
//! A pass holds no state between runs: everything it acts on comes from the demand reads, so a webhook pass and a
//! sweep that overlap are two independent passes. A Sandbox destroys itself when its runner exits (the host's job).

use graftyard_actions::RunnerId;
use graftyard_actions::hosted::{HostedDemand, HostedLabel, RUNNER_VCPU_STOP_PCT};

/// The Sandbox instance type a hosted label runs on (DESIGN, Shape, `graft-runners`): `standard-3` (2 vCPU, 8 GiB,
/// 16 GB), or `standard-4` (4 vCPU, 12 GiB, 20 GB) for a 4-core label, which match the hosted label table's rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Instance {
    Standard3,
    Standard4,
}

impl Instance {
    pub fn for_label(row: &HostedLabel) -> Instance {
        if row.vcpu > 2 { Instance::Standard4 } else { Instance::Standard3 }
    }

    pub fn vcpu(self) -> u32 {
        match self {
            Instance::Standard3 => 2,
            Instance::Standard4 => 4,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Instance::Standard3 => "standard-3",
            Instance::Standard4 => "standard-4",
        }
    }
}

/// A hosted registration's answer: the runner and its `--jitconfig`, the owner at its cap (409), or a failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Registered {
    Runner { id: RunnerId, jit: String },
    Full,
    Failed(String),
}

/// What a pass needs from the world: the install's routes (as the `graftyard-hosted-runners` App) and the runner
/// account's Sandboxes. The Workers host is `crate::worker`; the BF-60 test's is a fake over real members.
#[allow(async_fn_in_trait)]
pub trait Host {
    /// The owners the App is installed on that enable hosted runners.
    fn owners(&self) -> Vec<String>;
    /// The runner account's vCPU (its Containers limit).
    fn account_vcpu(&self) -> u32;
    async fn demand(&self, owner: &str) -> Result<HostedDemand, String>;
    async fn register(&self, owner: &str, row: &'static HostedLabel) -> Registered;
    /// `Ok(false)` when the install refused because the runner holds a job.
    async fn delete(&self, owner: &str, runner: RunnerId) -> Result<bool, String>;
    async fn start(&self, owner: &str, runner: RunnerId, instance: Instance, jit: String) -> Result<(), String>;
    async fn destroy(&self, runner: RunnerId, instance: Instance) -> Result<(), String>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub started: u32,
    pub destroyed: u32,
    /// Stale runners the install refused to delete because they took a job since the demand read.
    pub kept_busy: u32,
    /// Registrations refused at the owner's cap.
    pub full: u32,
    /// Starts skipped at `RUNNER_VCPU_STOP_PCT`.
    pub vcpu_stopped: u32,
    pub errors: Vec<String>,
}

/// One pass over every owner. An owner whose demand read fails gets nothing started or destroyed this pass, and is
/// counted at no vCPU, which the next pass corrects (DESIGN, Failure modes, `actions state`: "a demand read that fails
/// starts nothing").
pub async fn pass<H: Host>(h: &H) -> Report {
    let mut report = Report::default();
    let mut demands = Vec::new();
    for owner in h.owners() {
        match h.demand(&owner).await {
            Ok(d) => demands.push((owner, d)),
            Err(e) => report.errors.push(format!("{owner}: demand: {e}")),
        }
    }
    let stop = h.account_vcpu() * u32::from(RUNNER_VCPU_STOP_PCT) / 100;
    let vcpu_of = |label: &str| HostedLabel::named(label).map_or(4, |r| Instance::for_label(r).vcpu());
    let mut used: u32 = demands.iter().flat_map(|(_, d)| d.runners.iter()).map(|(_, l)| vcpu_of(l)).sum();

    for (owner, d) in &demands {
        for r in &d.stale {
            match h.delete(owner, *r).await {
                Ok(true) => {
                    let label = d.runners.iter().find(|(x, _)| x == r).map_or("", |(_, l)| &**l);
                    let instance = HostedLabel::named(label).map_or(Instance::Standard4, Instance::for_label);
                    if let Err(e) = h.destroy(*r, instance).await {
                        report.errors.push(format!("{owner}: destroy {}: {e}", r.0.0));
                    }
                    report.destroyed += 1;
                    used = used.saturating_sub(vcpu_of(label));
                }
                Ok(false) => report.kept_busy += 1,
                Err(e) => report.errors.push(format!("{owner}: delete {}: {e}", r.0.0)),
            }
        }
    }

    for (owner, d) in &demands {
        'labels: for (label, ld) in &d.labels {
            let Some(row) = HostedLabel::named(label) else { continue };
            let instance = Instance::for_label(row);
            for _ in 0..ld.start.iter().map(|(_, n)| u32::from(*n)).sum::<u32>() {
                if used + instance.vcpu() > stop {
                    report.vcpu_stopped += 1;
                    continue;
                }
                match h.register(owner, row).await {
                    Registered::Runner { id, jit } => match h.start(owner, id, instance, jit).await {
                        // A runner whose Sandbox did not start never polls, so the broker lists it stale after
                        // `HOSTED_IDLE` and a later pass deletes it.
                        Err(e) => report.errors.push(format!("{owner}: start {}: {e}", id.0.0)),
                        Ok(()) => {
                            report.started += 1;
                            used += instance.vcpu();
                        }
                    },
                    Registered::Full => {
                        report.full += 1;
                        continue 'labels;
                    }
                    Registered::Failed(e) => {
                        report.errors.push(format!("{owner}: register: {e}"));
                        continue 'labels;
                    }
                }
            }
        }
    }
    if report.vcpu_stopped > 0 {
        report.errors.push(format!(
            "{} starts skipped: the runner account is at {used} of {} vCPU, past RUNNER_VCPU_STOP_PCT",
            report.vcpu_stopped,
            h.account_vcpu()
        ));
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use graftyard_actions::hosted::LabelDemand;
    use graftyard_actions::shard::RunnerScope;
    use std::cell::RefCell;

    fn r(n: i64) -> RunnerId {
        RunnerId(graftyard_types::DbId(n))
    }

    /// A host that answers one demand and records the calls in order.
    struct Stub {
        demand: HostedDemand,
        busy: Vec<RunnerId>,
        calls: RefCell<Vec<String>>,
        next: RefCell<i64>,
        vcpu: u32,
    }

    impl Host for Stub {
        fn owners(&self) -> Vec<String> {
            vec!["o".into()]
        }
        fn account_vcpu(&self) -> u32 {
            self.vcpu
        }
        async fn demand(&self, _: &str) -> Result<HostedDemand, String> {
            Ok(self.demand.clone())
        }
        async fn register(&self, _: &str, row: &'static HostedLabel) -> Registered {
            *self.next.borrow_mut() += 1;
            self.calls.borrow_mut().push(format!("register {}", row.name()));
            Registered::Runner { id: r(*self.next.borrow()), jit: "j".into() }
        }
        async fn delete(&self, _: &str, runner: RunnerId) -> Result<bool, String> {
            self.calls.borrow_mut().push(format!("delete {}", runner.0.0));
            Ok(!self.busy.contains(&runner))
        }
        async fn start(&self, _: &str, runner: RunnerId, i: Instance, _: String) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("start {} {}", runner.0.0, i.name()));
            Ok(())
        }
        async fn destroy(&self, runner: RunnerId, _: Instance) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("destroy {}", runner.0.0));
            Ok(())
        }
    }

    fn want(label: &str, n: u16) -> (Box<str>, LabelDemand) {
        (
            label.into(),
            LabelDemand { queued: u32::from(n), oldest: None, start: smallvec::smallvec![(RunnerScope::Owner, n)] },
        )
    }

    fn stub(demand: HostedDemand, busy: Vec<RunnerId>, vcpu: u32) -> Stub {
        Stub { demand, busy, calls: RefCell::default(), next: RefCell::new(100), vcpu }
    }

    /// A stale runner is deleted before its Sandbox is destroyed, and a refused delete (it took a job) leaves the
    /// Sandbox; a 4-core label starts on `standard-4`, the others on `standard-3`.
    #[test]
    fn a_pass_deletes_before_it_destroys_and_picks_the_instance_by_label() {
        let d = HostedDemand {
            labels: vec![want("ubuntu-latest", 1), want("ubuntu-24.04-4core", 1)],
            stale: vec![r(1), r(2)],
            runners: vec![(r(1), "ubuntu-latest".into()), (r(2), "ubuntu-latest".into())],
        };
        let h = stub(d, vec![r(2)], 1_500);
        let rep = futures_executor::block_on(pass(&h));
        assert_eq!(
            h.calls.borrow().as_slice(),
            [
                "delete 1",
                "destroy 1",
                "delete 2",
                "register ubuntu-latest",
                "start 101 standard-3",
                "register ubuntu-24.04-4core",
                "start 102 standard-4"
            ]
        );
        assert_eq!((rep.destroyed, rep.kept_busy, rep.started), (1, 1, 2));
    }

    /// With 40 vCPU, 70% is 28: 10 registered `standard-3` runners take 20, so 4 more start and the rest wait; a
    /// stale runner's destroy frees its 2 vCPU first.
    #[test]
    fn a_pass_starts_none_past_the_vcpu_stop() {
        let runners: Vec<(RunnerId, Box<str>)> = (1..=10).map(|n| (r(n), "ubuntu-latest".into())).collect();
        let d = HostedDemand { labels: vec![want("ubuntu-latest", 10)], stale: vec![r(1)], runners };
        let h = stub(d, Vec::new(), 40);
        let rep = futures_executor::block_on(pass(&h));
        assert_eq!((rep.started, rep.vcpu_stopped), (5, 5), "{rep:?}");
        let d = HostedDemand { labels: vec![want("ubuntu-24.04-4core", 3)], stale: Vec::new(), runners: Vec::new() };
        let rep = futures_executor::block_on(pass(&stub(d, Vec::new(), 10)));
        assert_eq!((rep.started, rep.vcpu_stopped), (1, 2), "7 vCPU of 10 holds one 4-vCPU Sandbox");
    }

    /// The account in a cold burst: owners, each with the install's per-owner count as `decide` keeps it, and the
    /// vCPU started. Every pass reads the same demand, as passes that overlap read before any of them registers.
    struct Burst {
        owners: Vec<String>,
        demand: HostedDemand,
        registered: RefCell<std::collections::BTreeMap<String, u16>>,
        started_vcpu: RefCell<u32>,
        next: RefCell<i64>,
        vcpu: u32,
    }

    impl Host for Burst {
        fn owners(&self) -> Vec<String> {
            self.owners.clone()
        }
        fn account_vcpu(&self) -> u32 {
            self.vcpu
        }
        async fn demand(&self, _: &str) -> Result<HostedDemand, String> {
            Ok(self.demand.clone())
        }
        async fn register(&self, owner: &str, _: &'static HostedLabel) -> Registered {
            let mut reg = self.registered.borrow_mut();
            let n = reg.entry(owner.into()).or_default();
            if *n >= graftyard_actions::hosted::HOSTED_CAP {
                return Registered::Full;
            }
            *n += 1;
            *self.next.borrow_mut() += 1;
            Registered::Runner { id: r(*self.next.borrow()), jit: "j".into() }
        }
        async fn delete(&self, _: &str, _: RunnerId) -> Result<bool, String> {
            Ok(true)
        }
        async fn start(&self, _: &str, _: RunnerId, i: Instance, _: String) -> Result<(), String> {
            *self.started_vcpu.borrow_mut() += i.vcpu();
            Ok(())
        }
        async fn destroy(&self, _: RunnerId, _: Instance) -> Result<(), String> {
            Ok(())
        }
    }

    /// With no per-application `max_instances` (R23's `RunnerBox`), a cold burst is bounded by the two rules a pass
    /// applies: `decide`'s per-owner cap, which holds across passes, and `RUNNER_VCPU_STOP_PCT`, which holds within a
    /// pass, so passes that overlap can each start up to it but never past the cap; the account's vCPU limit, past
    /// which the platform refuses the start, is the box's (`tests/box.test.mjs`).
    #[test]
    fn a_cold_burst_is_bounded_by_the_cap_across_passes_and_the_vcpu_stop_within_one() {
        let burst = || Burst {
            owners: vec!["a".into(), "b".into(), "c".into()],
            demand: HostedDemand { labels: vec![want("ubuntu-latest", 20)], stale: Vec::new(), runners: Vec::new() },
            registered: RefCell::default(),
            started_vcpu: RefCell::default(),
            next: RefCell::new(0),
            vcpu: 100,
        };
        // One pass: 70 vCPU of 100 is 35 `standard-3`: owner a's 20, then b's 15; b's other 5 and all of c wait.
        let h = burst();
        let rep = futures_executor::block_on(pass(&h));
        assert_eq!((rep.started, rep.vcpu_stopped, *h.started_vcpu.borrow()), (35, 25, 70));
        // Four passes that read before any registered: each starts up to its stop, and the cap stops them at 20 per
        // owner, 120 vCPU here, which the account's limit then bounds.
        let h = burst();
        let reps = futures_executor::block_on(futures_util::future::join_all((0..4).map(|_| pass(&h))));
        assert!(h.registered.borrow().values().all(|n| *n == 20), "{:?}", h.registered.borrow());
        assert_eq!(reps.iter().map(|r| r.started).sum::<u32>(), 60);
        assert!(reps.iter().all(|r| r.started * 2 <= 70), "no pass starts past its stop");
        assert!(reps.iter().map(|r| r.full).sum::<u32>() > 0, "later passes are refused at the cap");
    }
}
