//! A protocol v2 `ls-refs` answered from the repository group's refs, without the git cache shard (`design/DESIGN.md`,
//! "Commit and read paths", Dominant access patterns, *Fetch or clone*). The answer is the bytes stock `git upload-pack`
//! writes for the shard's disk (git 2.56 `ls-refs.c`, `ls_refs`, `send_ref`, `send_possibly_unborn_head`), with the
//! shard's hidden refs (`containers/git/server/main.go`, `hideRefs` and `lsRefsOnly`):
//!
//! - HEAD first, when a `ref-prefix` matches it (or none is given): `<oid> HEAD`, then ` symref-target:<branch>` with
//!   `symrefs`. HEAD is the repository's default branch, as GHES's ("the initial branch that Git checks out locally when
//!   someone clones"); one whose branch does not exist (git's `unborn HEAD`, or no line) is the shard's to answer;
//! - then every ref under `refs/` whose name starts with a `ref-prefix`, each once, in name order;
//! - then a flush.
//!
//! The group knows each ref's object id but not its object's type, so `peel` (which adds ` peeled:<oid>` to a ref
//! naming an annotated tag) is answered here only when every ref the request can list is a branch (`refs/heads/`),
//! which names a commit. Any other `peel` request, and any request stock git would refuse, goes to the shard.
//!
//! `answer` builds the bytes only from whole reads: one complete page per read and HEAD's branch found, else `None`,
//! and the edge sends the request to the shard, so no answer is ever cut from a partial read.
//!
//! The edge asks one member for HEAD and every page at once (`LsRefs::query`, answered by `read`), by read-index on the
//! group's learners and followers (`design/DESIGN.md`, "Follower read"), so every line of an answer comes from one
//! fold at one applied index at or above the read's floor, which holds every push acknowledged before the read arrived.

use serde::{Deserialize, Serialize};

use crate::agent::{HOST_REFS, SESSION_REFS};
use crate::group::RefsPage;
use crate::pack::b64;
use crate::pkt;
use crate::refs::ObjectId;
use crate::repo::{REFS_PAGE_MAX, RepoFold};
use crate::smart::V2Request;

/// git's `TOO_MANY_PREFIXES`: at this many `ref-prefix` lines or more, git ignores them all and lists every ref.
const TOO_MANY_PREFIXES: usize = 65_536;

/// An `ls-refs` request the edge can answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LsRefs {
    peel: bool,
    symrefs: bool,
    /// The `ref-prefix` values, empty for every ref.
    prefixes: Vec<Vec<u8>>,
    /// The `ref-prefix` values under `SESSION_REFS`, which unhide the session refs that start with one of them.
    sessions: Vec<Vec<u8>>,
}

/// HEAD as the group knows it: the default branch (`RepoSettings::default_branch`), and that branch's tip when it exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    pub target: Vec<u8>,
    pub oid: Option<ObjectId>,
}

/// A member's answer to `LsRefs::query`, all from one fold: HEAD's branch (`crate::tiers::default_branch`) and tip when
/// asked, and one page per read, in the query's order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LsRefsRead {
    pub head: Option<HeadRead>,
    pub pages: Vec<RefsPage>,
}

/// `Head` on the wire: the branch's name as base64 of its bytes, and its tip as hex.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadRead {
    #[serde(with = "b64")]
    pub target: Vec<u8>,
    pub oid: Option<String>,
}

/// The member's half of `LsRefs::query` (`{"lsRefs": {"reads": [b64], "head": bool}}`): HEAD and each read's first
/// page of at most `REFS_PAGE_MAX` refs, from `fold` alone. `Null` for a query it cannot parse.
pub fn read(fold: &RepoFold, q: &serde_json::Value) -> serde_json::Value {
    let parse = || -> Option<(Vec<Vec<u8>>, bool)> {
        let reads = q.get("reads")?.as_array()?.iter().map(|r| b64::decode(r.as_str()?)).collect::<Option<_>>()?;
        Some((reads, q.get("head")?.as_bool()?))
    };
    let Some((reads, head)) = parse() else { return serde_json::Value::Null };
    let head = head
        .then(|| crate::tiers::default_branch(&*fold.core))
        .map(|b| HeadRead { oid: fold.get(&b).map(|o| o.hex()), target: b.as_bytes().to_vec() });
    let pages = reads.iter().map(|p| crate::group::refs_page(fold, p, None, REFS_PAGE_MAX)).collect();
    serde_json::to_value(LsRefsRead { head, pages }).unwrap_or(serde_json::Value::Null)
}

