//! `LogRelay { job }`'s pure half (`design/SHARED.md`, "Actions", Logs; DESIGN Shape, "Identity"): what the relay
//! does with the runner's `FeedStreamUrl` frames and what each web viewer is sent. The site class owns the sockets.
//!
//! - **Runner frames** are one JSON text message per batch, `{count, value: [lines], stepId, startLine}` [measured,
//!   runner 2.338.2, `tests/m4/results/m4-wp3.md`], with no timestamps, so a line's `at` is when the relay took it.
//! - **Viewers** get the web app's messages (`web/src/areas/actions/ui-api.ts`, `RelayMessage`): `hello` with the
//!   tail when a runner stream is attached, else `no-stream`; then `line` per line, and `end` when the job finished.
//!   `seq` is the line's position in the job's live log, so a viewer that joins late merges the tail without
//!   duplicates.
//! - **Steps** are numbered as the job's `steps`: "Set up job" is 1, the workflow's steps 2 to n + 1, and "Complete
//!   job" n + 2. The runner names a workflow step by the GUID the job message gave it (`runsvc::steps_json`, position
//!   p is `...-{p:012x}`), and the runner's own two steps by GUIDs of their own, before and after the rest.
//! - **Nothing is stored**: the step logs reach the blob store through the results service, so a relay lost with its
//!   site loses only live output, and its viewers fall back to the blobs.

use std::collections::VecDeque;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::ids::JobId;

/// Lines a relay keeps for viewers that join late (DESIGN: 1,000 [guess]).
pub const TAIL_LINES: usize = 1_000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Frame {
    #[serde(default)]
    value: Vec<String>,
    #[serde(default)]
    step_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub seq: u64,
    pub step: u32,
    pub at: String,
    pub text: String,
}

impl Line {
    fn json(&self) -> Value {
        json!({"seq": self.seq, "step": self.step, "at": self.at, "text": self.text})
    }
}

#[derive(Debug)]
pub struct Relay {
    pub job: JobId,
    /// The workflow's step count, n.
    steps: u32,
    tail: VecDeque<Line>,
    next_seq: u64,
    /// Whether a runner stream has attached since the relay started.
    pub attached: bool,
    /// The highest step number seen, which orders the runner's own steps.
    last_step: u32,
    pub ended: Option<String>,
}

/// The workflow step position a job-message step GUID names (`runsvc::steps_json`).
fn position(guid: &str) -> Option<u32> {
    let tail = guid.strip_prefix("00000000-0000-4004-8000-")?;
    (tail.len() == 12).then(|| u32::from_str_radix(tail, 16).ok()).flatten()
}

impl Relay {
    pub fn new(job: JobId, steps: u32) -> Relay {
        Relay { job, steps, tail: VecDeque::new(), next_seq: 0, attached: false, last_step: 0, ended: None }
    }

    fn step_of(&self, guid: &str) -> u32 {
        match position(guid) {
            Some(p) if p >= 1 && p <= self.steps => p + 1,
            _ if self.last_step == 0 || self.last_step == 1 => 1,
            _ => self.steps + 2,
        }
    }

    /// What a viewer is sent on joining.
    pub fn hello(&self) -> Value {
        if !self.attached {
            return json!({"type": "no-stream"});
        }
        json!({"type": "hello", "job_id": self.job.0.0, "tail": self.tail.iter().map(Line::json).collect::<Vec<_>>()})
    }

    /// One runner text message at `at` (ISO): the viewer messages to send. A message that is not a frame sends nothing.
    pub fn frame(&mut self, text: &str, at: &str) -> Vec<Value> {
        self.attached = true;
        let Ok(f) = serde_json::from_str::<Frame>(text) else { return Vec::new() };
        let step = self.step_of(&f.step_id);
        self.last_step = self.last_step.max(step);
        let mut out = Vec::with_capacity(f.value.len());
        for text in f.value {
            let l = Line { seq: self.next_seq, step, at: at.to_string(), text };
            self.next_seq += 1;
            out.push(json!({"type": "line", "line": l.json()}));
            self.tail.push_back(l);
            if self.tail.len() > TAIL_LINES {
                self.tail.pop_front();
            }
        }
        out
    }

    /// The job finished with the web conclusion (`success`, `failure`, `cancelled`, `skipped`).
    pub fn end(&mut self, conclusion: &str) -> Value {
        self.ended = Some(conclusion.to_string());
        json!({"type": "end", "conclusion": conclusion})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graftyard_types::DbId;

    fn frame(step: &str, lines: &[&str]) -> String {
        json!({"count": lines.len(), "value": lines, "stepId": step, "startLine": 1}).to_string()
    }

    #[test]
    fn lines_are_numbered_in_order_and_steps_as_the_jobs_steps() {
        let mut r = Relay::new(JobId(DbId(9)), 2);
        assert_eq!(r.hello(), json!({"type": "no-stream"}), "no runner yet");
        let setup = "bf0a232b-3745-4c26-bda3-e41b07c4d093";
        let s2 = "00000000-0000-4004-8000-000000000002";
        let s1 = "00000000-0000-4004-8000-000000000001";
        r.frame(&frame(setup, &["Current runner version"]), "t0");
        r.frame(&frame(s1, &["a", "b"]), "t1");
        r.frame(&frame(s2, &["c"]), "t2");
        let done = r.frame(&frame("ac053a9b-394c-4988-bc17-06942fab1de2", &["Cleaning up"]), "t3");
        assert_eq!(
            done[0]["line"],
            json!({"seq": 4, "step": 4, "at": "t3", "text": "Cleaning up"}),
            "Complete job is n + 2"
        );
        let h = r.hello();
        let steps: Vec<(u64, u64)> = h["tail"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| (l["seq"].as_u64().unwrap(), l["step"].as_u64().unwrap()))
            .collect();
        assert_eq!(steps, [(0, 1), (1, 2), (2, 2), (3, 3), (4, 4)]);
        assert_eq!(r.end("success"), json!({"type": "end", "conclusion": "success"}));
    }

    #[test]
    fn the_tail_keeps_the_last_lines_and_their_numbers() {
        let mut r = Relay::new(JobId(DbId(9)), 1);
        let s1 = "00000000-0000-4004-8000-000000000001";
        let many: Vec<String> = (0..TAIL_LINES + 5).map(|i| i.to_string()).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        r.frame(&frame(s1, &refs), "t");
        let tail = r.hello()["tail"].as_array().unwrap().clone();
        assert_eq!(tail.len(), TAIL_LINES);
        assert_eq!(tail[0]["seq"], json!(5), "the oldest five dropped, the rest keep their seq");
        assert!(r.frame("not json", "t").is_empty());
    }
}
