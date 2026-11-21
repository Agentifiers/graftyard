//! A retired member stays retired (`decisions/DECISIONS.md`, "`retire-sticks`"). A retire (the test route `/retire`, a
//! deleted repository's `/purge_retire`) deletes the member's alarm and storage and then writes one row, `retired` in
//! `me`, which survives the object's reset. A member with that row refuses every `/join`. Without it the object was a
//! fresh one: the edge's create on first use joined the same bootstrap names again, each checked the repository's
//! still-live `RepoOwner` row, and an empty group came back at generation 1 (graft-m1 `repo:1048577`, retired at
//! 07:09Z and joined again at 10:41:48Z on 2026-11-20 by one `git-upload-pack`).
//!
//! A placement try the group never took is cleared with `{"reuse": true}` (`Steps::clear`): no row, because the next
//! re-placement starts again at the committed placement's next generation and joins the same names.
//!
//! A member that holds no group takes a retire too (`unjoined`), and writes the row. A group retired before this row
//! existed left its members not joined and without it, and an unjoined member answered every path but `/ping`,
//! `/status`, `/rtt` and `/join` 409, so a second retire could not close it: graft-m1 `repo:1048597`, retired on
//! 2026-11-19, was joined again by a read's create on first use and its archive alarm started three shards at
//! 00:31:42Z on 2026-11-21 (`decisions/DECISIONS.md`, "`retire-sticks`", Finding).

use serde_json::{Value, json};
use worker::*;

/// The row's key in the member's `me` table.
const KEY: &str = "retired";

/// When this member was retired (ms), from its `me` table.
pub fn at(sql: &SqlStorage) -> Option<u64> {
    let rows: Vec<Value> = sql.exec("SELECT v FROM me WHERE k = ?", vec![KEY.into()]).ok()?.to_array().ok()?;
    rows.first()?.get("v")?.as_str()?.parse().ok()
}

/// A `/join`'s refusal from a retired member, or `None` when it may join.
pub fn refusal(retired: Option<u64>) -> Option<Value> {
    retired.map(|at| json!({ "retired": at }))
}

/// What a member that holds no group does with a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unjoined {
    Ping,
    Status,
    Rtt,
    /// A retire: `wipe`, with the row unless the test route's `{"reuse": true}`. `/purge_retire` always writes it.
    Retire {
        purge: bool,
    },
    /// 409 "member not joined".
    Refuse,
}

/// The route of a request to a member that holds no group (`/join` is answered before this). The test route
/// `/retire` exists only on a test-faults build, as on a joined member.
pub fn unjoined(method: &Method, path: &str, test_faults: bool) -> Unjoined {
    match (method, path) {
        (Method::Get, "/ping") => Unjoined::Ping,
        (Method::Get, "/status") => Unjoined::Status,
        (Method::Get, "/rtt") => Unjoined::Rtt,
        (Method::Post, "/purge_retire") => Unjoined::Retire { purge: true },
        (Method::Post, "/retire") if test_faults => Unjoined::Retire { purge: false },
        _ => Unjoined::Refuse,
    }
}

/// Deletes the member's alarm and storage, then writes the `retired` row unless `reuse`. The caller resets the object.
pub async fn wipe(state: &State, now: u64, reuse: bool) -> Result<()> {
    let storage = state.storage();
    storage.delete_alarm().await?;
    storage.delete_all().await?;
    if !reuse {
        let sql = storage.sql();
        sql.exec("CREATE TABLE IF NOT EXISTS me (k TEXT PRIMARY KEY, v TEXT NOT NULL)", None)?;
        sql.exec("INSERT OR REPLACE INTO me (k, v) VALUES (?, ?)", vec![KEY.into(), now.to_string().into()])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Unjoined, unjoined};
    use worker::Method;

    /// A member that holds no group takes either retire and nothing else: main's unjoined member answered `/retire`
    /// and `/purge_retire` 409, so a group retired before the row existed could never be given it.
    #[test]
    fn an_unjoined_member_takes_a_retire_and_refuses_every_other_route() {
        assert_eq!(unjoined(&Method::Post, "/retire", true), Unjoined::Retire { purge: false });
        assert_eq!(unjoined(&Method::Post, "/purge_retire", true), Unjoined::Retire { purge: true });
        assert_eq!(unjoined(&Method::Post, "/purge_retire", false), Unjoined::Retire { purge: true });
        // The test route stays off a product build, as it is on a joined member.
        assert_eq!(unjoined(&Method::Post, "/retire", false), Unjoined::Refuse);
        assert_eq!(unjoined(&Method::Get, "/retire", true), Unjoined::Refuse);
        for p in ["/read", "/raft", "/commit", "/campaign", "/fold", "/entries", "/reset", "/abort"] {
            assert_eq!(unjoined(&Method::Post, p, true), Unjoined::Refuse, "{p}");
        }
        assert_eq!(unjoined(&Method::Get, "/status", false), Unjoined::Status);
        assert_eq!(unjoined(&Method::Get, "/ping", false), Unjoined::Ping);
        assert_eq!(unjoined(&Method::Get, "/rtt", false), Unjoined::Rtt);
    }
}
