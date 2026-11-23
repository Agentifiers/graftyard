//! A `Release` group's entries, replayed and checked against an install's anchor, from `/view` pages on stdin (a JSON
//! array of `{"count", "entries": [[hex, at], ...]}` from position 0). BF-46's hosted row reads it
//! (`tests/m5/bf46/view.mjs`). Prints one JSON object: `count`, the last digest, the current intent, every secret's
//! active slot with its switch time, and each entry from position `from` with its time, kind, signer prefixes, and a
//! short form. No entry holds a secret.
//!
//!   relview <install ulid> <customer key hex> <from position> < pages.json

use std::io::Read;

use graftyard_release::view::Replica;
use graftyard_release::{Anchor, ReleaseEntry, SignedEntry};
use graftyard_types::Timestamp;
use serde_json::{Value, json};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex")).collect()
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [install, customer, from] = a.as_slice() else { panic!("usage: relview <install> <customer key> <from>") };
    let anchor = Anchor::from_vars(install, customer).expect("anchor");
    let from: usize = from.parse().expect("from");
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).expect("stdin");
    let pages: Vec<Value> = serde_json::from_str(&s).expect("pages");
    let mut entries = Vec::new();
    for p in &pages {
        for e in p["entries"].as_array().into_iter().flatten() {
            entries.push((unhex(e[0].as_str().expect("hex")), Timestamp(e[1].as_u64().expect("at"))));
        }
    }
    let mut r = Replica::default();
    r.extend(&anchor, 0, &entries).expect("every entry checks");
    let f = &r.fold;
    let shown: Vec<Value> = entries
        .iter()
        .enumerate()
        .skip(from)
        .map(|(i, (b, at))| {
            let e = SignedEntry::decode(b).expect("decodes");
            let short = match &e.body {
                ReleaseEntry::Intent(i) => format!("{:?} id {}", i.intent, i.id.0),
                ReleaseEntry::IntentStep { step, .. } => format!("step {step}"),
                b => b.kind().to_string(),
            };
            json!({ "index": i, "at": at.0, "kind": e.body.kind(), "sigs": e.sigs.iter().map(|(k, _)| k.hex()[..6].to_string()).collect::<Vec<_>>(), "entry": short })
        })
        .collect();
    let slots: serde_json::Map<String, Value> =
        f.slots.iter().map(|(s, (slot, at))| (format!("{s:?}"), json!({ "slot": slot, "switched": at.0 }))).collect();
    let current = f.current.as_deref().map(|c| {
        json!({ "intent": format!("{:?}", c.intent.intent), "state": format!("{:?}", c.state), "next": c.next, "since": c.since.0 })
    });
    let out = json!({
        "count": f.count,
        "prev": f.last.map(|d| d.hex()),
        "current": current,
        "drained": f.drained.keys().collect::<Vec<_>>(),
        "slots": slots,
        "entries": shown,
    });
    println!("{out}");
}
