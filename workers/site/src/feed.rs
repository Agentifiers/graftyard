//! The activity feed on a serving member (`design/AGENT-LAYER.md`, Shape, "Activity feed"; AM1's hosted half). A
//! repository member keeps `graftyard_git::feed::Ring` of the entries it applied this boot, fed by the host after each
//! input that applied entries, and answers `POST /feed` polls from it with no read-index call:
//!
//! - Learners serve; a follower serves only while the poll says no learner answered; a leader or witness never does.
//! - A member holds polls only while following (`Following`): no gap on its last AppendEntries, no `InstallSnapshot`
//!   catch-up, and its applied `at` within FEED_BATCH of the leader's newest it heard. One that stops following
//!   answers every held poll at once with its cursor and no events, and refuses new ones (`Serving::drop_all`).
//! - At most FEED_HELD_MAX held at once (`Full`, which the edge answers 429), and at most FEED_POLLS per token and per
//!   client IP across isolates of this object, counted over its held polls.
//! - A held poll returns once an event past its cursor matches its `paths`, then waits FEED_BATCH for more (no
//!   `paths`: FEED_BATCH_ALL), or when `wait` ends.
//! - Its peak held count and refusals go out on `/status` (`feed`), which `place_learners`' driver reads
//!   (`graftyard_git::feed::learners_for_feed`).
//!
//! The ring starts empty at boot (a restarted member serves cursors from its first applied entry on, and an older
//! cursor gets 410 with the oldest it can serve, as the design's ring rule allows).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use futures_channel::oneshot;
use graftyard_git::feed::{
    self, Admit, FEED_BATCH, FEED_BATCH_ALL, FEED_POLLS, FeedRole, Following, Ring, Serving, Start,
};
use graftyard_identity::AuthzFold;
use graftyard_types::{Index, RepoId, Timestamp};
use serde_json::{Value, json};

use crate::now_ms;

/// What one poll asks, as the edge sends it.
#[derive(serde::Deserialize)]
pub struct Poll {
    pub cursor: Option<String>,
    pub wait_s: u64,
    #[serde(default)]
    pub paths: Vec<String>,
    /// Whether a learner of this group answered the edge for this poll; a follower serves only while none did.
    #[serde(default)]
    pub learner_answers: bool,
    /// The poll's token and client IP, hashed by the edge: FEED_POLLS counts them over this member's held polls.
    pub token: String,
    pub ip: String,
    /// The edge's second try after every member it asked refused as lagging and this learner lagged least
    /// (`graftyard_git::feed::route`): held under FEED_HELD_MAX although it lags.
    #[serde(default)]
    pub lagging_ok: bool,
}

