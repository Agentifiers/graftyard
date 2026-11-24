//! `graft-agents-front`'s view (`design/AGENT-LAYER.md`, Shape, "Controller"; `build/BUILD-PLAN.md`, AM3-WP1, LF-7's
//! pure half), over a real `Release` group built through the release crate's public API and read as the edge pages it
//! (`agents::feed`, then the edge's JSON, then `agents::parse_page`): the pin only after a committed
//! `ControllerPointer`, a failed or refused refresh keeping the last view and pin with no request waiting, and a member
//! behind the view moving nothing.

use base64::Engine as _;
use graftyard_agents_front::{Page, Reader, next_after, plan};
use graftyard_release::agents::{AgentsAnchor, ENTRIES_PER_PAGE, feed, parse_page};
use graftyard_release::view::{Need, POINTER_TTL_MS, RETRY_MS};
use graftyard_release::{
    Anchor, Bound, Format, FormatSet, Genesis, Intent, IntentId, Keys, ReleaseEntry, ReleaseFold, ReleaseUploaded,
    Script, SignedEntry, SignedIntent, VersionId, presign_undo, public_key, sign_record, undo_entry,
};
use graftyard_types::{
    AccountId, ChainId, DbId, Index, PlacementName, PlacementRecord, Region, ReleaseId, Site, SiteId, SiteLink,
    SystemChain, Timestamp, Ulid,
};
use sha2::{Digest as _, Sha256};
use smallvec::SmallVec;

const CUSTOMER: [u8; 32] = [7; 32];
const VENDOR: [u8; 32] = [2; 32];
const RELEASE: [u8; 32] = [3; 32];
const DRAIN: [u8; 32] = [4; 32];
const EXECUTOR: [u8; 32] = [5; 32];
const AGENTS: [u8; 32] = [6; 32];
const T0: Timestamp = Timestamp(1_790_000_000_000);
const HOUR: u64 = 3_600_000;
const SCRIPT: &str = "graft-controller";

fn install() -> graftyard_release::InstallId {
    graftyard_release::InstallId(Ulid::new(T0, [1; 10]))
}

