//! One real `Acked`, for the render stages: `Acked::new` is private to the Raft core, so the bed commits one command on
//! a three-member group, as `crates/api/tests/common/mod.rs` `ack` does (copied from it), and `Acked::map`s it to each
//! output it renders.
use graftyard_raft::{
    Acked, Command, CommitError, Ev, Fold, Format, Member, MemberConfig, Record, ReleaseTag, Resolution, Role,
    Scratch, Stored, TierSet,
};
use graftyard_types::{
    ChainId, DbId, PlacementName, PlacementRecord, Region, ReleaseId, RepoId, RequestId, SiteId, Timestamp, Ulid,
};
use smallvec::{SmallVec, smallvec};

/// A command whose output is fixed: what the repository command decided.
#[derive(Clone, Debug, PartialEq)]
struct Decided<T>(T);

#[derive(Clone, Debug, Default, PartialEq)]
struct Nothing;

#[derive(Clone, Debug, PartialEq)]
struct NoEvent;

impl Record for NoEvent {
    fn encode(&self, _: &mut Vec<u8>) {}
}

struct NoScratch;

impl Scratch for NoScratch {
    type Event = NoEvent;
    fn apply(&mut self, _: &NoEvent) {}
    fn check(&self, _: &NoEvent) -> Result<(), Box<str>> {
        Ok(())
    }
}

impl Fold for Nothing {
    type Event = NoEvent;
    type Scratch<'a> = NoScratch;
    fn scratch(&self) -> NoScratch {
        NoScratch
    }
    fn apply(&mut self, _: &NoEvent) {}
}

impl<T> Record for Decided<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(0);
    }
}

const FORMAT: Format = Format { family: 0x7f, version: 1 };

impl<T: Clone + std::fmt::Debug + PartialEq> Command for Decided<T> {
    type Fold = Nothing;
    type Dep = ();
    type Out = T;
    type Err = ();
    fn format(&self) -> Format {
        FORMAT
    }
    fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
    fn deps(&self, _: &Nothing) -> SmallVec<[ChainId; 3]> {
        SmallVec::new()
    }
    fn decide(&self, _: &NoScratch, _: &[(ChainId, ())]) -> Result<(SmallVec<[Ev<Self>; 4]>, T), ()> {
        Ok((smallvec![NoEvent], self.0.clone()))
    }
    fn replay(&self, _: &[NoEvent]) -> T {
        self.0.clone()
    }
}

/// What `ack` does with one member's output: queue its messages, record the ticket, and run its self-ack.
type Take<'a, T> = dyn FnMut(
        usize,
        &mut Vec<Member<Decided<T>>>,
        &mut Vec<Stored<Nothing>>,
        &mut Vec<graftyard_raft::Envelope<NoEvent>>,
        graftyard_raft::Out<Decided<T>>,
    ) + 'a;

/// `out` acknowledged by the Raft core after its entry committed on a three-member `Tri` group whose messages are
/// delivered at once, as `crates/identity/tests/common`'s `Group` drives one.
pub fn ack<T: Clone + std::fmt::Debug + PartialEq>(out_value: T) -> Acked<T> {
    let sites = [SiteId(0), SiteId(1), SiteId(2)];
    let chain = ChainId::Repo(RepoId(DbId(99)));
    let record = PlacementRecord {
        name: PlacementName::Tri,
        sites: sites.iter().copied().collect(),
        home: sites[0],
        generation: 1,
        moving_from: None,
        at: Timestamp(1),
    };
    let cfg = |me| MemberConfig {
        chain,
        me,
        record: record.clone(),
        regions: vec![(sites[0], Region::Enam), (sites[1], Region::Weur), (sites[2], Region::Apac)],
        release: ReleaseTag { id: ReleaseId([7; 32]), seq: 1 },
        reads: vec![FORMAT],
    };
    let mut ms: Vec<Member<Decided<T>>> =
        sites.iter().map(|s| Member::new(cfg(*s), 1 + u64::from(s.0)).unwrap()).collect();
    let mut disks: Vec<Stored<Nothing>> = sites.iter().map(|_| Stored::new(Nothing)).collect();
    let now = Timestamp(10_000);
    let mut inbox = Vec::new();
    let mut acked = None;
    let mut take = |i: usize,
                    ms: &mut Vec<Member<Decided<T>>>,
                    disks: &mut Vec<Stored<Nothing>>,
                    inbox: &mut Vec<graftyard_raft::Envelope<NoEvent>>,
                    mut out: graftyard_raft::Out<Decided<T>>| loop {
        inbox.extend(out.early.drain(..).chain(out.messages.drain(..)));
        for res in out.resolutions.drain(..) {
            if let Resolution::Ticket { result, .. } = res {
                acked = Some(result);
            }
        }
        let Some(a) = out.self_ack.take() else { break };
        out = ms[i].self_ack(now, a).persist(&mut disks[i]).1;
    };
    let deliver = |ms: &mut Vec<Member<Decided<T>>>,
                   disks: &mut Vec<Stored<Nothing>>,
                   inbox: &mut Vec<graftyard_raft::Envelope<NoEvent>>,
                   take: &mut Take<'_, T>| {
        while !inbox.is_empty() {
            for e in std::mem::take(inbox) {
                let i = e.to.0 as usize;
                let (_, out) = ms[i].step(now, e.from, e.msg).persist(&mut disks[i]);
                take(i, ms, disks, inbox, out);
            }
        }
    };
    for _ in 0..2 {
        if ms[0].role() != Role::Leader {
            let (_, out) = ms[0].campaign(now).persist(&mut disks[0]);
            take(0, &mut ms, &mut disks, &mut inbox, out);
            deliver(&mut ms, &mut disks, &mut inbox, &mut take);
        }
    }
    assert_eq!(ms[0].role(), Role::Leader);
    let (admitted, out) = ms[0].commit(now, RequestId(Ulid(1)), Decided(out_value)).persist(&mut disks[0]);
    admitted.expect("admitted");
    take(0, &mut ms, &mut disks, &mut inbox, out);
    deliver(&mut ms, &mut disks, &mut inbox, &mut take);
    match acked.expect("the entry committed") {
        Ok(a) => a,
        Err(CommitError::Refused { .. } | CommitError::NotLeader(_) | CommitError::Full { .. }) => {
            panic!("not acknowledged")
        }
    }
}