/// One member's feed: its ring, its held polls, and the owner's `Authz` it last read (lease checks read it).
pub struct Feed {
    pub ring: Ring,
    pub serving: Serving,
    pub owner: Option<AuthzFold>,
    pub owner_at: u64,
    /// The `at` of the newest entry this member heard is committed, and of its last applied entry, for
    /// `Following::lag`. Not the newest entry in its log: an idle group's follower holds the leader's last entries
    /// without hearing they committed until the next write, and read as lagging (every poll 429) until then.
    pub leader_at: Timestamp,
    pub applied_at: Timestamp,
    /// An InstallSnapshot catch-up is running, or the last AppendEntries left a gap.
    pub installing: bool,
    pub gap: bool,
    /// Every `/feed` that reached this member while it led, and in all: LF-12 and L0-15 count them.
    pub leader_polls: u64,
    pub polls: u64,
    /// Held polls per token and per IP.
    by_token: BTreeMap<String, u8>,
    by_ip: BTreeMap<String, u16>,
    /// Woken at each fed entry, or when the member stops following (`true`).
    waiters: Vec<oneshot::Sender<bool>>,
    /// Counts every input that found the member not following (`fed`). A held poll compares it to its count at
    /// admission before each wait, so a stop that came between two of its waits, which drained no waiter of it,
    /// still answers it (`Feed::wait`).
    stops: u64,
    /// The last stop that answered held polls: when (this member's clock), how many, and why (`gap`, `install`, or
    /// `lag`), and when the member next followed again, on `/status` for LF-13.
    last_stop: Option<(u64, u32, &'static str)>,
    last_resume: Option<u64>,
    /// The owner's `Authz` read in flight, and the polls waiting for it: a burst of polls after the owner went stale
    /// sends one read, not one per poll (LF-12 hosted: 3,000 polls sent 3,000 reads to the owner's leader, which
    /// then answered the group leader's dep read 6.3 s late and held a commit for it).
    refreshing: Option<Vec<oneshot::Sender<()>>>,
}

/// What a poll does about the owner's `Authz`: nothing while it is fresh, the read when it is stale and none is in
/// flight, or wait for the one in flight.
#[derive(Debug, PartialEq, Eq)]
enum Refresh {
    Fresh,
    Read,
    Wait,
}

impl Feed {
    fn refresh(&mut self, now: u64, known: bool) -> (Refresh, Option<oneshot::Receiver<()>>) {
        if !known || self.owner_at + 60_000 >= now {
            return (Refresh::Fresh, None);
        }
        match self.refreshing.as_mut() {
            Some(w) => {
                let (tx, rx) = oneshot::channel();
                w.push(tx);
                (Refresh::Wait, Some(rx))
            }
            None => {
                self.refreshing = Some(Vec::new());
                (Refresh::Read, None)
            }
        }
    }

    /// The read in flight ended, read or not: the polls waiting for it go on with whatever owner is known now.
    fn refreshed(&mut self) {
        for w in self.refreshing.take().unwrap_or_default() {
            let _ = w.send(());
        }
    }
}

/// Held by the poll that reads the owner; dropping it ends the read in flight (`Feed::refreshed`).
struct Refreshed(Rc<RefCell<Feed>>);

impl Drop for Refreshed {
    fn drop(&mut self) {
        if let Ok(mut f) = self.0.try_borrow_mut() {
            f.refreshed();
        }
    }
}

impl Default for Feed {
    fn default() -> Feed {
        Feed {
            ring: Ring::default(),
            serving: Serving::default(),
            owner: None,
            owner_at: 0,
            leader_at: Timestamp(0),
            applied_at: Timestamp(0),
            installing: false,
            gap: false,
            leader_polls: 0,
            polls: 0,
            by_token: BTreeMap::new(),
            by_ip: BTreeMap::new(),
            waiters: Vec::new(),
            stops: 0,
            last_stop: None,
            last_resume: None,
            refreshing: None,
        }
    }
}

thread_local! {
    /// Each member's feed, by its object name, with the boot it belongs to. Keyed by member, not by repository: a
    /// learner and its group's voter can share one isolate, and keyed by repository they shared one `Feed`, so the
    /// voter's inputs woke the learner's held polls from the voter's object (B0-29 m3fp3: 220,515 "closure invoked
    /// recursively or after being dropped" and 3,897 "Cannot perform I/O on behalf of a different Durable Object";
    /// `decisions/DECISIONS.md`, "`feed-per-member`"). A new boot starts a new feed: an object reset in the same
    /// isolate leaves its old polls' futures unfinished, and the new boot must not wake them, as `crate::shard`'s
    /// `RUNS` is keyed by instance.
    static FEEDS: RefCell<BTreeMap<Box<str>, Kept>> = const { RefCell::new(BTreeMap::new()) };
}

/// A member's feed in `FEEDS`, with the boot it belongs to.
type Kept = (u64, Rc<RefCell<Feed>>);

/// The feed of the member named `member` at boot `boot`: a new one when none is kept or the kept one is another
/// boot's.
pub fn of(member: &str, boot: u64) -> Rc<RefCell<Feed>> {
    FEEDS.with(|f| {
        let mut f = f.borrow_mut();
        match f.get(member) {
            Some((b, feed)) if *b == boot => feed.clone(),
            _ => {
                let feed = Rc::new(RefCell::new(Feed::default()));
                f.insert(member.into(), (boot, feed.clone()));
                feed
            }
        }
    })
}

/// The member's feed status for `/status`, if it has a feed at this boot.
pub fn status_of(member: &str, boot: u64) -> Option<Value> {
    FEEDS.with(|f| f.borrow().get(member).filter(|(b, _)| *b == boot).map(|(_, feed)| feed.borrow().status()))
}

impl Feed {
    fn following(&self) -> Following {
        // The member's own last applied entry, not the ring's: the ring is empty after a boot and waits for the
        // owner's `Authz`, and a caught-up member must not read as lagging then.
        let lag = Duration::from_millis(self.leader_at.0.saturating_sub(self.applied_at.0));
        Following { gap: self.gap, installing: self.installing, lag }
    }

