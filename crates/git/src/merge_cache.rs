//! What the git cache answers for a real merge or a merge-queue group (`GitCacheShard::merge`; the container's `POST
//! /merge` with `method`), and how that answer becomes a `MergePlan` for `Merge` or a `CacheAnswer` for the
//! `MergeQueue` consumer's `step`. Pure: the hosts call the container and hand the answer here.
//!
//! The container merges each head in turn on the previous result, deterministically, and runs the tree-diff check on
//! each. The packs are inline (`PackRef::Inline`) and the member trusts them as it trusts a push's
//! (`DurablePack::trusted`). A group or plan whose objects pack larger than `MERGE_PACK_MAX` is refused here
//! [inference: a merge's own objects are a few KB; a larger one would need `PackStore::place`, which the edge's merge
//! path does not run yet].

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::issues::{IssueNumber, MergeMethod};
use crate::merge::{EngineVersion, MergePlan, MergeRequest, TreeDiffDigest};
use crate::pack::{DurablePack, PackRef};
use crate::queue::MergeQueueEntry;
use crate::queue_consumer::{CacheAnswer, CacheAsk};
use crate::refs::ObjectId;

/// Inline pack bytes one merge command may carry, under `COMMAND_MAX` with room for the rest of the command.
pub const MERGE_PACK_MAX: usize = crate::prsync::MERGE_BYTES_PER_COMMAND;

/// The container's name for a method.
pub fn method_name(m: MergeMethod) -> &'static str {
    match m {
        MergeMethod::Merge => "merge",
        MergeMethod::Squash => "squash",
        MergeMethod::Rebase => "rebase",
    }
}

/// One head merged onto the previous result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainStep {
    pub n: String,
    pub head: String,
    pub commit: String,
    #[serde(with = "crate::pack::b64_opt", default)]
    pub pack: Option<Vec<u8>>,
    pub up_to_date: bool,
    pub verified: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainConflict {
    pub index: usize,
    pub paths: Vec<String>,
}

/// The container's answer: every step that merged, the first conflict, one pack of every step's objects, and git's
/// version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainAnswer {
    pub steps: Vec<ChainStep>,
    pub conflict: Option<ChainConflict>,
    #[serde(with = "crate::pack::b64_opt", default)]
    pub pack: Option<Vec<u8>>,
    pub engine: String,
}

/// Why an answer cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheError {
    /// The direct merge conflicts: GitHub's 405 "Pull Request is not mergeable".
    Conflict(Vec<String>),
    /// The answer does not hold what was asked, or a pack is over `MERGE_PACK_MAX`.
    Bad(String),
}

fn oid(s: &str) -> Result<ObjectId, CacheError> {
    ObjectId::parse_hex(s.as_bytes()).ok_or_else(|| CacheError::Bad(format!("not an object id: {s}")))
}

fn inline(b: &[u8]) -> Result<DurablePack, CacheError> {
    if b.len() > MERGE_PACK_MAX {
        return Err(CacheError::Bad(format!("merge pack of {} bytes is over {MERGE_PACK_MAX}", b.len())));
    }
    Ok(DurablePack::trusted(PackRef::Inline(b.to_vec())))
}

impl ChainAnswer {
    /// Step `i` as a `MergePlan` from `base_old`.
    fn plan(&self, i: usize, base_old: ObjectId, method: MergeMethod) -> Result<MergePlan, CacheError> {
        let s = &self.steps[i];
        let digest = graftyard_types::Digest::parse_hex(&s.verified)
            .ok_or_else(|| CacheError::Bad(format!("verified {}", s.verified)))?;
        Ok(MergePlan {
            base_old,
            head: oid(&s.head)?,
            commit: oid(&s.commit)?,
            pack: inline(s.pack.as_deref().unwrap_or_default())?,
            method,
            up_to_date: s.up_to_date,
            verified: TreeDiffDigest(digest),
            engine: EngineVersion(self.engine.clone().into()),
        })
    }

