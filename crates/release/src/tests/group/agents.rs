//! AM3-WP1's `AgentsReleaseView` in memory (`crates/release/src/agents.rs`): LF-8's three refusals, 100 of 100 each,
//! the view never moving to a lower index, a failed refresh keeping the last view, and the `ControllerPointer` pin.

use graftyard_types::Index;

use super::*;
use crate::agents::{AgentsAnchor, AgentsReleaseView, controller_override, feed};

fn agents_anchor(g: &G) -> AgentsAnchor {
    AgentsAnchor { install: g.a.install, customer: g.a.customer, host: "git.example".into() }
}

/// `GET release/entries?after=<after>&per_page=<per_page>` over the group's log, as the edge answers it.
fn get(g: &G, after: usize, per_page: usize) -> Vec<(Index, Vec<u8>, Timestamp)> {
    let from: Vec<(Vec<u8>, Timestamp)> = g.log[after..].iter().cloned().zip(g.ats[after..].iter().copied()).collect();
    feed(after as u64, &from, per_page).into_iter().map(|(i, b, at)| (i, b.to_vec(), at)).collect()
}

/// A group whose controller is pinned: release 3 and 4 carry the agents scripts; a `ControllerRoll` to 3 ran to its
/// end, then a `ControllerRoll` to 4 whose pre-signed `ControllerPointer` back to 3 committed after its 1% step.
fn controller_pinned() -> G {
    let mut g = G::new();
    for n in 3..=4 {
        let b = g.entry(G::upload_body(n, &[Script::AgentsFront, Script::Controller]), &[&AGENTS]);
        g.ok(b);
    }
    let b = g.intent(1, Intent::ControllerRoll { release: rel(3).0 }, &[], &VENDOR);
    g.ok(b);
    for s in 0..2 {
        let b = g.entry(ReleaseEntry::IntentStep { intent: id(1), step: s }, &[&AGENTS]);
        g.ok(b);
    }
    let b = g.entry(ReleaseEntry::IntentDone { intent: id(1) }, &[&AGENTS]);
    g.ok(b);
    let pin = ReleaseEntry::ControllerPointer { pin: rel(3).0 };
    let b = g.intent(2, Intent::ControllerRoll { release: rel(4).0 }, std::slice::from_ref(&pin), &VENDOR);
    g.ok(b);
    let b = g.entry(ReleaseEntry::IntentStep { intent: id(2), step: 0 }, &[&AGENTS]);
    g.ok(b);
    let b = g.undo(2, &pin, &VENDOR);
    g.ok(b);
    g
}

#[test]
fn the_agents_view_replays_the_feed_in_pages_and_pins_the_controller_only_while_the_pointer_is_set() {
    let g = controller_pinned();
    let a = agents_anchor(&g);
    let all = get(&g, 0, 100);
    assert_eq!((all[0].0, all.len()), (Index(1), g.log.len()), "index 1 is the genesis");
    assert_eq!(get(&g, 2, 1000).len(), g.log.len() - 2, "per_page is at most 100, and the page starts after `after`");
    assert_eq!(get(&g, 2, 0)[0].0, Index(3), "per_page 0 reads one entry");
    // A view read 3 at a time ends where one read whole does, on the group's own fold.
    let mut v = AgentsReleaseView::default();
    while (v.index().0 as usize) < g.log.len() {
        let page = get(&g, v.index().0 as usize, 3);
        assert_eq!(v.extend(&a, &page).unwrap(), page.len() as u64);
    }
    assert_eq!((v.index(), v.fold()), (Index(g.log.len() as u64), &g.f));
    assert_eq!(v.current(), Some(id(2)));
    assert_eq!(v.keys(), g.f.keys.as_ref());
    let pinned = vid(3, 1);
    assert_eq!(v.controller(), Some(pinned));
    assert_eq!(controller_override(&v, "graft-controller"), Some(format!("graft-controller=\"{pinned}\"")));
    // One entry earlier, before the pointer, no override: the deployment's split applies.
    let mut before = AgentsReleaseView::default();
    before.extend(&a, &all[..all.len() - 1]).unwrap();
    assert_eq!((before.controller(), controller_override(&before, "graft-controller")), (None, None));
}