    /// After entries applied, or a gap or an install began (at `now`, this member's clock): wake every held poll to
    /// look at the ring again. A member that is no longer following answers them all now with their cursor and no
    /// events.
    pub fn fed(&mut self, now: u64) {
        let stop = !self.following().following();
        for w in self.waiters.drain(..) {
            let _ = w.send(stop);
        }
        if stop {
            self.stops += 1;
            let n = self.serving.drop_all();
            if n > 0 {
                let why = if self.gap {
                    "gap"
                } else if self.installing {
                    "install"
                } else {
                    "lag"
                };
                self.last_stop = Some((now, n, why));
                self.last_resume = None;
            }
            self.by_token.clear();
            self.by_ip.clear();
        } else if self.last_stop.is_some() && self.last_resume.is_none() {
            self.last_resume = Some(now);
        }
    }

    /// A held poll admitted at stop count `since` waits for the next fed entry: `None` when the member stopped
    /// following since then, so the poll answers now with its cursor and no events.
    fn wait(&mut self, since: u64) -> Option<oneshot::Receiver<bool>> {
        if self.stops != since {
            return None;
        }
        let (tx, rx) = oneshot::channel();
        self.waiters.push(tx);
        Some(rx)
    }

    pub fn status(&self) -> Value {
        let (floor, newest) = self.ring.bounds();
        json!({ "held": self.serving.held, "peak": self.serving.peak, "refused": self.serving.refused,
                "floor": floor.0, "through": newest.0, "events": self.ring.events.len(),
                "following": self.following().following(), "lagMs": self.following().lag.as_millis() as u64,
                "polls": self.polls, "leaderPolls": self.leader_polls, "gap": self.gap, "installing": self.installing,
                "stops": self.stops,
                "lastStop": self.last_stop.map(|(at, n, why)| json!({ "at": at, "held": n, "why": why })),
                "lastResume": self.last_resume })
    }
}

/// `POST /feed` on a repository member: `{"admit": .., "cursor", "events": [[index, at, event]]}`, where `admit` is
/// `hold` (answered with the events, possibly none when `wait` ended), `lagging`, `full`, `notServing`, `gone` (410
/// with `oldest`), or `tooMany` (FEED_POLLS). Events are rendered by the edge from the JSON form here.
pub async fn poll(feed: Rc<RefCell<Feed>>, repo: RepoId, role: FeedRole, committed: Index, p: Poll) -> Value {
    let q = match feed::parse_query(&format!(
        "wait={}{}",
        p.wait_s,
        p.paths.iter().map(|x| format!("&paths={}", x.replace('%', "%25").replace('&', "%26"))).collect::<String>()
    )) {
        Ok(q) => q,
        Err(e) => return json!({ "admit": "bad", "field": format!("{e:?}") }),
    };
    let filter: Vec<String> = q.paths.clone();
    let start = {
        let f = feed.borrow();
        let (oldest, newest) = f.ring.bounds();
        feed::start(repo, p.cursor.as_deref(), oldest, newest, committed.max(newest))
    };
    let (after, ahead) = match start {
        Start::Gone { oldest } => return json!({ "admit": "gone", "oldest": oldest.encode() }),
        Start::After(i) => (i, false),
        Start::Ahead(i) => (i, true),
    };
    let since = {
        let mut f = feed.borrow_mut();
        if f.by_token.get(&p.token).copied().unwrap_or(0) >= FEED_POLLS.0
            || f.by_ip.get(&p.ip).copied().unwrap_or(0) >= FEED_POLLS.1
        {
            f.serving.refused += 1;
            return json!({ "admit": "tooMany" });
        }
        let mut following = f.following();
        if p.lagging_ok && role == FeedRole::Learner {
            following = Following { gap: false, installing: false, lag: Duration::ZERO };
        } else if ahead {
            // The edge's last resort (`lagging_ok`) above still holds such a poll here.
            following = following.behind_cursor();
        }
        match f.serving.admit(role, p.learner_answers, following) {
            Admit::Hold => {}
            Admit::Lagging => {
                return json!({ "admit": "lagging", "lagMs": following.lag.as_millis() as u64,
                               "cursor": cursor(repo, after), "at": now_ms() });
            }
            Admit::Full => return json!({ "admit": "full" }),
            Admit::NotServing => return json!({ "admit": "notServing" }),
        }
        *f.by_token.entry(p.token.clone()).or_default() += 1;
        *f.by_ip.entry(p.ip.clone()).or_default() += 1;
        f.stops
    };
    let refs: Vec<&str> = filter.iter().map(String::as_str).collect();
    let end = now_ms() + q.wait.as_millis() as u64;
    let batch = if filter.is_empty() { FEED_BATCH_ALL } else { FEED_BATCH };
    let mut first_hit: Option<u64> = None;
    let mut dropped = false;
    loop {
        let (hit, through) = {
            let f = feed.borrow();
            (!f.ring.past(after, &refs).is_empty(), f.ring.through)
        };
        let now = now_ms();
        if hit && first_hit.is_none() {
            first_hit = Some(now);
        }
        let deadline = match first_hit {
            Some(t) => (t + batch.as_millis() as u64).min(end.max(t)),
            None => end,
        };
        if now >= deadline || dropped {
            let _ = through;
            break;
        }
        let Some(rx) = feed.borrow_mut().wait(since) else {
            dropped = true;
            break;
        };
        if let Some(Ok(true)) = crate::host::within(Duration::from_millis(deadline - now), rx).await {
            dropped = true;
        }
    }
    let mut f = feed.borrow_mut();
    if !dropped {
        f.serving.done();
    }
    if let Some(n) = f.by_token.get_mut(&p.token) {
        *n = n.saturating_sub(1);
    }
    if let Some(n) = f.by_ip.get_mut(&p.ip) {
        *n = n.saturating_sub(1);
    }
    if dropped {
        // Stopped following: the cursor as the poll gave it, and no events. `stoppedAt` and `answeredAt` (on every
        // answer) are this member's clock, so LF-13 times answers against its stop and its lift on one clock (A26).
        return json!({ "admit": "hold", "cursor": cursor(repo, after), "events": [], "stopped": true,
                       "stoppedAt": f.last_stop.map(|s| s.0), "answeredAt": now_ms() });
    }
    let events: Vec<Value> =
        f.ring.past(after, &refs).into_iter().map(|(i, at, e)| json!([i.0, at.0, event_json(&e)])).collect();
    json!({ "admit": "hold", "cursor": cursor(repo, f.ring.through.max(after)), "events": events, "answeredAt": now_ms() })
}

fn cursor(repo: RepoId, i: Index) -> String {
    feed::FeedCursor { group: repo, index: i }.encode()
}

/// An event in a JSON form the edge turns back into `ActivityEvent` (`graftyard_git::feed::ActivityEvent::from_json`).
pub fn event_json(e: &feed::ActivityEvent) -> Value {
    e.to_json()
}

/// The member route: this member's role for the feed, the owner's `Authz` once (read through the owner's leader as a
/// `dep`, then refreshed each minute so a lease whose holder lost access expires), then `poll`.
pub async fn route(host: &Rc<crate::host::Host<crate::family::Repo>>, body: Value) -> worker::Result<Value> {
    let p: Poll = serde_json::from_value(body).map_err(|e| crate::host::err(e.to_string()))?;
    let (repo, role, owner, record, committed, feed) = {
        let core = host.core.borrow();
        let c = core.as_ref().ok_or_else(|| crate::host::err("member not joined"))?;
        let Some(repo) = c.m.group_fold().fold.repo else { return Ok(json!({ "admit": "notServing" })) };
        let me = c.join.me.kind;
        let role = match (c.m.role(), me) {
            (_, graftyard_types::MemberKind::Learner) => FeedRole::Learner,
            (graftyard_raft::Role::Leader, _) => FeedRole::Leader,
            (_, graftyard_types::MemberKind::Witness) => FeedRole::Witness,
            _ => FeedRole::Follower,
        };
        // What this member has applied and heard of, for `Following` before any input since boot fed it.
        let feed = of(&c.join.me.do_name, c.m.boot());
        let mut f = feed.borrow_mut();
        f.polls += 1;
        if role == FeedRole::Leader {
            f.leader_polls += 1;
        }
        let applied = c.m.group_fold().applied();
        if let Some(e) = c.m.entry_at(c.m.state().commit) {
            f.leader_at = f.leader_at.max(e.at);
        }
        f.applied_at = f.applied_at.max(c.m.entry_at(applied).map_or(f.leader_at, |e| e.at));
        drop(f);
        // The highest index this member knows the group committed (`Host::committed`): a cursor past it is gone.
        let committed = host.committed(c.m.state().commit);
        (repo, role, c.join.owner, c.join.record.clone(), committed, feed)
    };
    if matches!(role, FeedRole::Leader | FeedRole::Witness) {
        // Never a leader's or a witness's: answered before any read.
        return Ok(json!({ "admit": "notServing" }));
    }
    let (refresh, wait) = feed.borrow_mut().refresh(now_ms(), owner.is_some());
    if let Some(rx) = wait {
        let _ = rx.await;
    }
    if let (Refresh::Read, Some(owner)) = (refresh, owner) {
        // Wakes the polls waiting for this read however it ends, also when this poll's future is dropped mid-read.
        let _done = Refreshed(feed.clone());
        let chain = graftyard_types::ChainId::Authz(owner);
        if let Ok((v, _)) =
            crate::lookup::call(&host.env, chain, &record, "/read", &json!({ "q": { "dep": true } })).await
            && let Some(b) = v.get("value").and_then(Value::as_str).and_then(crate::host::unhex)
            && let Ok(f) = graftyard_raft::codec::from_bytes::<AuthzFold>(&b)
        {
            let mut fd = feed.borrow_mut();
            fd.owner = Some(f);
            fd.owner_at = now_ms();
        }
        // The first owner known: the ring starts at this member's applied entry (`Ring::start`). The owner is cloned
        // out first: a `borrow()` in the `if let` would live through the block and panic the `borrow_mut()` below (it
        // reset the isolate on the first hosted poll).
        let core = host.core.borrow();
        let owner = feed.borrow().owner.clone();
        if let (Some(c), Some(o)) = (core.as_ref(), owner) {
            let applied = c.m.group_fold().applied();
            let mut fd = feed.borrow_mut();
            let at = c.m.entry_at(applied).map_or(fd.applied_at, |e| e.at);
            fd.ring.start(repo, &o, &c.m.group_fold().fold.core, (applied, at));
        }
    }
    if feed.borrow().owner.is_none() {
        return Ok(json!({ "admit": "lagging", "lagMs": u64::MAX }));
    }
    Ok(poll(feed, repo, role, committed, p).await)
}

#[cfg(test)]
mod tests {
    use super::{Feed, Refresh, Refreshed};
    use graftyard_git::feed::{Admit, FeedRole, Following};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    /// LF-13: a held poll is woken by every fed entry and waits again, so between two of its waits it holds no waiter.
    /// A follower that begins an `InstallSnapshot` catch-up in that moment drains no waiter of it, and the poll must
    /// still answer at once instead of waiting out `wait`. The stop is recorded with what it answered and why.
    #[test]
    fn a_stop_between_two_waits_of_a_held_poll_still_answers_it() {
        let mut f = Feed::default();
        let following = Following { gap: false, installing: false, lag: Duration::ZERO };
        assert_eq!(f.serving.admit(FeedRole::Follower, false, following), Admit::Hold);
        let since = f.stops;
        let mut rx = f.wait(since).expect("following: the poll waits");
        f.fed(1_000);
        assert_eq!(rx.try_recv(), Ok(Some(false)), "an entry fed: woken to look at the ring");
        // Before the poll waits again, the leader's `InstallSnapshot` arrives.
        f.installing = true;
        f.fed(2_000);
        assert!(f.wait(since).is_none(), "the poll answers now, with its cursor and no events");
        assert_eq!(f.last_stop, Some((2_000, 1, "install")));
        assert_eq!(f.serving.held, 0);
        // Caught up: a poll admitted from now on waits again.
        f.installing = false;
        f.fed(3_000);
        let since = f.stops;
        assert!(f.wait(since).is_some());
    }

