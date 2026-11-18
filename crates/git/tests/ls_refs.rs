//! `graftyard_git::ls_refs` against github.com's own v2 `ls-refs` answers (`tests/ls_refs/`, recorded with `git/2.56.0`
//! request bodies for jqlang/jq, git/git and axios/axios; `RECORDED`). Each repository's refs and HEAD come from its `source` answer
//! (HEAD, every branch, and tags, with `peeled:`); every other recorded case is answered from them and must match
//! github.com's bytes exactly, or, for a case with `peel` over a tag, be sent to the shard.

use graftyard_git::group::RefsPage;
use graftyard_git::ls_refs::{Head, LsRefs};
use graftyard_git::pkt::{Pkt, Reader};
use graftyard_git::refs::ObjectId;
use graftyard_git::smart::V2Request;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/ls_refs/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn lines(answer: &[u8]) -> Vec<String> {
    let mut r = Reader::new(answer);
    let mut out = Vec::new();
    while let Some(p) = r.next_pkt().unwrap() {
        match p {
            Pkt::Data(d) => out.push(String::from_utf8(d.to_vec()).unwrap()),
            Pkt::Flush => out.push("<flush>".into()),
            other => panic!("{other:?} in an ls-refs answer"),
        }
    }
    out
}

/// A repository as the group holds it: its refs as (name, oid hex) in name order, and HEAD.
struct Group {
    refs: Vec<(String, String)>,
    head: Head,
}

/// The refs and HEAD a `source` answer lists. `peeled:` is dropped: the group never holds it.
fn group(repo: &str) -> Group {
    let mut refs = Vec::new();
    let mut head = None;
    for l in lines(&fixture(&format!("{repo}-source.ans"))) {
        let Some(l) = l.strip_suffix('\n') else { continue };
        let mut f = l.split(' ');
        let (oid, name) = (f.next().unwrap(), f.next().unwrap());
        if name == "HEAD" {
            let target = f.find_map(|a| a.strip_prefix("symref-target:")).unwrap();
            head = Some(Head { target: target.as_bytes().to_vec(), oid: ObjectId::parse_hex(oid.as_bytes()) });
        } else {
            refs.push((name.to_owned(), oid.to_owned()));
        }
    }
    refs.sort();
    Group { refs, head: head.unwrap() }
}

fn request(case: &str) -> V2Request {
    let args = String::from_utf8(fixture(&format!("{case}.req"))).unwrap();
    V2Request {
        command: "ls-refs".into(),
        caps: vec!["agent=git/2.56.0".into(), "object-format=sha1".into()],
        args: args.lines().map(|l| l.as_bytes().to_vec()).collect(),
    }
}

/// The group's whole page for each of the request's reads, as the leader answers `refs_query(prefix, None, ..)`.
fn pages(r: &LsRefs, refs: &[(String, String)]) -> Vec<RefsPage> {
    r.reads()
        .iter()
        .map(|p| RefsPage {
            refs: refs.iter().filter(|(n, _)| n.as_bytes().starts_with(p)).cloned().collect(),
            next: None,
        })
        .collect()
}

#[test]
fn every_recorded_github_answer_is_matched_byte_for_byte_or_sent_to_the_shard() {
    let cases = [
        ("jq", "clone-nopeel", true),
        ("jq", "peel-heads", true),
        ("jq", "peel-tags", false),
        ("jq", "partial", true),
        ("jq", "branch", true),
        ("jq", "h", true),
        ("jq", "head-nosym", true),
        ("jq", "source", false),
        ("gitgit", "overlap", true),
        ("gitgit", "source", false),
        // HEAD is the default branch (`v1.x`), not `refs/heads/main`, which also exists: the shard's rule would differ.
        ("axios", "source", true),
    ];
    for (repo, case, edge) in cases {
        let g = group(repo);
        let name = format!("{repo}-{case}");
        let req = request(&name);
        let Some(r) = LsRefs::parse(&req) else {
            assert!(!edge, "{name}: sent to the shard, but the edge can answer it");
            continue;
        };
        assert!(edge, "{name}: answered at the edge, but it asks peel over a tag the group cannot peel");
        let want = fixture(&format!("{name}.ans"));
        let got = r.answer(Some(&g.head), &pages(&r, &g.refs)).expect("whole reads answer");
        assert_eq!(lines(&got), lines(&want), "{name}");
        assert_eq!(got, want, "{name}: framing");
    }
}

