//! One App's record from its owner's `Authz` fold (the `dep` read's hex on stdin), for the `HostedRunnersAppKey`
//! end-to-end row (`tests/m5/a17/hrak.sh`). `PutApp` replaces the whole record, so a key change carries every other
//! field as it stands, the webhook and its wrapped secret included.
//!
//!   appkeys <app id> show                     the record's key ids, created times and SPKI SHA-256 prefixes
//!   appkeys <app id> add <key id> <spki.der>  the `PutApp` op with that public key added (at most two)
//!   appkeys <app id> only <key id>            the `PutApp` op keeping that key alone
//!
//! Only public halves are read or printed.

use std::io::Read;

use graftyard_identity::{AppId, AppKey, AuthzFold, AuthzRead, RsaPublic};
use graftyard_types::{DbId, Timestamp};
use serde_json::json;
use sha2::Digest as _;

fn h8(b: &[u8]) -> String {
    sha2::Sha256::digest(b)[..8].iter().map(|x| format!("{x:02x}")).collect()
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: appkeys <app id> show | add <key id> <spki.der> | only <key id>";
    let app = AppId(DbId(a.first().expect(usage).parse().expect("app id")));
    let mut hex = String::new();
    std::io::stdin().read_to_string(&mut hex).expect("stdin");
    let hex = hex.trim();
    let bytes: Vec<u8> =
        (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex")).collect();
    let f: AuthzFold = graftyard_raft::codec::from_bytes(&bytes).expect("an Authz fold");
    let mut r = f.app(app).expect("no such App on this fold").clone();
    match a.get(1).map(String::as_str) {
        Some("show") => {
            let keys: Vec<_> = r
                .keys
                .iter()
                .map(|k| json!({ "id": k.id.0, "created": k.created.0, "spki_sha256": h8(&k.public.0) }))
                .collect();
            let out = json!({ "app": app.0.0, "slug": r.slug.as_str(), "keys": keys,
                "webhook": r.webhook.as_ref().map(|u| &*u.0), "webhook_secret": r.webhook_secret.is_some(),
                "events": r.events.iter().map(|e| &*e.0).collect::<Vec<_>>(), "client_secrets": r.client_secrets.len() });
            println!("{out}");
        }
        Some("add") => {
            let id = DbId(a.get(2).expect(usage).parse().expect("key id"));
            let der = std::fs::read(a.get(3).expect(usage)).expect("spki.der");
            assert!(r.keys.iter().all(|k| k.id != id), "the record already has key {}", id.0);
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_millis();
            r.keys.push(AppKey { id, public: RsaPublic(der.into()), created: Timestamp(now as u64) });
            assert!(r.keys.len() <= 2, "PutApp takes at most two keys");
            println!("{}", json!({ "PutApp": { "record": r } }));
        }
        Some("only") => {
            let id = DbId(a.get(2).expect(usage).parse().expect("key id"));
            r.keys.retain(|k| k.id == id);
            assert_eq!(r.keys.len(), 1, "the record has no key {}", id.0);
            println!("{}", json!({ "PutApp": { "record": r } }));
        }
        _ => panic!("{usage}"),
    }
}
