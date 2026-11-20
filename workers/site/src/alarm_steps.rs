//! alarm-steps: a repository member's alarm logs each of its steps that took ALARM_STEP_LOG_MS or more, so a long
//! alarm names its step. On B0-35 m60 the leader's alarm ran 2,288 ms of CPU (05:23:27.121 to 29.583Z) and nothing
//! said which of its eight steps it was (`decisions/OPEN-QUESTIONS.md`, F104). The isolate's clock moves only on I/O,
//! so `mark` ends each step with a zero-length timer [doc, Workers, "Performance and timers"]; that also yields, once
//! per step.

/// A step that took at least this long is logged [guess: above the leader's longest alarm in each minute of B0-35
/// m60's load before its last, 40 to 400 ms].
pub const ALARM_STEP_LOG_MS: u64 = 100;

/// F109: a member's whole alarm that took at least this long logs every await in it, named by its group [guess, the
/// lead's]. On B0-31 m64r31 the install `Authz` leader's alarms ran 3,784 and 2,451 ms with 0 ms of CPU, and every
/// request it had open ended within 70 ms of their end, while its median alarm took 49 ms
/// (`tests/m4/results/m4-wp4.md`, "B0-31 on graft-m4a after test-faults site day 64").
pub const ALARM_SLOW_MS: u64 = 500;

/// One alarm's step times: `mark` (or `note`) after each step, then `line` or `slow_line` once at the end. `line`
/// names only the steps of ALARM_STEP_LOG_MS or more, so an ordinary alarm logs nothing.
pub struct AlarmSteps {
    at: u64,
    steps: Vec<(&'static str, u64)>,
    total: u64,
}

impl AlarmSteps {
    pub fn start(now: u64) -> AlarmSteps {
        AlarmSteps { at: now, steps: Vec::new(), total: 0 }
    }

    pub async fn mark(&mut self, step: &'static str) {
        worker::Delay::from(std::time::Duration::ZERO).await;
        self.ended(step, crate::now_ms());
    }

    /// `mark` with no timer: for a step that ends on an await of I/O, which already moved the isolate's clock, so
    /// the alarm runs exactly as it did before it was timed. A step of CPU alone reads 0 here.
    pub fn note(&mut self, step: &'static str) {
        self.ended(step, crate::now_ms());
    }

    fn ended(&mut self, step: &'static str, now: u64) {
        let ms = now.saturating_sub(self.at);
        self.at = now;
        self.total += ms;
        self.steps.push((step, ms));
    }

    pub fn line(&self) -> Option<String> {
        let slow: Vec<String> =
            self.steps.iter().filter(|s| s.1 >= ALARM_STEP_LOG_MS).map(|(s, ms)| format!("{s} {ms}ms")).collect();
        if slow.is_empty() {
            return None;
        }
        Some(format!("alarm steps over {ALARM_STEP_LOG_MS}ms: {}; alarm {}ms", slow.join(", "), self.total))
    }

    /// Every step, once the whole alarm took ALARM_SLOW_MS or more (F109), so a long alarm names the await it spent
    /// its time in, or shows that none of them held it.
    pub fn slow_line(&self) -> Option<String> {
        if self.total < ALARM_SLOW_MS {
            return None;
        }
        let steps: Vec<String> = self.steps.iter().map(|(s, ms)| format!("{s} {ms}ms")).collect();
        Some(format!("alarm slow over {ALARM_SLOW_MS}ms: {}; alarm {}ms", steps.join(", "), self.total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a step of ALARM_STEP_LOG_MS or more is named, with the whole alarm's ms; a quick alarm logs nothing.
    #[test]
    fn an_alarm_names_only_its_slow_steps() {
        let mut a = AlarmSteps::start(1_000);
        for (step, at) in [("split", 1_000), ("load", 1_099), ("prsync", 3_387), ("snap", 3_487)] {
            a.ended(step, at);
        }
        assert_eq!(a.line().as_deref(), Some("alarm steps over 100ms: prsync 2288ms, snap 100ms; alarm 2487ms"));
        let mut quick = AlarmSteps::start(0);
        quick.ended("split", 99);
        assert_eq!(quick.line(), None);
    }

    /// F109: an alarm of ALARM_SLOW_MS or more names every await with its ms, the quick ones too, so a reader sees
    /// which one held it; one a millisecond under logs nothing.
    #[test]
    fn a_slow_alarm_names_every_await() {
        let mut a = AlarmSteps::start(10_000);
        for (step, at) in [("family", 10_002), ("tick", 10_002), ("keep_led", 10_002), ("set_alarm", 13_786)] {
            a.ended(step, at);
        }
        a.ended("near_check", 13_786);
        assert_eq!(
            a.slow_line().as_deref(),
            Some(
                "alarm slow over 500ms: family 2ms, tick 0ms, keep_led 0ms, set_alarm 3784ms, near_check 0ms; \
                 alarm 3786ms"
            )
        );
        let mut under = AlarmSteps::start(0);
        under.ended("family", 300);
        under.ended("set_alarm", 499);
        assert_eq!(under.slow_line(), None);
        under.ended("near_check", 500);
        assert!(under.slow_line().is_some_and(|l| l.ends_with("near_check 1ms; alarm 500ms")));
    }
}
