//! alarm-steps: a repository member's alarm logs each of its steps that took ALARM_STEP_LOG_MS or more, so a long
//! alarm names its step. On B0-35 m60 the leader's alarm ran 2,288 ms of CPU (05:23:27.121 to 29.583Z) and nothing
//! said which of its eight steps it was (`decisions/OPEN-QUESTIONS.md`, F104). The isolate's clock moves only on I/O,
//! so `mark` ends each step with a zero-length timer [doc, Workers, "Performance and timers"]; that also yields, once
//! per step.

/// A step that took at least this long is logged [guess: above the leader's longest alarm in each minute of B0-35
/// m60's load before its last, 40 to 400 ms].
pub const ALARM_STEP_LOG_MS: u64 = 100;

/// One alarm's step times: `mark` after each step, `line` once at the end. A step under ALARM_STEP_LOG_MS is left
/// out, so an ordinary alarm logs nothing.
pub struct AlarmSteps {
    at: u64,
    slow: Vec<(&'static str, u64)>,
    total: u64,
}

impl AlarmSteps {
    pub fn start(now: u64) -> AlarmSteps {
        AlarmSteps { at: now, slow: Vec::new(), total: 0 }
    }

    pub async fn mark(&mut self, step: &'static str) {
        worker::Delay::from(std::time::Duration::ZERO).await;
        self.ended(step, crate::now_ms());
    }

    fn ended(&mut self, step: &'static str, now: u64) {
        let ms = now.saturating_sub(self.at);
        self.at = now;
        self.total += ms;
        if ms >= ALARM_STEP_LOG_MS {
            self.slow.push((step, ms));
        }
    }

    pub fn line(&self) -> Option<String> {
        if self.slow.is_empty() {
            return None;
        }
        let steps: Vec<String> = self.slow.iter().map(|(s, ms)| format!("{s} {ms}ms")).collect();
        Some(format!("alarm steps over {ALARM_STEP_LOG_MS}ms: {}; alarm {}ms", steps.join(", "), self.total))
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
}