    /// A whole-install restore keeps the group's `RepoId` and can end its log below a cursor a client holds. Such a
    /// cursor, past every index this member knows the group committed, answers `gone` at once, even on the edge's
    /// last resort (`lagging_ok`), which holds a lagging poll on a learner: held there, it would skip the events
    /// between the restored log's end and the cursor.
    #[test]
    fn a_cursor_past_the_groups_commit_index_is_gone_even_on_the_last_resort() {
        use std::future::Future;
        let feed = Rc::new(RefCell::new(Feed::default()));
        let repo = graftyard_types::RepoId(graftyard_types::DbId(7));
        let cursor = graftyard_git::feed::FeedCursor { group: repo, index: graftyard_types::Index(55) }.encode();
        let p = super::Poll {
            cursor: Some(cursor),
            wait_s: 25,
            paths: Vec::new(),
            learner_answers: false,
            token: "t".into(),
            ip: "i".into(),
            lagging_ok: true,
        };
        let fut = super::poll(feed.clone(), repo, FeedRole::Learner, graftyard_types::Index(40), p);
        let mut fut = std::pin::pin!(fut);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let std::task::Poll::Ready(v) = fut.as_mut().poll(&mut cx) else {
            panic!("held: the poll waits past a restore")
        };
        assert_eq!(v["admit"], "gone", "{v}");
        assert_eq!(feed.borrow().serving.held, 0, "nothing held");
    }

