//! The controller's per-task Durable Object, `TaskBox`, as pure rules (`design/AGENT-LAYER.md`, Shape, "Controller";
//! `build/BUILD-PLAN.md`, AM3-WP1, LM-3's "a Sandbox killed mid-task resumes"; DECISIONS "`controller-taskbox`"). A
//! `TaskBox` holds one `Task`, its one container (the Sandbox), and one alarm. `graft-controller`'s Workers host keeps
//! the box in the object's storage and runs `TaskBox::on` for every report and alarm, so these rules are the whole of what
//! the object decides.
//!
//! - **One Sandbox.** A Durable Object has one container, so a task never holds two Sandboxes. A restart destroys the
//!   container before it starts the next under a new `SandboxId`, `<task>-<generation>` (`TaskBox::restart`).
//! - **Late reports.** Every report from the Sandbox (a push the router saw acked, an exit, a model response's usage)
//!   names the `SandboxId` the router's props carry. While the task is not final, one that names any other is refused,
//!   counted in `late`, and logged by the host (`Report::Late`), and moves nothing. A final task refuses every report
//!   as `Final`, uncounted.
//! - **Acked pushes.** `last_push` moves only on the host's `ok` for a ref in the task's refs, read from the
//!   receive-pack response the router forwards back (`acked_pushes`), never from the Sandbox's word.
//! - **The alarm.** Every `HEARTBEAT` the alarm checks the container: running, it renews the inactivity timeout, so
//!   the 6-hour stop never comes from the platform's own idle rule (OPEN-QUESTIONS A10); stopped, it restarts the task
//!   from its last acked push (`TaskEvent::Lost`). A box that has heard nothing from its Sandbox for `SILENT_MAX` also
//!   restarts it.
//! - **The version.** Which version an object runs is Cloudflare's assignment for the deployment, fixed until the
//!   next deployment, and no caller's override changes it [doc, https://developers.cloudflare.com/workers/
//!   versions-and-deployments/gradual-deployments/with-durable-objects/]. So a `ControllerPointer`, which deploys
//!   nothing, leaves a box on N+1. A box remembers the version that started its Sandbox, and an alarm on another
//!   version restarts it there, so a deployment moves a running task's code at its next alarm.
//! - **The hold.** Each alarm also carries the `ControllerPointer` a caught-up `graft-agents-front` view holds, never
//!   its `CONTROLLER_DEFAULT`, so a roll's ordinary split holds nothing. A pointer to another version than the box
//!   runs holds the box: its Sandbox is destroyed and every report and outbound request is late. The first alarm with
//!   no such pointer (the object now runs the pointer's version, after the agents `graft-ops` deploys it, or the
//!   pointer is gone) releases it and starts the next Sandbox from the last acked push. An alarm that could not read a
//!   caught-up view (`Pointer::Unread`) changes no hold, so a failed read neither holds a box nor releases one. A box
//!   held for `HOLD_MAX` fails its task (DECISIONS "`controller-taskbox`", Versions).
//! - **The base's task.** The base's `AgentTask` follows the box: after each change the host sends `PATCH
//!   .../tasks/{id}` with the state `TaskState::api` gives, under `graftyard-agent`'s token, until the base answered it
//!   (`TaskBox::unreported`, `TaskBox::answered`; OPEN-QUESTIONS A59). `Cancelled` is never sent: only the cancel
//!   endpoint sets it, and it is what started the box's own cancel.
//! - **The host pause.** The base pauses and resumes a task itself (`decisions/DECISIONS.md`, "`host-pause`"): a
//!   non-writer's title or body edit sets it `waiting_for_user`, and a writer's reassignment to the controller's bot
//!   queues it again. On `issues.edited` and `issues.assigned` for the task's issue the host reads the base's task and
//!   hands its state to the box (`Input::Base`), which follows it with no permission check of its own: a pause stops
//!   the Sandbox, and a resume starts a new one from the base, on the snapshot the base moved.
//! - **The session log.** The harness reports its log once, before its last report (`crate::session_log`). From then
//!   until the host's push of it is answered (`TaskBox::log_answered`), `unreported` withholds a final state, because
//!   `decide` takes the session ref only while the base reads the task live; a push with no answer is tried again at
//!   each alarm, at most `LOG_TRIES` times in all (AM3-WP4). A host pause drops a log not yet pushed, so a paused and
//!   resumed task gets its one log from the resumed Sandbox; a log pushed before the pause stays the task's one log.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::agent::RefPattern;
use crate::controller::{Resume, SandboxId, StepErr, Task, TaskEvent, TaskState};
use crate::issues::IssueNumber;
use crate::pkt::{Pkt, Reader};
use crate::refs::ObjectId;
use crate::session_log::{LOG_TRIES, Pushed};
use crate::task::TaskApiState;