/// LF-8: an entry signed for another install, a `Keys` not signed by the anchored customer key, and an entry out of
/// index order, 100 of 100 of each refused by the agents view, and each leaves the view as it was.
#[test]
fn lf8_the_agents_view_refuses_another_install_a_keys_not_by_the_customer_and_an_entry_out_of_order() {
    let g = controller_pinned();
    let a = agents_anchor(&g);
    let mut held = AgentsReleaseView::default();
    held.extend(&a, &get(&g, 0, 100)).unwrap();
    let next = Index(held.index().0 + 1);
    let at = Timestamp(g.at.0 + 60_000);
    let refused = |v: &AgentsReleaseView, page: &[(Index, Vec<u8>, Timestamp)]| {
        let mut w = v.clone();
        let r = w.extend(&a, page);
        assert_eq!(&w, v, "a refused entry moves nothing");
        r.expect_err("accepted")
    };
    // The group's real next entry, valid at index `next`: the executor's step 0 of the undo the pointer started.
    let real = g.entry(ReleaseEntry::IntentStep { intent: id(2), step: 0 }, &[&AGENTS]);
    assert_eq!(held.clone().extend(&a, &[(next, real.clone(), at)]), Ok(1), "the control is accepted in order");
    let (mut other, mut keys, mut order) = (0, 0, 0);
    for k in 0..100u8 {
        // 1. Another install's entry: the same step bound to install 2 for a warm view, or install 2's own genesis,
        // signed by its own customer key, as a cold view's first entry.
        let refusal = if k % 2 == 0 {
            let b = Bound { install: install(2), group: ChainId::Release, prev: g.f.last, parent: None };
            let mut e = SignedEntry::unsigned(b, ReleaseEntry::IntentStep { intent: id(2), step: 0 });
            e.sign(&AGENTS);
            refused(&held, &[(next, e.encode(), at)])
        } else {
            let seed = [100 + k; 32];
            refused(&AgentsReleaseView::default(), &[(Index(1), signed(genesis_for(install(2), &seed), &seed), T0)])
        };
        if let Refusal::WrongInstall { .. } = refusal {
            other += 1;
        }
        // 2. A Keys at the next generation signed by a key that is not the current customer key: the vendor, the
        // executor, the agents executor, a stranger, or a new customer key alone.
        let stranger = [150 + k / 5; 32];
        let signer: &[u8; 32] = match k % 5 {
            0 => &VENDOR,
            1 => &EXECUTOR,
            2 => &AGENTS,
            3 => &stranger,
            _ => &NEW_CUSTOMER,
        };
        let mut new = g.f.keys.clone().unwrap();
        new.generation += 1;
        if k % 5 == 4 {
            new.customer = public_key(&NEW_CUSTOMER);
        }
        let bytes = g.entry(ReleaseEntry::Keys(Box::new(new)), &[signer]);
        if matches!(refused(&held, &[(next, bytes, at)]), Refusal::UnknownKey(_) | Refusal::WrongSigner("Keys")) {
            keys += 1;
        }
        // 3. Out of index order: the same valid next entry offered 1 to 7 indexes past the next.
        let page = [(Index(next.0 + 1 + u64::from(k % 7)), real.clone(), at)];
        if refused(&held, &page) == Refusal::BadState("a release/entries page skips an index") {
            order += 1;
        }
    }
    assert_eq!((other, keys, order), (100, 100, 100));
    // An entry out of order inside a page: its entries before the gap are taken, the rest are not.
    let mut v = AgentsReleaseView::default();
    let mut page = get(&g, 0, 100);
    page.remove(4);
    assert!(v.extend(&a, &page).is_err());
    assert_eq!(v.index(), Index(4));
    // A history whose first Keys after genesis was not signed by the customer key is refused at that entry, so a
    // view never adopts a chain of Keys that starts anywhere but the anchored customer key.
    let mut cold = AgentsReleaseView::default();
    let other_anchor = AgentsAnchor { customer: public_key(&OTHER), ..a.clone() };
    assert!(cold.extend(&other_anchor, &get(&g, 0, 100)).is_err());
    assert_eq!(cold.index(), Index(0));
}