    /// `feed-per-member`: a learner and its group's voter in one isolate keep separate feeds. On B0-29 m3fp3 learner
    /// 195 and site 0's voter of repo:65597 shared an isolate and, keyed by repository, one `Feed`: the voter's every
    /// input woke the learner's held polls from the voter's object, and its gap became the learner's. Here the voter
    /// steps with a refused AppendEntries reply (a gap): the learner's held poll stays waiting and the learner still
    /// follows. A new boot of the learner gets a new feed, so a reset object never wakes its old boot's polls.
    #[test]
    fn a_voters_input_never_wakes_a_learners_poll_in_the_same_isolate() {
        use crate::family::{Family, Repo};
        use graftyard_raft::{AppendRes, Envelope, MemberConfig, Message, Out, Term};
        use graftyard_types::{
            ChainId, DbId, Member as MemberRef, MemberKind, PlacementName, PlacementRecord, Region, SiteId, Timestamp,
        };
        let repo = graftyard_types::RepoId(DbId(65_597));
        let chain = ChainId::Repo(repo);
        let record = PlacementRecord {
            name: PlacementName::Tri,
            sites: [0, 1, 2].into_iter().map(SiteId).collect(),
            home: SiteId(0),
            generation: 1,
            moving_from: None,
            at: Timestamp(1),
        };
        let member = |me: &MemberRef, seed: u64| {
            let cfg = MemberConfig {
                chain,
                me: me.addr(),
                record: record.clone(),
                regions: vec![(SiteId(0), Region::Enam), (SiteId(1), Region::Weur), (SiteId(2), Region::Apac)],
                release: crate::host::RELEASE,
                reads: Repo::reads(),
            };
            graftyard_raft::Member::<<Repo as Family>::C>::new(cfg, seed).expect("a member")
        };
        let join = |me: MemberRef| crate::member::Join {
            chain,
            me,
            record: record.clone(),
            peers: Vec::new(),
            owner: None,
            genesis: None,
            #[cfg(feature = "test-faults")]
            no_claim: false,
        };
        let (voter, learner) = (
            join(MemberRef::new(chain, SiteId(0), MemberKind::Full, 0, 1)),
            join(MemberRef::new(chain, SiteId(0), MemberKind::Learner, 3, 1)),
        );
        let (vm, lm) = (member(&voter.me, 1), member(&learner.me, 2));
        assert_ne!(vm.boot(), lm.boot());

        let lf = super::of(&learner.me.do_name, lm.boot());
        let since = lf.borrow().stops;
        let mut held = lf.borrow_mut().wait(since).expect("the learner follows: its poll waits");

        let gap = AppendRes {
            term: Term(2),
            boot: vm.boot(),
            round: 1,
            sent_at: Timestamp(0),
            prev_index: graftyard_types::Index(5),
            ok: false,
            matched: graftyard_types::Index(0),
            last: graftyard_types::Index(3),
            promise: false,
            refused: None,
            stepped_down_at: None,
            boots: [Timestamp(0); 2],
            release: crate::host::RELEASE,
        };
        let out = Out::<<Repo as Family>::C> {
            messages: vec![Envelope { from: SiteId(0), to: SiteId(1), msg: Message::AppendRes(gap) }],
            ..Default::default()
        };
        let at = vm.state().applied;
        Repo::stepped(&vm, &voter, at, at, &out);

        assert!(super::of(&voter.me.do_name, vm.boot()).borrow().gap, "the voter's own feed took its gap");
        assert_eq!(held.try_recv(), Ok(None), "the voter's input woke no poll of the learner");
        assert!(!lf.borrow().gap, "the learner still follows");
        assert!(lf.borrow().following().following());

        let next = super::of(&learner.me.do_name, lm.boot() + 1);
        assert!(!Rc::ptr_eq(&next, &lf), "a new boot of the learner starts a new feed");
        assert!(super::status_of(&learner.me.do_name, lm.boot()).is_none(), "the old boot's feed is gone");
    }