#[test]
fn only_whole_reads_and_a_born_head_are_answered_at_the_edge() {
    let g = group("jq");
    let clone = LsRefs::parse(&request("jq-clone-nopeel")).unwrap();
    let whole = pages(&clone, &g.refs);
    assert!(clone.answer(Some(&g.head), &whole).is_some());
    // A read with a page still to come, a read missing, HEAD unknown, or HEAD's branch not created yet (git's `unborn
    // HEAD`, which the shard answers from its own disk): each goes to the shard, never a short answer.
    let mut cut = whole.clone();
    cut[0].next = Some("cmVmcy9oZWFkcy9t".into());
    assert_eq!(clone.answer(Some(&g.head), &cut), None, "a partial page");
    assert_eq!(clone.answer(Some(&g.head), &whole[1..]), None, "a read missing");
    assert_eq!(clone.answer(None, &whole), None, "no HEAD");
    let unborn = Head { target: b"refs/heads/main".to_vec(), oid: None };
    assert_eq!(clone.answer(Some(&unborn), &whole), None, "an unborn HEAD");
    let mut bad = whole.clone();
    bad[0].refs[0].1 = "not-hex".into();
    assert_eq!(clone.answer(Some(&g.head), &bad), None, "an oid that does not parse");
    // A request that lists no HEAD needs none.
    let tip =
        LsRefs::parse(&V2Request { args: vec![b"ref-prefix refs/heads/master".to_vec()], ..request("jq-branch") })
            .unwrap();
    let got = tip.answer(None, &pages(&tip, &g.refs)).unwrap();
    assert_eq!(lines(&got), [format!("{} refs/heads/master\n", g.head.oid.unwrap().hex()), "<flush>".into()]);
}

#[test]
fn host_refs_stay_hidden_and_session_refs_list_only_under_their_prefix() {
    let o = ObjectId([7; 20]).hex();
    let refs: Vec<(String, String)> =
        ["refs/graftyard/lease", "refs/graftyard/sessions/t1", "refs/graftyard/sessions/t2", "refs/heads/x"]
            .iter()
            .map(|n| (n.to_string(), o.clone()))
            .collect();
    let ask = |args: &[&str]| {
        let r = LsRefs::parse(&V2Request {
            command: "ls-refs".into(),
            caps: vec![],
            args: args.iter().map(|a| a.as_bytes().to_vec()).collect(),
        })
        .unwrap();
        lines(&r.answer(None, &pages(&r, &refs)).unwrap()).join("")
    };
    assert_eq!(ask(&["ref-prefix refs/"]), format!("{o} refs/heads/x\n<flush>"));
    assert_eq!(
        ask(&["ref-prefix refs/", "ref-prefix refs/graftyard/sessions/t1"]),
        format!("{o} refs/graftyard/sessions/t1\n{o} refs/heads/x\n<flush>")
    );
}

mod common;