/// The view never moves to a lower index: a member behind it answers entries it holds, which add nothing; and a
/// refresh that fails, or answers an entry the view refuses, keeps the last view, pin included.
#[test]
fn the_agents_view_never_moves_to_a_lower_index_and_keeps_its_last_view_when_a_refresh_fails() {
    let g = controller_pinned();
    let a = agents_anchor(&g);
    let n = g.log.len();
    let mut v = AgentsReleaseView::default();
    v.extend(&a, &get(&g, 0, 100)).unwrap();
    let held = v.clone();
    // A behind member: the same entries, or an older prefix, from index 1.
    assert_eq!(v.extend(&a, &get(&g, 0, 100)[..n - 3]).unwrap(), 0);
    assert_eq!(v.extend(&a, &get(&g, 2, 100)).unwrap(), 0);
    assert_eq!(v, held);
    // A refresh that fails: no page at all, or a page whose first new entry is refused. The view is unchanged.
    assert_eq!(v.extend(&a, &[]).unwrap(), 0);
    let mut bad = g.entry(ReleaseEntry::IntentStep { intent: id(2), step: 0 }, &[&AGENTS]);
    let last = bad.len() - 1;
    bad[last] ^= 1;
    assert!(v.extend(&a, &[(Index(n as u64 + 1), bad, Timestamp(g.at.0 + 60_000))]).is_err());
    assert_eq!(v, held);
    assert_eq!(controller_override(&v, "graft-controller"), controller_override(&held, "graft-controller"));
    // The group's next entry then moves it on.
    let mut g2 = g;
    let b = g2.entry(ReleaseEntry::IntentStep { intent: id(2), step: 0 }, &[&AGENTS]);
    g2.ok(b);
    assert_eq!(v.extend(&a, &get(&g2, n, 100)).unwrap(), 1);
    assert_eq!(v.fold(), &g2.f);
}

/// OPEN-QUESTIONS A55 (LF-8 r1; DECISIONS "`agents-equivocation`"): an answer at an index the view holds must carry the
/// bytes the view applied there. The same bytes are a member behind and add nothing; other bytes at k (the last held)
/// or k-1 are two signed histories, refused as `Equivocation` naming the index, which pages, and the view, its index
/// and its pin do not move.
#[test]
fn a55_other_bytes_at_a_held_index_are_equivocation_and_the_same_bytes_are_skipped() {
    let g = controller_pinned();
    let a = agents_anchor(&g);
    let n = g.log.len();
    let mut v = AgentsReleaseView::default();
    v.extend(&a, &get(&g, 0, 100)).unwrap();
    let held = v.clone();
    // The other bytes are a valid next entry, the group's own, so only the index they are offered at is wrong.
    let next = g.entry(ReleaseEntry::IntentStep { intent: id(2), step: 0 }, &[&AGENTS]);
    let at = Timestamp(g.at.0 + 60_000);
    // An equal replay of k and of k-1, alone or with the entries before them: nothing taken, nothing refused.
    for k in [n, n - 1] {
        assert_eq!(v.extend(&a, &get(&g, k - 1, 1)), Ok(0), "an equal replay of {k}");
        assert_eq!(v.extend(&a, &get(&g, 0, 100)[..k]), Ok(0));
    }
    assert_eq!(v, held);
    for k in [n, n - 1] {
        let mut other = get(&g, 0, 100)[..k].to_vec();
        other[k - 1].1 = next.clone();
        for page in [other.clone(), other[k - 1..].to_vec()] {
            let r = v.extend(&a, &page).expect_err("other bytes at a held index");
            assert_eq!(r, Refusal::Equivocation { index: k as u64 });
            assert!(r.pages(), "an equivocation pages");
            assert_eq!(v, held, "index {k}: the view does not move");
        }
    }
    assert_eq!((v.index(), v.controller()), (held.index(), Some(vid(3, 1))));
    // After an equal replay of every held index, the next entry still lands.
    assert_eq!(v.extend(&a, &[get(&g, 0, 100), vec![(Index(n as u64 + 1), next, at)]].concat()), Ok(1));
}