    /// A burst of polls after the owner went stale sends one `Authz` read: the first poll reads, the rest wait for it
    /// and go on when it ends, also when the reading poll is dropped mid-read. Fresh again until the next minute.
    #[test]
    fn a_burst_of_polls_on_a_stale_owner_sends_one_read() {
        let feed = Rc::new(RefCell::new(Feed::default()));
        let now = 1_000_000;
        assert_eq!(feed.borrow_mut().refresh(now, false).0, Refresh::Fresh, "no owner to read");
        let (first, _) = feed.borrow_mut().refresh(now, true);
        assert_eq!(first, Refresh::Read);
        let mut waiting = Vec::new();
        for _ in 0..2_999 {
            let (r, rx) = feed.borrow_mut().refresh(now, true);
            assert_eq!(r, Refresh::Wait);
            waiting.push(rx.expect("a waiter"));
        }
        let guard = Refreshed(feed.clone());
        feed.borrow_mut().owner_at = now;
        drop(guard);
        for mut rx in waiting {
            assert_eq!(rx.try_recv(), Ok(Some(())), "every waiting poll goes on once the read ends");
        }
        assert_eq!(feed.borrow_mut().refresh(now + 60_000, true).0, Refresh::Fresh);
        let (again, _) = feed.borrow_mut().refresh(now + 60_001, true);
        assert_eq!(again, Refresh::Read, "stale again after a minute");
        drop(Refreshed(feed.clone()));
        assert_eq!(
            feed.borrow_mut().refresh(now + 60_002, true).0,
            Refresh::Read,
            "a dropped read leaves none in flight"
        );
    }
}