/// How often the alarm checks the container [guess]. Well inside a Sandbox's 6-hour inactivity stop, and short enough
/// that a lost Sandbox restarts within the 2-minute loss bound plus one push interval.
pub const HEARTBEAT: Duration = Duration::from_secs(30);
/// How long a running Sandbox may send nothing before the box restarts it as lost [guess]: the controller's own
/// scripted pushes and every model response report, so silence this long is a hung harness.
pub const SILENT_MAX: Duration = Duration::from_secs(10 * 60);
/// How long a box waits on a held version before it fails its task [guess]: an hour is past a `ControllerRoll`'s
/// abort on a healthy deploy API, and a task that waits longer has outlived `TASK_WALL` anyway.
pub const HOLD_MAX: Duration = Duration::from_secs(60 * 60);
/// How many tries the host makes at a task's lease and draft before the task goes on without them [guess]: the start
/// and two alarms.
pub const OPEN_TRIES: u8 = 3;
/// The branch the task's draft pull request opens from, under the task's prefix; the harness pushes its work there.
pub fn work_branch(id: &graftyard_identity::SessionId) -> String {
    format!("{}{}/work", crate::task::RUN_PREFIX, id.text())
}
/// The most acked heads a box keeps for its session log's `pushes` [guess]: a 60-minute task pushing every 15 s
/// makes 240.
pub const PUSHES_MAX: usize = 1_024;

/// The task's durable state in its object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskBox {
    pub task: Task,
    /// How many Sandboxes the task has started; the current one is `<task>-<generation>`.
    pub generation: u32,
    /// The `graft-controller` version that started the current Sandbox.
    pub version: Option<String>,
    /// When the current Sandbox last reported, in ms.
    pub heard: u64,
    /// Reports refused because they named a Sandbox the task no longer holds, or came while it was held.
    pub late: u64,
    /// The pointer's version this box is held for, and since when (ms).
    pub hold: Option<(String, u64)>,
    /// The state the base last answered for the task: `Queued` as created, then each `PATCH` it answered. A box stored
    /// before A59 reads `None` and sends its state once.
    #[serde(default)]
    pub base: Option<TaskApiState>,
    /// The heads the host saw acked, in order, at most `PUSHES_MAX`: the session log's `pushes`.
    #[serde(default)]
    pub pushes: Vec<ObjectId>,
    /// The session log's push.
    #[serde(default)]
    pub log: LogState,
    /// The model tokens of every inference response the router forwarded for this task.
    #[serde(default)]
    pub model: crate::outbound::ModelUsage,
    /// Whether the host holds the task's Shared lease (A3 step 6).
    #[serde(default)]
    pub leased: bool,
    /// The task's draft pull request (A3 step 6), once the host opened it.
    #[serde(default)]
    pub draft: Option<IssueNumber>,
    /// How many tries at the lease and the draft failed, at most `OPEN_TRIES`.
    #[serde(default)]
    pub open_tries: u8,
}

/// Where a task's session log is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogState {
    /// The harness has not reported one.
    #[default]
    Unreported,
    /// Reported and stored; `tries` pushes so far got no answer.
    Pending { tries: u8 },
    /// Answered: pushed, refused, or `Retry` when `LOG_TRIES` pushes got no answer.
    Done(Pushed),
}

