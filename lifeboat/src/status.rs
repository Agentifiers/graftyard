//! `LifeboatStatus` (`design/DESIGN.md`, Shape, "Lifeboat feed"): the release whose formats the copy decodes, and how
//! far behind it is, served in `/api/v3/meta` and the `X-Graftyard-Lifeboat-Lag` header of every response.
//!
//! `cursor_age` is the time since the copy last confirmed it was current: since the last pass after which no known
//! chain had more to pull, from the feed or the edge. A pass that applied rows but left some behind does not reset it,
//! so the lag never reads lower than the copy's oldest unread commit could be. Past `LAG_ALARM_MS` the lifeboat alarms
//! (`design/DESIGN.md`, "Failure modes", `lifeboat` provider-wide: "It serves its last state with its lag published").

use std::time::Duration;

use graftyard_types::ReleaseId;
use serde::{Deserialize, Serialize};

pub const LAG_HEADER: &str = "x-graftyard-lifeboat-lag";
/// The lag the lifeboat alarms at [guess, as the vault's `LAG_ALARM_MS`].
pub const LAG_ALARM_MS: u64 = 5 * 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifeboatStatus {
    pub decoder: ReleaseId,
    pub cursor_age: Duration,
}

impl LifeboatStatus {
    /// The status at `now` for a copy last current at `current_at` (0 if it never was: the whole time since the epoch,
    /// which no reader takes for fresh).
    pub fn at(decoder: ReleaseId, current_at: u64, now: u64) -> LifeboatStatus {
        LifeboatStatus { decoder, cursor_age: Duration::from_millis(now.saturating_sub(current_at)) }
    }

    /// The header's value: whole seconds behind, rounded up, so a copy a moment behind never reads 0.
    pub fn header(&self) -> String {
        self.cursor_age.as_millis().div_ceil(1000).to_string()
    }

    pub fn alarms(&self) -> bool {
        self.cursor_age.as_millis() > u128::from(LAG_ALARM_MS)
    }

    /// The fields `/api/v3/meta` adds, beside GitHub's own, under `graftyard_lifeboat`.
    pub fn meta(&self) -> serde_json::Value {
        let hex: String = self.decoder.0.iter().map(|b| format!("{b:02x}")).collect();
        serde_json::json!({ "graftyard_lifeboat": { "decoder": hex, "lag_seconds": self.header().parse::<u64>().unwrap_or(0) } })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_rounds_up_and_the_alarm_starts_past_five_minutes() {
        let r = ReleaseId([3; 32]);
        assert_eq!(LifeboatStatus::at(r, 1_000, 1_000).header(), "0");
        assert_eq!(LifeboatStatus::at(r, 1_000, 1_001).header(), "1");
        assert_eq!(LifeboatStatus::at(r, 1_000, 31_000).header(), "30");
        assert!(!LifeboatStatus::at(r, 0, LAG_ALARM_MS).alarms());
        assert!(LifeboatStatus::at(r, 0, LAG_ALARM_MS + 1).alarms());
        // A clock behind the last confirmation reads 0, never a wrap.
        assert_eq!(LifeboatStatus::at(r, 5_000, 1_000).header(), "0");
        assert_eq!(LifeboatStatus::at(r, 0, 61_500).meta()["graftyard_lifeboat"]["lag_seconds"], 62);
    }
}
