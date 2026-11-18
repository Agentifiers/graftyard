//! The member's clock: this isolate's clock, raised to the freshest stamp a peer or the edge sent (`design/DESIGN.md`,
//! Shape, `DRIFT`). A busy isolate's `Date.now()` reads the time the runtime last polled for I/O, which can be up to
//! seconds before the event it is handling (`tests/am1/results/lease-clock.md`, the probe). Every `/raft` call and
//! reply, and every edge request to a member, carries its sender's clock (`STAMP`); the floor is the largest stamp this
//! boot has seen, and every input steps at the later of the two. A stamp from a sender whose clock runs ahead raises
//! the floor by at most that sender's drift, which DRIFT already budgets.
//!
//! The same readings judge the read lease. Each stamp minus the member clock just before it, the clock a lease reads,
//! is the lag it showed then; an isolate clock the floor already covers is no lag (clock-guard). While the largest of
//! the last LAG_WINDOW lags is DRIFT / 2 or more, the clock is suspect, and the leader grants reads only by a round sent
//! after the read arrived (`raft::Member::set_clock_suspect`).

use std::collections::VecDeque;

/// The header that carries a sender's clock, in ms since the epoch, on member calls, their replies, and edge requests.
pub const STAMP: &str = graftyard_lookup::MEMBER_CLOCK;

/// Lags at or above this make the clock suspect: half of DRIFT, so a lag that grows by as much again between a lease's
/// start and a read still leaves the lease inside DRIFT [guess].
pub const LAG_SUSPECT_MS: u64 = graftyard_raft::DRIFT.as_millis() as u64 / 2;

/// The lags the guard looks at: one fresh stamp does not clear a clock that the stamps before it showed lagging.
pub const LAG_WINDOW: usize = 8;

/// One member's clock floor and its recent lags. Owned by the host, one per boot.
#[derive(Debug, Default)]
pub struct Clock {
    floor: u64,
    last: u64,
    lags: VecDeque<u64>,
}

impl Clock {
    /// The clock an input steps at: the later of this isolate's clock and the floor, never below a reading it gave
    /// before.
    pub fn now(&mut self, own: u64) -> u64 {
        self.last = self.last.max(own).max(self.floor);
        self.last
    }

    /// A stamp a sender put on a call or reply, read against the member clock at its arrival (this isolate's clock
    /// `own`, the floor, and every reading given out). Records the lag it showed, then raises the floor. A stamp at or
    /// below the member clock records a lag of 0.
    pub fn stamp(&mut self, stamp: u64, own: u64) -> u64 {
        let lag = stamp.saturating_sub(self.last.max(own).max(self.floor));
        self.floor = self.floor.max(stamp);
        if self.lags.len() == LAG_WINDOW {
            self.lags.pop_front();
        }
        self.lags.push_back(lag);
        lag
    }

    /// The largest of the last LAG_WINDOW lags, in ms.
    pub fn lag(&self) -> u64 {
        self.lags.iter().copied().max().unwrap_or(0)
    }

    /// Whether lease reads must wait for a round.
    pub fn suspect(&self) -> bool {
        self.lag() >= LAG_SUSPECT_MS
    }

    /// The lags recorded, oldest first, for `/stats`.
    pub fn lags(&self) -> impl Iterator<Item = u64> + '_ {
        self.lags.iter().copied()
    }
}

/// A stamp header's value, if it parses.
pub fn parse(v: Option<String>) -> Option<u64> {
    v.and_then(|v| v.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A busy isolate whose own clock reads 1,200 ms behind a peer's stamp steps at the stamp, and its later readings
    /// never fall back below it.
    #[test]
    fn a_stale_own_clock_steps_at_the_newer_stamp() {
        let mut c = Clock::default();
        assert_eq!(c.now(10_000), 10_000);
        c.stamp(11_200, 10_000);
        assert_eq!(c.now(10_000), 11_200, "the newer stamp did not raise the clock");
        assert_eq!(c.now(10_500), 11_200, "the clock fell back to the isolate's");
        assert_eq!(c.now(11_300), 11_300, "the isolate's own clock, once ahead, wins");
    }

    /// A stamp older than the clock never moves it back, and records no lag.
    #[test]
    fn an_older_stamp_never_moves_the_clock_back() {
        let mut c = Clock::default();
        assert_eq!(c.now(20_000), 20_000);
        c.stamp(19_000, 20_000);
        assert_eq!(c.now(20_000), 20_000);
        assert_eq!(c.lag(), 0);
        // Monotonic even when this isolate's own reading steps back.
        assert_eq!(c.now(19_500), 20_000);
        // Two stamps between inputs, newer first: the older one leaves the floor at the newer.
        c.stamp(22_000, 20_000);
        c.stamp(21_000, 20_000);
        assert_eq!(c.now(20_000), 22_000, "an older stamp lowered the floor");
    }

    /// The guard: suspect from a lag at DRIFT / 2, not below it, and held until LAG_WINDOW later stamps show none.
    #[test]
    fn the_guard_trips_at_half_drift_and_holds_for_the_window() {
        let mut c = Clock::default();
        c.stamp(1_000 + LAG_SUSPECT_MS - 1, 1_000);
        assert!(!c.suspect(), "a lag under DRIFT / 2 tripped the guard");
        c.stamp(2_000 + LAG_SUSPECT_MS, 2_000);
        assert!(c.suspect(), "a lag of DRIFT / 2 left lease reads on");
        for k in 0..LAG_WINDOW - 1 {
            c.stamp(3_000 + k as u64, 3_000 + k as u64);
            assert!(c.suspect(), "one fresh stamp cleared the guard after {} of {LAG_WINDOW}", k + 1);
        }
        c.stamp(4_000, 4_000);
        assert!(!c.suspect(), "LAG_WINDOW fresh stamps left the guard on");
    }

    /// clock-guard: the lag is a stamp past the member clock, the clock a lease reads, not past this isolate's own.
    /// B0-29 `mwlbozdl`'s leader read its own clock 256 ms or more behind 97.4% of stamps while each stamp's floor kept
    /// its member clock within 85 ms (p99) of the next, and its reads all took rounds.
    #[test]
    fn a_stamp_the_member_clock_already_covers_shows_no_lag() {
        let mut c = Clock::default();
        assert_eq!(c.now(10_000), 10_000);
        // A reply 1,000 ms ahead: the member clock was behind it, so that is a lag.
        assert_eq!(c.stamp(11_000, 10_000), 1_000);
        assert!(c.suspect());
        // This isolate's clock stays 1,000 ms behind, but each stamp is only 1 ms past the floor the last one set.
        for k in 1..=LAG_WINDOW as u64 {
            assert_eq!(c.stamp(11_000 + k, 10_000), 1, "a stale own clock under a fresh floor counted as lag");
        }
        assert!(!c.suspect(), "the guard held for a lag the floor already covered");
        // A stamp 293 ms past the member clock, with the own clock still behind: a real lag again.
        assert_eq!(c.stamp(11_008 + 293, 10_000), 293);
        assert!(c.suspect(), "a stamp past the member clock by DRIFT / 2 left lease reads on");
        // A reading given out counts too: the member clock read 20,000 and the isolate's own clock fell back to 19,000.
        let mut c = Clock::default();
        assert_eq!(c.now(20_000), 20_000);
        assert_eq!(c.stamp(20_200, 19_000), 200, "the lag ignored the member clock's last reading");
        assert!(!c.suspect());
    }
}
