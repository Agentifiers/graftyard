//! Prints what a gate checks in an owner's `Authz` fold, from its `dep` read's hex on stdin: the owner, the opened
//! kind, the enterprise, and each org member's role (`tests/m3/hosted/rows.ts`, BF-30 and BF-34).

use std::io::Read;

use graftyard_identity::{AuthzFold, AuthzRead, UserId};
use graftyard_types::{AccountId, DbId};

fn main() {
    let mut hex = String::new();
    std::io::stdin().read_to_string(&mut hex).expect("stdin");
    let hex = hex.trim();
    let bytes: Vec<u8> =
        (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex")).collect();
    let f: AuthzFold = graftyard_raft::codec::from_bytes(&bytes).expect("an Authz fold");
    let roles: serde_json::Map<String, serde_json::Value> = std::env::args()
        .skip(1)
        .map(|a| {
            let u = UserId(AccountId(DbId(a.parse().expect("account id"))));
            (a, serde_json::json!(f.role(u).map(|r| format!("{r:?}"))))
        })
        .collect();
    let out = serde_json::json!({
        "owner": f.owner().map(|o| o.0.0),
        "opened": f.opened().map(|k| format!("{k:?}")),
        "enterprise": f.enterprise().map(|e| e.0.0),
        "roles": roles,
    });
    println!("{out}");
}