    /// The direct merge's plan: the one step, of the head asked for.
    pub fn merge_plan(&self, req: &MergeRequest) -> Result<MergePlan, CacheError> {
        if let Some(c) = &self.conflict {
            return Err(CacheError::Conflict(c.paths.clone()));
        }
        let [s] = self.steps.as_slice() else { return Err(CacheError::Bad("one step".into())) };
        if oid(&s.head)? != req.head {
            return Err(CacheError::Bad("the answer is of another head".into()));
        }
        self.plan(0, req.base_old, req.method)
    }

    /// The consumer's `CacheAnswer` for `ask`. A `Build` keeps the entries before the first conflict and names the
    /// conflicting pull request. A `Plan` re-plans built entries; a step whose commit is not its entry's `group_tip`
    /// is planned anyway, and `QueueMerge::decide` refuses it `StaleChain`.
    pub fn answer(&self, ask: &CacheAsk) -> Result<CacheAnswer, CacheError> {
        let heads: Vec<(IssueNumber, ObjectId)> = match ask {
            CacheAsk::Build { prs, .. } => prs.clone(),
            CacheAsk::Plan { entries, .. } => entries.iter().map(|(e, h)| (e.pr, *h)).collect(),
        };
        if self.steps.len() > heads.len() {
            return Err(CacheError::Bad("more steps than heads".into()));
        }
        for (s, (_, h)) in self.steps.iter().zip(&heads) {
            if oid(&s.head)? != *h {
                return Err(CacheError::Bad("a step of another head".into()));
            }
        }
        match ask {
            CacheAsk::Build { on, .. } => {
                let mut tip = *on;
                let mut entries: SmallVec<[MergeQueueEntry; 8]> = SmallVec::new();
                for (s, (n, _)) in self.steps.iter().zip(&heads) {
                    let e = MergeQueueEntry { pr: *n, parent_tip: tip, group_tip: oid(&s.commit)? };
                    tip = e.group_tip;
                    entries.push(e);
                }
                let pack = match (&self.pack, entries.is_empty()) {
                    (Some(p), false) => Some(inline(p)?),
                    _ => None,
                };
                let conflict = self.conflict.as_ref().and_then(|c| heads.get(c.index)).map(|(n, _)| *n);
                if self.conflict.is_none() && entries.len() != heads.len() {
                    return Err(CacheError::Bad("a step is missing".into()));
                }
                Ok(CacheAnswer::Built { ask: ask.clone(), entries, pack, conflict })
            }
            CacheAsk::Plan { entries, method, .. } => {
                if self.conflict.is_some() || self.steps.len() != entries.len() {
                    return Err(CacheError::Bad("a built entry no longer merges".into()));
                }
                let plans = entries
                    .iter()
                    .enumerate()
                    .map(|(i, (e, _))| Ok((e.pr, self.plan(i, e.parent_tip, *method)?)))
                    .collect::<Result<_, CacheError>>()?;
                Ok(CacheAnswer::Planned { ask: ask.clone(), plans })
            }
        }
    }
}

/// What the container is asked for an ask: the base to start on, and each (n, head) in order.
pub fn chain_of(ask: &CacheAsk) -> (ObjectId, Vec<(IssueNumber, ObjectId)>, MergeMethod) {
    match ask {
        CacheAsk::Build { on, prs, method, .. } => (*on, prs.clone(), *method),
        CacheAsk::Plan { entries, method, .. } => (
            entries.first().map_or(ObjectId([0; 20]), |(e, _)| e.parent_tip),
            entries.iter().map(|(e, h)| (e.pr, *h)).collect(),
            *method,
        ),
    }
}