impl LsRefs {
    /// The request when the edge answers it the same as the shard would: `command=ls-refs`, SHA-1, only the arguments
    /// git knows, and `peel` only where every listed ref is a branch. `None` sends it to the shard, so an unknown
    /// argument or another object format gets stock git's own answer.
    pub fn parse(req: &V2Request) -> Option<LsRefs> {
        if req.command != "ls-refs" {
            return None;
        }
        if req.caps.iter().any(|c| c.strip_prefix("object-format=").is_some_and(|f| f != "sha1")) {
            return None;
        }
        let mut r = LsRefs { peel: false, symrefs: false, prefixes: Vec::new(), sessions: Vec::new() };
        for a in &req.args {
            match a.as_slice() {
                b"peel" => r.peel = true,
                b"symrefs" => r.symrefs = true,
                // Changes only a HEAD whose branch does not exist, which `answer` leaves to the shard.
                b"unborn" => {}
                _ => r.prefixes.push(a.strip_prefix(b"ref-prefix ")?.to_vec()),
            }
        }
        if r.prefixes.len() >= TOO_MANY_PREFIXES {
            r.prefixes.clear();
        }
        r.sessions = r.prefixes.iter().filter(|p| p.starts_with(SESSION_REFS.as_bytes())).cloned().collect();
        let branches_only = r.reads().iter().all(|p| p.starts_with(b"refs/heads/") || !may_list(p));
        (!r.peel || branches_only).then_some(r)
    }

    /// The prefixes to read refs by, each read once: git's `find_longest_prefixes`, sorted and with any prefix that
    /// another one starts with folded into it, so the reads are disjoint and their pages in order are the answer's
    /// order. Every ref when the request names none; none that no ref under `refs/` can start with, such as `HEAD`.
    pub fn reads(&self) -> Vec<Vec<u8>> {
        let mut ps = if self.prefixes.is_empty() { vec![Vec::new()] } else { self.prefixes.clone() };
        ps.sort();
        ps.dedup();
        let mut out: Vec<Vec<u8>> = Vec::new();
        for p in ps.into_iter().filter(|p| may_list(p)) {
            if !out.last().is_some_and(|l| p.starts_with(l)) {
                out.push(p);
            }
        }
        out
    }

    /// The one read that answers this request (`read`): every read's page and, when the answer lists HEAD, HEAD.
    pub fn query(&self) -> serde_json::Value {
        let reads: Vec<String> = self.reads().iter().map(|p| b64::encode(p)).collect();
        serde_json::json!({ "q": { "lsRefs": { "reads": reads, "head": self.wants_head() } } })
    }

    /// The answer from a member's `LsRefsRead`, as `answer` builds it. `None` as well when HEAD's tip and its branch's
    /// line in a page differ: one fold gives both the same oid, so they come from two states, and the shard answers.
    pub fn answer_read(&self, r: &LsRefsRead) -> Option<Vec<u8>> {
        let head = match &r.head {
            Some(h) => {
                let name = String::from_utf8_lossy(&h.target);
                let line = r.pages.iter().flat_map(|p| &p.refs).find(|(n, _)| **n == *name).map(|(_, o)| o);
                if line.is_some_and(|o| Some(o) != h.oid.as_ref()) {
                    return None;
                }
                Some(Head {
                    target: h.target.clone(),
                    oid: h.oid.as_deref().and_then(|o| ObjectId::parse_hex(o.as_bytes())),
                })
            }
            None => None,
        };
        self.answer(head.as_ref(), &r.pages)
    }

    /// Whether the answer starts with a HEAD line, so the caller needs `Head`.
    pub fn wants_head(&self) -> bool {
        self.matches(b"HEAD")
    }

    fn matches(&self, name: &[u8]) -> bool {
        self.prefixes.is_empty() || self.prefixes.iter().any(|p| name.starts_with(p))
    }