/// What a reader parses from the edge's page is what `feed` served: the same indexes, bytes, and times, so the view
/// replays it to the group's fold; a page with a bad entry is refused whole.
#[test]
fn the_edges_page_parses_back_to_the_feed() {
    use base64::Engine as _;
    let g = controller_pinned();
    let page = get(&g, 2, 5);
    let b64 = base64::engine::general_purpose::STANDARD;
    let rows: Vec<serde_json::Value> =
        page.iter().map(|(i, e, at)| serde_json::json!({ "index": i.0, "entry": b64.encode(e), "at": at.0 })).collect();
    let body = serde_json::to_vec(&rows).unwrap();
    assert_eq!(crate::agents::parse_page(&body).unwrap(), page);
    assert_eq!(crate::agents::parse_page(b"[]").unwrap(), vec![]);
    for bad in [&b"{}"[..], b"[{\"index\":3,\"at\":1}]", b"[{\"index\":3,\"entry\":\"!!\",\"at\":1}]", b"nope"] {
        assert!(crate::agents::parse_page(bad).is_err(), "{}", String::from_utf8_lossy(bad));
    }
}

/// DECISIONS "`controller-default`": `CONTROLLER_DEFAULT` is N while no controller version serves 100%, whichever
/// order the deployments API lists the split in, and absent once one does; a settings change keeps every other binding.
#[test]
fn the_front_default_is_n_only_while_a_split_is_deployed_and_keeps_the_fronts_other_bindings() {
    use crate::agents::{CONTROLLER_DEFAULT, controller_default, front_bindings, front_default};
    let (n, n1) = (vid(3, 1), vid(4, 1));
    assert_eq!(controller_default(&[(n, 99), (n1, 1)]), Some(n));
    assert_eq!(controller_default(&[(n1, 1), (n, 99)]), Some(n));
    assert_eq!(controller_default(&[(n1, 100)]), None);
    assert_eq!(controller_default(&[(n, 0), (n1, 100)]), None);
    assert_eq!(controller_default(&[]), None);
    let src = serde_json::json!({ "resources": { "bindings": [
        { "type": "secret_text", "name": "FRONT_TOKEN" },
        { "type": "plain_text", "name": "CONTROLLER_SCRIPT", "text": "graft-controller" },
        { "type": "plain_text", "name": CONTROLLER_DEFAULT, "text": n1.to_string() },
    ] } });
    assert_eq!(front_default(&src), Some(n1));
    let set = front_bindings(&src, Some(n));
    let names: Vec<&str> = set.iter().map(|b| b["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["FRONT_TOKEN", "CONTROLLER_SCRIPT", CONTROLLER_DEFAULT]);
    assert_eq!(set[0], src["resources"]["bindings"][0], "the secret is sent as read, so it keeps its value");
    let after = serde_json::json!({ "resources": { "bindings": set } });
    assert_eq!(front_default(&after), Some(n));
    let cleared = serde_json::json!({ "resources": { "bindings": front_bindings(&after, None) } });
    assert_eq!((front_default(&cleared), cleared["resources"]["bindings"].as_array().unwrap().len()), (None, 2));
}