/// One input to the box.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Input {
    /// A report from a Sandbox, by the `SandboxId` the router's props carried.
    Report { from: SandboxId, event: TaskEvent },
    /// The alarm: whether the container runs, the version this object runs now, and `graft-agents-front`'s
    /// `ControllerPointer`.
    Alarm { running: bool, version: String, pointer: Pointer },
    /// The base's task, read after an `issues.edited` or `issues.assigned` delivery for its issue: its `state`, and
    /// the version this object runs now, for a resume's Sandbox.
    Base { state: TaskApiState, version: String },
}

/// The `ControllerPointer` as the alarm read it from `graft-agents-front`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pointer {
    /// No caught-up view answered (a failed read, or a view that is a prefix of the history): a held box stays held, so
    /// one failed read never restarts a box on the version a swap-back left; any other box goes on as with `Clear`.
    Unread,
    /// A caught-up view holds no `ControllerPointer`.
    Clear,
    /// A caught-up view's `Pointers.controller`.
    To(String),
}

/// What the host does after an input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Act {
    /// Nothing but store the box.
    Keep,
    /// Renew the container's inactivity timeout.
    Renew,
    /// Destroy the container if it runs, then start a new one under `sandbox`, from `from`.
    Start { sandbox: SandboxId, from: Resume },
    /// Destroy the container: the task is final, or held for another version.
    Stop,
}

/// Why a report moved nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Report {
    /// It named `from`, which is not the task's Sandbox: the host logs it and `late` counts it.
    Late { from: SandboxId },
    /// The task refused the event (`Task::step`).
    Refused(StepErr),
}

impl TaskBox {
    pub fn new(task: Task) -> TaskBox {
        TaskBox {
            task,
            generation: 0,
            version: None,
            heard: 0,
            late: 0,
            hold: None,
            base: Some(TaskApiState::Queued),
            pushes: Vec::new(),
            log: LogState::Unreported,
            model: Default::default(),
            leased: false,
            draft: None,
            open_tries: 0,
        }
    }

    fn next_id(&mut self) -> SandboxId {
        self.generation += 1;
        SandboxId(format!("{}-{}", self.task.id.text(), self.generation).into())
    }

    fn current(&self) -> Option<&SandboxId> {
        match &self.task.state {
            TaskState::Running { sandbox, .. } => Some(sandbox),
            _ => None,
        }
    }

    /// Starts a queued task's first Sandbox on `version`.
    pub fn start(&mut self, version: &str, now: u64) -> Result<Act, StepErr> {
        let mut next = self.clone();
        let sandbox = next.next_id();
        next.task.step(TaskEvent::Started { sandbox: sandbox.clone() })?;
        next.version = Some(version.into());
        next.heard = now;
        *self = next;
        Ok(Act::Start { sandbox, from: Resume::Base })
    }

    /// Restarts a running task's Sandbox on `version`, from its last acked push.
    fn restart(&mut self, version: &str, now: u64) -> Act {
        let Some(lost) = self.current().cloned() else { return Act::Keep };
        let sandbox = self.next_id();
        self.task.step(TaskEvent::Lost { lost, sandbox: sandbox.clone() }).expect("a running task takes its own loss");
        self.version = Some(version.into());
        self.heard = now;
        Act::Start { sandbox, from: self.task.resume().expect("running") }
    }

    /// Whether traffic from `from` at `now` is the current Sandbox's: its outbound requests and its reports alike. A
    /// final task refuses all of it as `Final`, uncounted; otherwise a Sandbox the task does not hold, or any while
    /// the box is held, is `Late` and counted. Admitted traffic counts as the Sandbox's activity.
    pub fn admit(&mut self, from: &SandboxId, now: u64) -> Result<(), Report> {
        if self.task.state.is_final() {
            return Err(Report::Refused(StepErr::Final));
        }
        if self.hold.is_some() || self.current() != Some(from) {
            self.late += 1;
            return Err(Report::Late { from: from.clone() });
        }
        self.heard = now;
        Ok(())
    }

    /// The first-party delivery's `cancelled`: the task ends `Cancelled` and its Sandbox stops. A final task changes
    /// nothing, and its container is stopped anyway.
    pub fn cancel(&mut self) -> Act {
        let _ = self.task.step(TaskEvent::Cancelled);
        self.hold = None;
        Act::Stop
    }