    /// The shard's hidden refs: every ref under `HOST_REFS`, except the session refs that start with a session
    /// `ref-prefix` of this request (`hideRefs`, `lsRefsOnly`).
    fn visible(&self, name: &[u8]) -> bool {
        let host = HOST_REFS.trim_end_matches('/').as_bytes();
        let hidden = name.strip_prefix(host).is_some_and(|rest| rest.is_empty() || rest[0] == b'/');
        !hidden || (name.starts_with(SESSION_REFS.as_bytes()) && self.sessions.iter().any(|s| name.starts_with(s)))
    }

    /// The whole answer: HEAD's line when asked, then each read's page in `reads` order, then the flush. `None`, for
    /// the shard to answer, unless every read came as one whole page (`next` none), each ref parses, and HEAD, when
    /// asked, is known and its branch exists. `pages` are the answers to `reads()`, in its order.
    pub fn answer(&self, head: Option<&Head>, pages: &[RefsPage]) -> Option<Vec<u8>> {
        if pages.len() != self.reads().len() || pages.iter().any(|p| p.next.is_some()) {
            return None;
        }
        let mut out = Vec::new();
        if self.wants_head() {
            let head = head?;
            let mut line = format!("{} HEAD", head.oid?.hex()).into_bytes();
            if self.symrefs {
                line.extend_from_slice(b" symref-target:");
                line.extend_from_slice(&head.target);
            }
            line.push(b'\n');
            pkt::data(&mut out, &line);
        }
        for (name, oid) in pages.iter().flat_map(|p| &p.refs) {
            // A name that is not UTF-8 reads with U+FFFD (`RefName::display`), not its own bytes.
            if name.contains('\u{FFFD}') {
                return None;
            }
            let (name, oid) = (name.as_bytes(), ObjectId::parse_hex(oid.as_bytes())?);
            if !name.starts_with(b"refs/") || !self.matches(name) || !self.visible(name) {
                continue;
            }
            let mut line = format!("{} ", oid.hex()).into_bytes();
            line.extend_from_slice(name);
            line.push(b'\n');
            pkt::data(&mut out, &line);
        }
        pkt::flush(&mut out);
        Some(out)
    }
}

/// Whether a ref under `refs/` can start with `p`.
fn may_list(p: &[u8]) -> bool {
    p.starts_with(b"refs/") || b"refs/".starts_with(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(args: &[&str]) -> V2Request {
        V2Request {
            command: "ls-refs".into(),
            caps: vec!["agent=git/2.56.0".into(), "object-format=sha1".into()],
            args: args.iter().map(|a| a.as_bytes().to_vec()).collect(),
        }
    }

    #[test]
    fn reads_fold_overlapping_prefixes_as_git_does() {
        let r = LsRefs::parse(&req(&[
            "ref-prefix refs/tags/v2.40",
            "ref-prefix refs/heads/",
            "ref-prefix refs/tags/v2.4",
            "ref-prefix refs/heads/ma",
            "ref-prefix HEAD",
        ]))
        .unwrap();
        let got: Vec<String> = r.reads().iter().map(|p| String::from_utf8(p.clone()).unwrap()).collect();
        assert_eq!(got, ["refs/heads/", "refs/tags/v2.4"]);
        assert!(r.wants_head());
        assert!(!LsRefs::parse(&req(&["ref-prefix refs/heads/main"])).unwrap().wants_head());
        assert!(LsRefs::parse(&req(&["ref-prefix H"])).unwrap().reads().is_empty());
        assert_eq!(LsRefs::parse(&req(&[])).unwrap().reads(), vec![Vec::<u8>::new()]);
    }

    #[test]
    fn peel_is_answered_only_over_branches() {
        assert!(LsRefs::parse(&req(&["peel", "ref-prefix refs/heads/", "ref-prefix HEAD"])).is_some());
        for shard in [
            &["peel"][..],
            &["peel", "ref-prefix refs/tags/"],
            &["peel", "ref-prefix refs/"],
            &["peel", "ref-prefix r"],
            &["peel", "ref-prefix refs/heads/", "ref-prefix refs/pull/"],
        ] {
            assert!(LsRefs::parse(&req(shard)).is_none(), "{shard:?}");
        }
        assert!(LsRefs::parse(&req(&["bogus"])).is_none(), "an argument git refuses");
        let mut sha256 = req(&[]);
        sha256.caps.push("object-format=sha256".into());
        assert!(LsRefs::parse(&sha256).is_none());
    }
}