/// The container's query for a chain: `base=..&heads=..&ns=..&method=..`.
pub fn chain_query(base: ObjectId, heads: &[(IssueNumber, ObjectId)], method: MergeMethod) -> String {
    let hs: Vec<String> = heads.iter().map(|(_, h)| h.hex()).collect();
    let ns: Vec<String> = heads.iter().map(|(n, _)| n.0.to_string()).collect();
    format!("base={}&heads={}&ns={}&method={}", base.hex(), hs.join(","), ns.join(","), method_name(method))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::refs::RefName;

    fn h(b: u8) -> String {
        ObjectId([b; 20]).hex()
    }

    fn step(head: u8, commit: u8) -> ChainStep {
        ChainStep {
            n: "1".into(),
            head: h(head),
            commit: h(commit),
            pack: Some(vec![commit]),
            up_to_date: false,
            verified: "ab".repeat(32),
        }
    }

    fn main() -> RefName {
        RefName::parse(b"refs/heads/main").unwrap()
    }

    #[test]
    fn a_build_answer_chains_from_on_and_keeps_the_entries_before_a_conflict() {
        let ask = CacheAsk::Build {
            base: main(),
            on: ObjectId([1; 20]),
            prs: vec![
                (IssueNumber(4), ObjectId([4; 20])),
                (IssueNumber(5), ObjectId([5; 20])),
                (IssueNumber(6), ObjectId([6; 20])),
            ],
            method: MergeMethod::Merge,
        };
        let a = ChainAnswer {
            steps: vec![step(4, 40)],
            conflict: Some(ChainConflict { index: 1, paths: vec!["f".into()] }),
            pack: Some(vec![1]),
            engine: "git version 2.56.0".into(),
        };
        let CacheAnswer::Built { entries, pack, conflict, .. } = a.answer(&ask).unwrap() else { panic!() };
        assert_eq!(
            entries.as_slice(),
            &[MergeQueueEntry { pr: IssueNumber(4), parent_tip: ObjectId([1; 20]), group_tip: ObjectId([40; 20]) }]
        );
        assert!(pack.is_some());
        assert_eq!(conflict, Some(IssueNumber(5)));
        // A step of a head that was not asked for is refused.
        let bad = ChainAnswer { steps: vec![step(9, 40)], ..a };
        assert!(bad.answer(&ask).is_err());
    }

    #[test]
    fn a_plan_answer_plans_each_entry_from_its_parent_tip() {
        let e1 = MergeQueueEntry { pr: IssueNumber(4), parent_tip: ObjectId([1; 20]), group_tip: ObjectId([40; 20]) };
        let e2 = MergeQueueEntry { pr: IssueNumber(5), parent_tip: ObjectId([40; 20]), group_tip: ObjectId([50; 20]) };
        let ask = CacheAsk::Plan {
            base: main(),
            entries: vec![(e1, ObjectId([4; 20])), (e2, ObjectId([5; 20]))],
            method: MergeMethod::Squash,
        };
        assert_eq!(chain_of(&ask).0, ObjectId([1; 20]));
        let a = ChainAnswer { steps: vec![step(4, 40), step(5, 50)], conflict: None, pack: None, engine: "g".into() };
        let CacheAnswer::Planned { plans, .. } = a.answer(&ask).unwrap() else { panic!() };
        assert_eq!(plans[1].1.base_old, ObjectId([40; 20]));
        assert_eq!(plans[1].1.commit, ObjectId([50; 20]));
        assert_eq!(plans[1].1.method, MergeMethod::Squash);
    }

    #[test]
    fn a_direct_conflict_and_an_oversized_pack_are_refused() {
        let req = MergeRequest {
            n: IssueNumber(1),
            base: main(),
            base_old: ObjectId([1; 20]),
            head: ObjectId([4; 20]),
            method: MergeMethod::Merge,
        };
        let c = ChainAnswer {
            steps: vec![],
            conflict: Some(ChainConflict { index: 0, paths: vec!["f".into()] }),
            pack: None,
            engine: "g".into(),
        };
        assert_eq!(c.merge_plan(&req), Err(CacheError::Conflict(vec!["f".into()])));
        let mut big = step(4, 40);
        big.pack = Some(vec![0; MERGE_PACK_MAX + 1]);
        let a = ChainAnswer { steps: vec![big], conflict: None, pack: None, engine: "g".into() };
        assert!(matches!(a.merge_plan(&req), Err(CacheError::Bad(_))));
    }
}