    /// The state the base's task should read and has not answered: `TaskState::api`'s, unless the base already
    /// answered it or the task is `Cancelled`. A final state waits while the session log's push is unanswered.
    pub fn unreported(&self) -> Option<TaskApiState> {
        let s = self.task.state.api().filter(|s| self.base != Some(*s))?;
        (!(s.is_final() && self.log_due())).then_some(s)
    }

    /// The harness reported its session log: `true` for the first report, which the host stores and pushes; `false`
    /// for any later one, which the host refuses, since the session ref is created once.
    pub fn log_reported(&mut self) -> bool {
        if self.log != LogState::Unreported {
            return false;
        }
        self.log = LogState::Pending { tries: 0 };
        true
    }

    /// Whether the host still owes the session log's push.
    pub fn log_due(&self) -> bool {
        matches!(self.log, LogState::Pending { .. })
    }

    /// The host's push of the session log was answered `p`: a push or a refusal ends it, and a push with no answer is
    /// tried again until `LOG_TRIES` have had none.
    pub fn log_answered(&mut self, p: Pushed) {
        let LogState::Pending { tries } = self.log else { return };
        self.log = match p {
            Pushed::Retry if tries + 1 < LOG_TRIES => LogState::Pending { tries: tries + 1 },
            p => LogState::Done(p),
        };
    }

    /// The base answered the `PATCH` for `state` with `status`. A 2xx took it. A 4xx refused it for good, as for a task
    /// the base already reads final, or a paused task its App may not resume (`crate::task::decide`, `UpdateTask`), so
    /// the box does not send it again. A 5xx, like no answer, leaves it for the next alarm.
    pub fn answered(&mut self, state: TaskApiState, status: u16) {
        if status < 500 {
            self.base = Some(state);
        }
    }

    /// The host could not mint the token for what the box owes the base: the state `Some(state)`'s `PATCH`, or the
    /// session log's push for `None`. The base answered the token request `status`. A 4xx is a refusal for good, as
    /// for a task the base no longer reads live (a cancelled one, F145), so it settles the debt as a 4xx from the base
    /// itself would. Anything else is no answer: the state stays owed, and the log's push counts one try with none.
    pub fn unminted(&mut self, owed: Option<TaskApiState>, status: u16) {
        let refused = (400..500).contains(&status);
        match owed {
            Some(state) => self.answered(state, if refused { status } else { 503 }),
            None => self.log_answered(if refused { Pushed::Refused } else { Pushed::Retry }),
        }
    }

    /// Whether the host should take the task's Shared lease or open its draft now (A3 step 6): a running task that
    /// lacks either, under `OPEN_TRIES` failed tries.
    pub fn open_due(&self) -> bool {
        (!self.leased || self.draft.is_none())
            && self.open_tries < OPEN_TRIES
            && matches!(self.task.state, TaskState::Running { .. })
    }

    /// The host's try at A3 step 6: whether the lease is held now, and the draft it opened, if any. A new draft makes
    /// the base owe the task's state again, so the next `PATCH` carries `pr_number`. A try that leaves either missing
    /// counts toward `OPEN_TRIES`; a task goes on without them after that.
    pub fn opened(&mut self, leased: bool, draft: Option<IssueNumber>) {
        self.leased |= leased;
        if let Some(n) = draft.filter(|_| self.draft.is_none()) {
            self.draft = Some(n);
            self.base = None;
        }
        if !self.leased || self.draft.is_none() {
            self.open_tries = self.open_tries.saturating_add(1);
        }
    }

