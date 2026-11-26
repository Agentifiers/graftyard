//! `graft-controller`, the GitHub App backend of Graftyard-run agents in the agents account (`design/AGENT-LAYER.md`,
//! Shape, "Controller"; `build/BUILD-PLAN.md`, AM3-WP1). `graft-agents-front` forwards every webhook to it over a
//! `fetch()` service binding. This slice is the intake of journey A3 step 2: check the `X-Hub-Signature-256`, drop a
//! repeated delivery id, and hand the event on as a trigger. `worker` runs it on the Workers runtime.

use std::collections::{HashSet, VecDeque};

#[cfg(target_arch = "wasm32")]
mod taskbox;
#[cfg(target_arch = "wasm32")]
mod worker;

/// How many recent delivery ids one isolate remembers [guess].
pub const SEEN_MAX: usize = 1_000;

/// Whether `header` is `sha256=<hex>` of the HMAC-SHA256 of `body` under `secret`, compared in constant time. A missing
/// header, any other scheme (`sha1=`), a wrong length, or a wrong MAC is false.
pub fn verify(secret: &[u8], body: &[u8], header: Option<&str>) -> bool {
    let Some(got) = header.and_then(|h| h.strip_prefix("sha256=")) else { return false };
    let mac = graftyard_identity::sealed::hmac_sha256(secret, body);
    let want: String = mac.iter().map(|b| format!("{b:02x}")).collect();
    got.len() == want.len() && got.bytes().zip(want.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
}

/// The delivery ids this isolate accepted most recently, at most `SEEN_MAX`, the oldest evicted first.
///
/// `Seen` lives in one isolate, so a redelivery that lands on another isolate, or after this one was evicted, is not
/// dropped here. The durable dedupe is the controller's task store, which is not built yet [inference: A3 step 3's
/// create goes through the task API, which a repeated trigger would reach again until then].
#[derive(Debug, Default)]
pub struct Seen {
    order: VecDeque<String>,
    ids: HashSet<String>,
}

impl Seen {
    /// Records `id`; false when it was already recorded.
    fn insert(&mut self, id: &str) -> bool {
        if self.ids.contains(id) {
            return false;
        }
        if self.order.len() == SEEN_MAX
            && let Some(old) = self.order.pop_front()
        {
            self.ids.remove(&old);
        }
        self.order.push_back(id.to_owned());
        self.ids.insert(id.to_owned());
        true
    }
}

/// What the intake made of one webhook.
#[derive(Debug, PartialEq, Eq)]
pub enum Intake {
    /// A signed delivery not seen before: `X-GitHub-Event`, the payload's `action` if it has one, and
    /// `X-GitHub-Delivery`.
    Accepted { event: String, action: Option<String>, delivery: String },
    /// A delivery id this isolate already accepted (journey A3 step 2, "drops a repeated delivery id").
    Duplicate,
    /// The status to answer and why.
    Refused(u16, &'static str),
}

/// Whether an `issues.assigned` payload assigns the bot account of the App `app`: an `assignee` of type `Bot` whose `id`
/// is the App's, as the host renders an App's bot (`decisions/DECISIONS.md`, "`app-bot`"). Not by `login`: a host
/// that renders the bot without its App's record names it `ghost[bot]` with the same id (F143).
pub fn assigns_app_bot(payload: &serde_json::Value, app: i64) -> bool {
    let a = &payload["assignee"];
    a["type"] == "Bot" && a["id"].as_i64() == Some(app)
}

/// Takes one webhook `POST`. `headers` looks a header up by its lowercase name. The signature is checked before
/// anything else is read, so an unsigned caller learns nothing about the parser. The payload is a trigger only (A3 step
/// 2): nothing but its `action` is read, and the controller re-reads the issue over REST.
pub fn intake(secret: &[u8], headers: &impl Fn(&str) -> Option<String>, body: &[u8], seen: &mut Seen) -> Intake {
    if !verify(secret, body, headers("x-hub-signature-256").as_deref()) {
        return Intake::Refused(401, "bad signature");
    }
    let nonempty = |name| headers(name).filter(|v| !v.is_empty());
    let (Some(event), Some(delivery)) = (nonempty("x-github-event"), nonempty("x-github-delivery")) else {
        return Intake::Refused(400, "missing X-GitHub-Event or X-GitHub-Delivery");
    };
    let Ok(serde_json::Value::Object(payload)) = serde_json::from_slice(body) else {
        return Intake::Refused(400, "body is not a JSON object");
    };
    if !seen.insert(&delivery) {
        return Intake::Duplicate;
    }
    let action = payload.get("action").and_then(|a| a.as_str()).map(str::to_owned);
    Intake::Accepted { event, action, delivery }
}