/// F91's read-your-writes: a push acknowledged through the leader and one follower, while the other follower heard
/// none of it, then an `ls-refs` read on that follower by read-index (`LsRefs::query`, `ls_refs::read`). The leader's
/// answer reaches the follower before the entries do; the read is granted only once the follower applied the leader's
/// index, and its answer lists the pushed tip, HEAD and its branch alike, from one fold. A read granted at the
/// follower's own applied index would list the old tip: a stale `ls-refs` after an acknowledged push.
#[test]
fn a_follower_ls_refs_after_an_acknowledged_push_lists_the_new_tip() {
    use common::*;
    use graftyard_git::ls_refs::{LsRefsRead, read};
    let mut w = World::new();
    let now = w.now;
    let g = &mut w.repo;
    let l = g.leader();
    let b = *SITES.iter().find(|s| **s != l).unwrap();
    // The push, acknowledged with b hearing nothing.
    let rid = RequestId(Ulid(9 << 40));
    let push = RepoEvent::Refs {
        push: rid,
        actor: None,
        updates: vec![up("refs/heads/main", Some(oid(1)), Some(oid(2)))],
        pack: None,
    };
    g.on(l, now, |m, t| m.record(t, rid, REPO_FORMAT_V1, vec![push])).unwrap();
    for _ in 0..50 {
        for Envelope { from, to, msg } in std::mem::take(&mut g.inbox) {
            if from != b && to != b {
                g.on(to, now, |m, t| m.step(t, from, msg));
            }
        }
    }
    let acked = g
        .resolutions
        .iter()
        .find_map(|r| match r {
            Resolution::Recorded { id, result: Ok(i) } if *id == rid => Some(*i),
            _ => None,
        })
        .expect("the push is acknowledged without b");
    assert!(g.members[b.0 as usize].group_fold().applied() < acked, "b has not applied the push");
    // b's read: the leader's answer first, b's entries after it.
    let ls = LsRefs::parse(&V2Request {
        command: "ls-refs".into(),
        caps: vec![],
        args: vec![b"symrefs".to_vec(), b"ref-prefix HEAD".to_vec(), b"ref-prefix refs/heads/".to_vec()],
    })
    .unwrap();
    let q = ls.query()["q"]["lsRefs"].clone();
    let id = g.on(b, now, |m, t| m.read_index(t)).expect("b names its leader");
    let granted = |g: &Group<RepoCmd>| {
        g.resolutions.iter().find_map(|r| match r {
            Resolution::Read { id: i, result } if *i == id => Some(result.clone()),
            _ => None,
        })
    };
    // Only the read-index exchange: b's ask and the leader's answer. Anything else to b waits.
    let mut held = Vec::new();
    for _ in 0..20 {
        for e in std::mem::take(&mut g.inbox) {
            let read = matches!(
                e.msg,
                graftyard_raft::Message::ReadIndex { .. } | graftyard_raft::Message::ReadIndexRes { .. }
            );
            if e.to == b && !read {
                held.push(e);
                continue;
            }
            let Envelope { from, to, msg } = e;
            g.on(to, now, |m, t| m.step(t, from, msg));
        }
    }
    assert_eq!(granted(g), None, "b has the leader's index but not its entries: the read waits");
    // Then b's entries, and the leader's resends on ticks.
    g.inbox.extend(held);
    let mut answer = None;
    let mut t = now;
    for _ in 0..20 {
        for Envelope { from, to, msg } in std::mem::take(&mut g.inbox) {
            let seen = g.resolutions.len();
            g.on(to, t, |m, t| m.step(t, from, msg));
            let grant = g.resolutions[seen..].iter().find_map(|r| match r {
                Resolution::Read { id: i, result: Ok(grant) } if *i == id => Some(grant.clone()),
                _ => None,
            });
            if let (Some(grant), None) = (grant, &answer) {
                let f = g.members[b.0 as usize].fresh(&grant).expect("granted on b in this input");
                answer = Some((grant.floor(), f.index(), read(f.fold(), &q)));
            }
        }
        t = Timestamp(t.0 + graftyard_raft::CALL_DEADLINE.as_millis() as u64 + 1);
        for s in SITES {
            g.on(s, t, |m, t| m.tick(t));
        }
    }
    let (floor, at, v) = answer.unwrap_or_else(|| panic!("b's read was not granted: {:?}", granted(g)));
    assert!(floor >= acked && at >= acked, "granted at {at:?} (floor {floor:?}) below the push at {acked:?}");
    let r: LsRefsRead = serde_json::from_value(v).unwrap();
    let got = lines(&ls.answer_read(&r).expect("one fold answers"));
    let tip = oid(2).hex();
    assert_eq!(
        got,
        [format!("{tip} HEAD symref-target:refs/heads/main\n"), format!("{tip} refs/heads/main\n"), "<flush>".into()]
    );
}

/// HEAD and its branch's line from two states (a HEAD tip that differs from the page's line, as an answer stitched
/// from reads at two indexes gives) are never answered: the shard answers.
#[test]
fn head_and_its_branch_from_two_states_go_to_the_shard() {
    use graftyard_git::ls_refs::{HeadRead, LsRefsRead};
    let ls = LsRefs::parse(&V2Request {
        command: "ls-refs".into(),
        caps: vec![],
        args: vec![b"symrefs".to_vec(), b"ref-prefix HEAD".to_vec(), b"ref-prefix refs/heads/".to_vec()],
    })
    .unwrap();
    let (old, new) = (ObjectId([1; 20]).hex(), ObjectId([2; 20]).hex());
    let page = |o: &str| RefsPage { refs: vec![("refs/heads/main".into(), o.into())], next: None };
    let head = |o: &str| Some(HeadRead { target: b"refs/heads/main".to_vec(), oid: Some(o.into()) });
    assert!(ls.answer_read(&LsRefsRead { head: head(&new), pages: vec![page(&new)] }).is_some(), "one state");
    assert_eq!(ls.answer_read(&LsRefsRead { head: head(&old), pages: vec![page(&new)] }), None, "HEAD older");
    assert_eq!(ls.answer_read(&LsRefsRead { head: head(&new), pages: vec![page(&old)] }), None, "the page older");
    assert_eq!(ls.answer_read(&LsRefsRead { head: None, pages: vec![page(&new)] }), None, "no HEAD");
}