    /// Takes one input at `now` (ms).
    pub fn on(&mut self, input: Input, now: u64) -> Result<Act, Report> {
        match input {
            Input::Report { from, event } => {
                self.admit(&from, now)?;
                let head = match &event {
                    TaskEvent::Pushed { head } => Some(*head),
                    _ => None,
                };
                self.task.step(event).map_err(Report::Refused)?;
                if let Some(h) = head
                    && self.pushes.len() < PUSHES_MAX
                {
                    self.pushes.push(h);
                }
                self.heard = now;
                Ok(if self.task.state.is_final() { Act::Stop } else { Act::Keep })
            }
            Input::Base { state, version } => {
                if self.task.state.is_final() {
                    return Ok(Act::Keep);
                }
                let paused = matches!(self.task.state, TaskState::Paused { .. });
                match state {
                    // Paused on the base: the box pauses and its Sandbox stops. The base already reads the state.
                    TaskApiState::WaitingForUser if !paused => {
                        self.task.step(TaskEvent::IssueEdited { by_writer: false }).expect("a live task pauses");
                        self.base = Some(TaskApiState::WaitingForUser);
                        // A log not yet pushed is dropped: `decide` refuses the session ref while the task is paused,
                        // and the resumed Sandbox reports the task's one log. A pushed one stays the task's log.
                        if self.log != LogState::Done(Pushed::Ok) {
                            self.log = LogState::Unreported;
                        }
                        self.hold = None;
                        Ok(Act::Stop)
                    }
                    // Resumed on the base (`TaskResumed`): queued again, and started on a new Sandbox from the base,
                    // since the issue it works from is the base's new snapshot.
                    TaskApiState::Queued if paused => {
                        self.task.state = TaskState::Queued;
                        self.base = Some(TaskApiState::Queued);
                        self.start(&version, now).map_err(Report::Refused)
                    }
                    _ => Ok(Act::Keep),
                }
            }
            Input::Alarm { running, version, pointer } => {
                if self.task.state.is_final() {
                    return Ok(if running { Act::Stop } else { Act::Keep });
                }
                if self.current().is_none() {
                    return Ok(Act::Keep);
                }
                let held = match pointer {
                    Pointer::To(p) => (p != version).then_some(p),
                    Pointer::Unread => self.hold.as_ref().map(|h| h.0.clone()).filter(|p| *p != version),
                    Pointer::Clear => None,
                };
                if let Some(to) = held {
                    let since = self.hold.get_or_insert((to.clone(), now)).1;
                    if now.saturating_sub(since) >= HOLD_MAX.as_millis() as u64 {
                        let reason = format!("held {} min for graft-controller {to}", HOLD_MAX.as_secs() / 60);
                        self.task.step(TaskEvent::Failed { reason: reason.into() }).expect("running");
                        self.hold = None;
                        return Ok(Act::Stop);
                    }
                    return Ok(if running { Act::Stop } else { Act::Keep });
                }
                let silent = now.saturating_sub(self.heard) >= SILENT_MAX.as_millis() as u64;
                if self.hold.take().is_some() || !running || silent || self.version.as_deref() != Some(&version) {
                    return Ok(self.restart(&version, now));
                }
                Ok(Act::Renew)
            }
        }
    }
}

/// What the host answers traffic `admit` refused, and the line it logs: 403 for a Sandbox's outbound request or report
/// that is late (counted in `late`), logged with the task, the Sandbox and the count so far; 409 unlogged for a final
/// task's. LM-3's kill row reads these lines and `late` to show every late report refused, counted, and logged.
pub fn refused(b: &TaskBox, r: &Report) -> (u16, Option<String>) {
    match r {
        Report::Late { from } => (
            403,
            Some(format!(
                "graft-controller: task {} refused late traffic from {} ({} late)",
                b.task.id.text(),
                from.0,
                b.late
            )),
        ),
        Report::Refused(_) => (409, None),
    }
}

/// The new tips the host acknowledged in a receive-pack response, for refs in `refs`: each `ok <ref>` line of the
/// report-status, in or out of side-band 1, paired with the new id the request's command list gave that ref. An
/// `unpack` other than `ok`, an `ng` line, a delete, or a ref outside `refs` yields nothing for that ref; a response
/// that does not parse yields none at all.
pub fn acked_pushes(request: &[u8], response: &[u8], refs: &RefPattern) -> Vec<(String, ObjectId)> {
    let Some(commands) = commands(request) else { return vec![] };
    let Some(lines) = report_lines(response) else { return vec![] };
    if lines.first().map(String::as_str) != Some("unpack ok") {
        return vec![];
    }
    lines
        .iter()
        .filter_map(|l| l.strip_prefix("ok "))
        .filter(|name| refs.matches(name.as_bytes()))
        .filter_map(|name| {
            commands.iter().find(|(n, _)| n == name).and_then(|(n, id)| Some((n.clone(), id.non_zero()?)))
        })
        .collect()
}

