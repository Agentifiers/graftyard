//! A retired member stays retired (`decisions/DECISIONS.md`, "`retire-sticks`"). A retire (the test route `/retire`, a
//! deleted repository's `/purge_retire`) deletes the member's alarm and storage and then writes one row, `retired` in
//! `me`, which survives the object's reset. A member with that row refuses every `/join`. Without it the object was a
//! fresh one: the edge's create on first use joined the same bootstrap names again, each checked the repository's
//! still-live `RepoOwner` row, and an empty group came back at generation 1 (graft-m1 `repo:1048577`, retired at
//! 07:09Z and joined again at 10:41:48Z on 2026-11-20 by one `git-upload-pack`).
//!
//! A placement try the group never took is cleared with `{"reuse": true}` (`Steps::clear`): no row, because the next
//! re-placement starts again at the committed placement's next generation and joins the same names.

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