fn rel(n: u8) -> (ReleaseId, Vec<u8>) {
    let file = reads(n).file();
    let rec =
        format!(r#"{{"label":"r{n}","files":[{{"name":"formats.txt","sha256":"{}"}}]}}"#, hex(&Sha256::digest(file)));
    (ReleaseId(Sha256::digest(rec.as_bytes()).into()), rec.into_bytes())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn reads(n: u8) -> FormatSet {
    FormatSet::new((1..=u16::from(n)).map(|v| Format { family: 32, version: v }))
}

/// The controller's version in release `n`.
fn vid(n: u8) -> VersionId {
    VersionId([n, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1])
}

fn id(n: u8) -> IntentId {
    IntentId(Ulid(u128::from(n) + 1000))
}

fn site(id: u8, region: Region, spare: bool) -> Site {
    Site {
        id: SiteId(id),
        script: format!("graft-site-{id}"),
        account: "acct".into(),
        region,
        jurisdiction: None,
        bucket: format!("graft-packs-{id}"),
        release: None,
        link: SiteLink::Binding { member_class: "Member".into() },
        spare,
    }
}

fn genesis() -> Vec<u8> {
    let tri = PlacementRecord {
        name: PlacementName::Tri,
        sites: [SiteId(0), SiteId(1), SiteId(2)].into_iter().collect(),
        home: SiteId(0),
        generation: 1,
        moving_from: None,
        at: Timestamp(5),
    };
    let g = Genesis {
        install: install(),
        account: AccountId(DbId(1)),
        keys: Keys {
            generation: 0,
            customer: public_key(&CUSTOMER),
            vendor: Some(public_key(&VENDOR)),
            release: public_key(&RELEASE),
            drain: public_key(&DRAIN),
            executor: public_key(&EXECUTOR),
            agents_executor: Some(public_key(&AGENTS)),
            ring: SmallVec::new(),
        },
        sites: [
            site(0, Region::Enam, false),
            site(1, Region::Weur, false),
            site(2, Region::Apac, false),
            site(3, Region::Eeur, true),
        ]
        .into_iter()
        .collect(),
        placements: [SystemChain::Release, SystemChain::Names, SystemChain::Tokens, SystemChain::Ids]
            .into_iter()
            .map(|c| (c, tri.clone()))
            .collect(),
        evaluation: false,
        restore: None,
    };
    let bound = Bound { install: install(), group: ChainId::Release, prev: None, parent: None };
    let mut e = SignedEntry::unsigned(bound, ReleaseEntry::Genesis(Box::new(g)));
    e.sign(&CUSTOMER);
    e.encode()
}

/// The install's `Release` group as its members run it: each entry decided on the fold, then applied, and logged with
/// the time of the entry it committed in.
struct Group {
    fold: ReleaseFold,
    at: Timestamp,
    log: Vec<(Vec<u8>, Timestamp)>,
}

impl Group {
    fn new() -> Group {
        let mut g = Group { fold: ReleaseFold::default(), at: T0, log: Vec::new() };
        g.ok(genesis());
        g
    }

    fn anchor() -> Anchor {
        Anchor { install: install(), customer: public_key(&CUSTOMER) }
    }

    fn ok(&mut self, bytes: Vec<u8>) {
        self.at = Timestamp(self.at.0 + 60_000);
        let ev = self.fold.decide(&Group::anchor(), self.at, &bytes).expect("the group accepts it");
        self.fold.apply(&ev, self.at);
        self.log.push((bytes, self.at));
    }

    fn entry(&self, body: ReleaseEntry, seed: &[u8; 32]) -> Vec<u8> {
        let b = Bound { install: install(), group: ChainId::Release, prev: self.fold.last, parent: None };
        let mut e = SignedEntry::unsigned(b, body);
        e.sign(seed);
        e.encode()
    }

    fn upload(&mut self, n: u8) {
        let (release, record) = rel(n);
        let body = ReleaseEntry::ReleaseUploaded(Box::new(ReleaseUploaded {
            release,
            record_sig: sign_record(&RELEASE, &record),
            record: record.into(),
            versions: [
                (Script::Controller, vid(n)),
                (Script::AgentsFront, VersionId([n, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1])),
            ]
            .into_iter()
            .collect(),
            images: SmallVec::new(),
            reads: reads(n),
        }));
        let b = self.entry(body, &AGENTS);
        self.ok(b);
    }

    fn roll(&mut self, n: u8, to: u8, undo: &[ReleaseEntry]) {
        let undo = undo.iter().map(|u| (u.clone(), presign_undo(install(), id(n), u, &VENDOR))).collect();
        let body = ReleaseEntry::Intent(Box::new(SignedIntent {
            id: id(n),
            issued_at: self.at,
            not_after: Timestamp(self.at.0 + 4 * HOUR),
            intent: Intent::ControllerRoll { release: rel(to).0 },
            undo,
        }));
        let b = self.entry(body, &VENDOR);
        self.ok(b);
    }

    fn step(&mut self, n: u8, step: u8) {
        let b = self.entry(ReleaseEntry::IntentStep { intent: id(n), step }, &AGENTS);
        self.ok(b);
    }

    fn done(&mut self, n: u8) {
        let b = self.entry(ReleaseEntry::IntentDone { intent: id(n) }, &AGENTS);
        self.ok(b);
    }

    /// The pre-signed `ControllerPointer` undo of intent `n` back to release `to`, as `graft-ops` submits it.
    fn pin(&mut self, n: u8, to: u8) {
        let u = pointer(to);
        let b = undo_entry(install(), id(n), &u, public_key(&VENDOR), presign_undo(install(), id(n), &u, &VENDOR));
        self.ok(b.encode());
    }

    /// `GET release/entries?after=<after>&per_page=<per_page>` as the edge answers it, read back by `parse_page`.
    fn get(&self, after: usize, per_page: usize) -> Page {
        let page = feed(after as u64, &self.log[after..], per_page);
        let rows: Vec<serde_json::Value> = page
            .iter()
            .map(|(i, e, at)| {
                serde_json::json!({ "index": i.0, "entry": base64::engine::general_purpose::STANDARD.encode(e), "at": at.0 })
            })
            .collect();
        parse_page(serde_json::to_string(&rows).unwrap().as_bytes()).expect("the edge's page parses")
    }
}

fn pointer(to: u8) -> ReleaseEntry {
    ReleaseEntry::ControllerPointer { pin: rel(to).0 }
}

fn agents_anchor() -> AgentsAnchor {
    AgentsAnchor { install: install(), customer: public_key(&CUSTOMER), host: "git.example".into() }
}

/// Releases 3 and 4 uploaded; a `ControllerRoll` to 3 ran to its end; a `ControllerRoll` to 4, carrying the pre-signed
/// `ControllerPointer` back to 3, at its 1% step. The pointer is not committed yet.
fn rolling() -> Group {
    let mut g = Group::new();
    g.upload(3);
    g.upload(4);
    g.roll(1, 3, &[]);
    g.step(1, 0);
    g.step(1, 1);
    g.done(1);
    g.roll(2, 4, &[pointer(3)]);
    g.step(2, 0);
    g
}

/// A reader that read `g` whole in one refresh at `start`.
fn read(g: &Group, start: u64) -> Reader {
    let mut r = Reader::default();
    assert_eq!(r.need(start), Need::Wait, "a cold isolate waits for its first view");
    r.start(start, true);
    r.apply(&agents_anchor(), Ok(vec![g.get(0, 100)]), start, start + 50);
    assert_eq!((r.oks(), r.fails(), r.view().index()), (1, 0, Index(g.log.len() as u64)));
    r
}

#[test]
fn pages_in_order_pin_the_controller_only_after_a_committed_controller_pointer() {
    let a = agents_anchor();
    let mut g = rolling();
    // The view read 3 entries a page, each page after the last index taken, ends on the group's own fold.
    let mut r = Reader::default();
    let mut pages = Vec::new();
    while pages.iter().map(|p: &Page| p.len()).sum::<usize>() < g.log.len() {
        pages.push(g.get(pages.iter().map(|p: &Page| p.len()).sum(), 3));
    }
    assert!(pages.len() > 2);
    r.start(0, true);
    r.apply(&a, Ok(pages), 0, 10);
    assert_eq!((r.view().index(), r.view().fold()), (Index(g.log.len() as u64), &g.fold));
    assert_eq!(r.view().current(), Some(id(2)));
    // The roll to 4 is at 1%, and no pointer has committed: no override, so the deployment's split applies.
    assert_eq!((r.view().controller(), plan(r.view(), None, SCRIPT).1), (None, None));
    // The pointer commits; the next refresh reads from the view's index and pins N.
    let held = g.log.len();
    g.pin(2, 3);
    let t = POINTER_TTL_MS;
    assert_eq!(r.need(t), Need::Wait, "a view POINTER_TTL old is refreshed before the request");
    r.start(t, false);
    r.apply(&a, Ok(vec![g.get(held, 100)]), t, t + 10);
    assert_eq!(r.view().controller(), Some(vid(3)));
    assert_eq!(plan(r.view(), None, SCRIPT).1, Some(format!("{SCRIPT}=\"{}\"", vid(3))));
    // The undo's step and done keep the pin; the next ControllerRoll clears it.
    let held = g.log.len();
    g.step(2, 0);
    g.done(2);
    g.upload(5);
    let before_roll = g.log.len();
    // The pin left N = 3 in the deployment, so the next roll's undo points back to 3.
    g.roll(3, 5, &[pointer(3)]);
    let mut kept = r.clone();
    kept.apply(&a, Ok(vec![g.get(held, 100)[..before_roll - held].to_vec()]), 2 * t, 2 * t);
    assert_eq!(plan(kept.view(), None, SCRIPT).1, Some(format!("{SCRIPT}=\"{}\"", vid(3))));
    r.apply(&a, Ok(vec![g.get(held, 100)]), 2 * t, 2 * t);
    assert_eq!((r.view().index(), plan(r.view(), None, SCRIPT).1), (Index(g.log.len() as u64), None));
}

#[test]
fn a_failed_or_refused_refresh_keeps_the_view_and_its_pin_and_no_request_waits() {
    let a = agents_anchor();
    let mut g = rolling();
    g.pin(2, 3);
    let r0 = read(&g, 0);
    let pinned = plan(r0.view(), None, SCRIPT).1;
    assert!(pinned.is_some());
    // A blackholed or 503 read, then a page whose next entry does not verify, each at a time the view is due.
    let mut bad = g.entry(ReleaseEntry::IntentStep { intent: id(2), step: 0 }, &AGENTS);
    let last = bad.len() - 1;
    bad[last] ^= 1;
    let refused: Page = vec![(Index(g.log.len() as u64 + 1), bad, Timestamp(g.at.0 + 60_000))];
    let not_listed = vec![(
        Index(g.log.len() as u64 + 1),
        g.entry(ReleaseEntry::IntentStep { intent: id(9), step: 0 }, &AGENTS),
        g.at,
    )];
    for (k, pages) in [Err("503 no Release member answered".to_string()), Ok(vec![refused]), Ok(vec![not_listed])]
        .into_iter()
        .enumerate()
    {
        let mut r = r0.clone();
        let t = POINTER_TTL_MS + 1;
        assert_eq!(r.need(t), Need::Wait);
        r.start(t, true);
        r.apply(&a, pages, t, t + 20);
        assert_eq!(r.view(), r0.view(), "case {k}: the view held stays");
        assert_eq!(plan(r.view(), None, SCRIPT).1, pinned, "case {k}: and so does its pin");
        assert_eq!((r.oks(), r.fails(), r.fresh().failing), (1, 1, true), "case {k}");
        assert!(r.last_error().is_some());
        // Until RETRY_MS the view is served as held; from then on each request refreshes beside itself, however long
        // the install stays down, and none waits.
        assert_eq!(r.need(t + 20 + RETRY_MS - 1), Need::Use, "case {k}");
        for later in [t + 20 + RETRY_MS, t + 10 * POINTER_TTL_MS, t + 600_000] {
            assert_eq!(r.need(later), Need::Refresh, "case {k}: at {later}");
        }
        // A second failure beside requests still never makes one wait.
        let u = t + 20 + RETRY_MS;
        r.start(u, false);
        assert_eq!(r.need(u + 1), Need::Use, "a refresh beside requests in flight leaves them on the view");
        r.apply(&a, Err("timed out".into()), u, u + 1_000);
        assert_eq!((r.fails(), r.need(u + 1_000 + RETRY_MS + POINTER_TTL_MS)), (2, Need::Refresh));
        // Once the install answers, the view moves on and requests stop refreshing until it is due again.
        let v = u + 5_000;
        r.start(v, false);
        r.apply(&a, Ok(vec![g.get(g.log.len(), 100)]), v, v + 10);
        assert_eq!(
            (r.fresh().failing, r.need(v + 10), plan(r.view(), None, SCRIPT).1),
            (false, Need::Use, pinned.clone())
        );
    }
    // A cold isolate whose first read fails holds no view, forwards unpinned, and does not wait again.
    let mut cold = Reader::default();
    cold.start(0, true);
    cold.apply(&a, Err("401".into()), 0, VIEW_CALL);
    assert_eq!((cold.view().index(), plan(cold.view(), None, SCRIPT).1), (Index(0), None));
    assert_eq!(cold.need(VIEW_CALL + RETRY_MS), Need::Refresh);
}

const VIEW_CALL: u64 = graftyard_release::view::VIEW_CALL_MS;

#[test]
fn a_page_from_a_member_behind_the_view_moves_nothing() {
    let a = agents_anchor();
    let mut g = rolling();
    let before_pin = g.log.len();
    g.pin(2, 3);
    let r0 = read(&g, 0);
    let n = g.log.len();
    // A member one entry behind the pointer, read from the start or from part way, and an empty page.
    let behind = g.get(0, 100)[..before_pin].to_vec();
    for pages in [
        vec![behind.clone()],
        vec![g.get(2, 100)[..before_pin - 2].to_vec()],
        vec![vec![]],
        vec![behind.clone(), behind],
    ] {
        let mut r = r0.clone();
        let t = POINTER_TTL_MS;
        r.start(t, false);
        r.apply(&a, Ok(pages), t, t);
        assert_eq!(r.view(), r0.view());
        assert_eq!((r.view().index(), r.view().controller()), (Index(n as u64), Some(vid(3))));
        assert_eq!(r.fails(), 0, "an answer behind the view is an answer, not a failure");
    }
    // Other bytes at an index the view holds, such as a history from before the pointer with its last entry replaced:
    // still nothing.
    let mut older = g.get(0, 100);
    older[n - 1] = (Index(n as u64), g.log[n - 2].0.clone(), g.log[n - 2].1);
    let mut r = r0.clone();
    r.apply(&a, Ok(vec![older]), 1, 1);
    assert_eq!((r.view(), plan(r.view(), None, SCRIPT).1), (r0.view(), plan(r0.view(), None, SCRIPT).1));
}

#[test]
fn a_refresh_reads_the_next_page_only_after_a_full_one() {
    let g = rolling();
    let full: Page = (1..=ENTRIES_PER_PAGE as u64).map(|i| (Index(10 + i), Vec::new(), T0)).collect();
    assert_eq!(next_after(Index(10), &full), Some(Index(10 + ENTRIES_PER_PAGE as u64)));
    assert_eq!(next_after(Index(10), &full[..ENTRIES_PER_PAGE - 1].to_vec()), None, "a short page is the last");
    assert_eq!(next_after(Index(10), &Vec::new()), None);
    // A full page that does not move past `after` (a member behind) ends the refresh instead of repeating it.
    let stale: Page = (1..=ENTRIES_PER_PAGE as u64).map(|i| (Index(i), Vec::new(), T0)).collect();
    assert_eq!(next_after(Index(500), &stale), None);
    // The real feed: a short page after the view's index ends it.
    assert_eq!(next_after(Index(3), &g.get(3, 100)), None);
}

/// LF-7 r1 and OPEN-QUESTIONS A53 (DECISIONS "`controller-default`"): while a `ControllerRoll`'s split is deployed the
/// front carries N as `CONTROLLER_DEFAULT`. An isolate with no view, or whose first read fails, or whose view predates
/// the roll, pins N; one whose verified view shows the roll follows the split; a committed pointer wins over both.
#[test]
fn an_isolate_with_no_view_under_a_controller_roll_pins_the_default_until_a_view_lands() {
    let a = agents_anchor();
    let mut g = rolling();
    let n = Some(vid(3));
    let pin_n = (n, Some(format!("{SCRIPT}=\"{}\"", vid(3))));
    let cold = Reader::default();
    assert_eq!(plan(cold.view(), n, SCRIPT), pin_n, "no view: N, never the split");
    let mut failed = Reader::default();
    failed.start(0, true);
    failed.apply(&a, Err("blackholed".into()), 0, VIEW_CALL);
    assert_eq!(plan(failed.view(), n, SCRIPT), pin_n, "a first read that fails keeps N");
    // A view read before the roll's intent committed (the first 7 entries end on intent 1's done) shows no roll.
    let mut before = Reader::default();
    before.start(0, true);
    before.apply(&a, Ok(vec![g.get(0, 100)[..7].to_vec()]), 0, 10);
    assert_eq!((before.view().current(), plan(before.view(), n, SCRIPT)), (None, pin_n.clone()));
    // The view lands: the roll is running, so the deployment's split applies.
    let mut r = cold.clone();
    r.start(1, true);
    r.apply(&a, Ok(vec![g.get(0, 100)]), 1, 10);
    assert_eq!(r.view().current(), Some(id(2)));
    assert_eq!(plan(r.view(), n, SCRIPT), (None, None), "a verified view of the roll follows the split");
    // The pointer commits: the pin is the pointer's, whatever the variable says.
    let held = g.log.len();
    g.pin(2, 3);
    r.start(POINTER_TTL_MS, false);
    r.apply(&a, Ok(vec![g.get(held, 100)]), POINTER_TTL_MS, POINTER_TTL_MS + 10);
    assert_eq!(plan(r.view(), Some(vid(4)), SCRIPT).0, Some(vid(3)));
    // With no roll's split deployed the variable is absent, and nothing pins.
    assert_eq!(plan(Reader::default().view(), None, SCRIPT), (None, None));
}