/// The request's `(ref, new id)` commands, up to the flush.
fn commands(body: &[u8]) -> Option<Vec<(String, ObjectId)>> {
    let mut r = Reader::new(body);
    let mut out = Vec::new();
    loop {
        let line = match r.next_pkt().ok()?? {
            Pkt::Flush => return Some(out),
            p @ Pkt::Data(_) => p.text()?,
            Pkt::Delim | Pkt::End => return None,
        };
        let cmd = line.split(|&b| b == 0).next()?;
        let mut parts = cmd.splitn(3, |&b| b == b' ');
        let (_old, new, name) = (parts.next()?, parts.next()?, parts.next()?);
        out.push((String::from_utf8(name.to_vec()).ok()?, ObjectId::parse_hex(new)?));
    }
}

/// The report-status lines of a response, from side-band 1 when the response is multiplexed.
fn report_lines(response: &[u8]) -> Option<Vec<String>> {
    let mut r = Reader::new(response);
    let mut inner = Vec::new();
    let mut plain = Vec::new();
    let mut banded = None;
    while let Some(p) = r.next_pkt().ok()? {
        let Pkt::Data(d) = p else { continue };
        let is_band = banded.unwrap_or_else(|| {
            let b = matches!(d.first(), Some(1..=3));
            banded = Some(b);
            b
        });
        if is_band {
            if d[0] == 1 {
                inner.extend_from_slice(&d[1..]);
            }
        } else {
            plain.push(String::from_utf8_lossy(p.text()?).into_owned());
        }
    }
    if banded != Some(true) {
        return Some(plain);
    }
    let mut r = Reader::new(&inner);
    let mut lines = Vec::new();
    while let Some(p) = r.next_pkt().ok()? {
        if let Some(t) = p.text() {
            lines.push(String::from_utf8_lossy(t).into_owned());
        }
    }
    Some(lines)
}

/// The draft's empty commit (A3 step 6): `parent`'s tree with `parent` as its one parent, by `who`, its message naming
/// the task's session page at `origin` (`https://<host>/<owner>/<repo>`), and the `git-receive-pack` request that
/// creates the task's work branch at it. The pack holds the commit alone; the tree and the parent are the base's.
pub fn draft_push(
    id: &graftyard_identity::SessionId,
    parent: ObjectId,
    tree: ObjectId,
    origin: &str,
    who: &crate::init::Signature<'_>,
) -> (ObjectId, Vec<u8>) {
    use sha1::{Digest, Sha1};
    let sig = format!("{} <{}> {} +0000", who.name, who.email, who.at.0 / 1000);
    let msg =
        format!("Graftyard task {}\n\nGraftyard-Session: {origin}/_graftyard/sessions/{}\n", id.text(), id.text());
    let commit = format!("tree {}\nparent {}\nauthor {sig}\ncommitter {sig}\n\n{msg}", tree.hex(), parent.hex());
    let commit_id = crate::init::object_id("commit", commit.as_bytes());
    let mut pack = b"PACK".to_vec();
    pack.extend_from_slice(&2u32.to_be_bytes());
    pack.extend_from_slice(&1u32.to_be_bytes());
    crate::init::entry(&mut pack, 1, commit.as_bytes());
    let trailer: [u8; 20] = Sha1::digest(&pack).into();
    pack.extend_from_slice(&trailer);
    let mut b = Vec::new();
    let cmd = format!(
        "{} {} {}\0report-status agent=graft-controller",
        ObjectId::ZERO.hex(),
        commit_id.hex(),
        work_branch(id)
    );
    crate::pkt::data(&mut b, cmd.as_bytes());
    crate::pkt::flush(&mut b);
    b.extend_from_slice(&pack);
    (commit_id, b)
}
