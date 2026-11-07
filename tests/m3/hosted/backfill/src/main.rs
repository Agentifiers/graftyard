//! The `AccountLogin` backfill for accounts seeded before F50 (`tests/m3/results/m3-hosted.md`, "Backfill"): plans
//! with `finish::account_login` from two `dep` reads and prints the step it owes, if any, as the tools edge's
//! `/commit` body. `backfill.sh` does the reads and the commit.
//!
//!   m3-backfill <account> <login> <login shard dep hex file> <account shard dep hex file>
//!
//! Prints `{"chain": .., "cmd": ..}`, or `null` when nothing is owed.

use graftyard_identity::finish::{Step, account_login};
use graftyard_identity::{Login, NamesFold};
use graftyard_types::{AccountId, DbId};

fn fold(path: &str) -> NamesFold {
    let hex = std::fs::read_to_string(path).expect("read").trim().to_owned();
    if hex.is_empty() || hex == "null" {
        return NamesFold::default();
    }
    let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex")).collect();
    graftyard_raft::codec::from_bytes(&bytes).expect("a Names fold")
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [account, login, login_shard, account_shard] = a.as_slice() else {
        panic!("usage: m3-backfill <account> <login> <login shard hex> <account shard hex>");
    };
    let account = AccountId(DbId(account.parse().expect("account id")));
    let login = Login::parse(login).expect("login");
    let out = match account_login(account, &login, &fold(login_shard), &fold(account_shard)) {
        Some(Step::Names(chain, cmd)) => serde_json::json!({ "chain": chain.to_string(), "cmd": cmd }),
        Some(other) => panic!("account_login planned a non-Names step: {other:?}"),
        None => serde_json::Value::Null,
    };
    println!("{out}");
}
