//! `LogRelay { job }` hosted (`design/DESIGN.md`, Shape, "Identity"; `design/SHARED.md`, "Actions", Logs;
//! `build/BUILD-PLAN.md`, M4-WP3). The pure half is `graftyard_actions::relay`.
//!
//! - **Where.** One object per job, `logrelay:<job>`, on the site of the job's shard leader at `acquirejob`, which
//!   the job message's `FeedStreamUrl` names. A later leader move does not move it: the runner keeps the socket it
//!   opened.
//! - **Sockets.** The runner's socket and each viewer's are accepted with the hibernation API, tagged `runner` or
//!   `viewer`. A runner text message becomes `line` messages to every viewer; a viewer joining gets `hello` with the
//!   tail, or `no-stream` and a close when no runner stream is attached, and then reads the step log blobs. `/end`
//!   from `completejob` sends `end` and closes every socket.
//! - **State.** The tail lives in the isolate's memory only; the object stores nothing. A hibernated or restarted
//!   object has no tail, so a viewer that joins then gets only the lines after it (`attached` is rebuilt from the
//!   sockets it still holds).
//!
//! No request path calls the Cloudflare API.

use std::cell::RefCell;

use graftyard_actions::JobId;
use graftyard_actions::relay::Relay;
use graftyard_types::DbId;
use serde_json::json;
use worker::*;

#[durable_object]
pub struct LogRelay {
    state: State,
    env: Env,
    relay: RefCell<Option<Relay>>,
}

fn iso_now() -> String {
    graftyard_wire::time::iso(graftyard_types::Timestamp(crate::now_ms()))
}

impl LogRelay {
    fn relay<R>(&self, job: JobId, f: impl FnOnce(&mut Relay) -> R) -> R {
        let mut r = self.relay.borrow_mut();
        let r = r.get_or_insert_with(|| {
            let mut fresh = Relay::new(job, 0);
            fresh.attached = !self.state.get_websockets_with_tag("runner").is_empty();
            fresh
        });
        f(r)
    }

    fn job_of(&self, req: &Request) -> Result<JobId> {
        let n: i64 = req
            .path()
            .rsplit('/')
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::RustError("no job in path".into()))?;
        Ok(JobId(DbId(n)))
    }

    fn broadcast(&self, msg: &serde_json::Value) {
        let text = msg.to_string();
        for ws in self.state.get_websockets_with_tag("viewer") {
            let _ = ws.send_with_str(&text);
        }
    }
}

impl DurableObject for LogRelay {
    fn new(state: State, env: Env) -> Self {
        LogRelay { state, env, relay: RefCell::new(None) }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        if crate::site_down(&self.env) {
            return Response::error("site down", 503);
        }
        let path = req.path();
        // `/open/<job>?steps=n` from `acquirejob`: the job's step count, which numbers the runner's steps.
        if let Some(rest) = path.strip_prefix("/open/") {
            let job = JobId(DbId(rest.parse().map_err(|_| Error::RustError("job".into()))?));
            let steps: u32 =
                req.url()?.query_pairs().find(|(k, _)| k == "steps").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
            *self.relay.borrow_mut() = Some(Relay::new(job, steps));
            return Response::from_json(&json!({"open": job.0.0}));
        }
        // `/end/<job>` from `completejob`: `end` to every viewer, and every socket closed.
        if let Some(rest) = path.strip_prefix("/end/") {
            let job = JobId(DbId(rest.parse().map_err(|_| Error::RustError("job".into()))?));
            let v: serde_json::Value = req.json().await.unwrap_or_default();
            let c = v["conclusion"].as_str().unwrap_or("failure").to_string();
            let msg = self.relay(job, |r| r.end(&c));
            self.broadcast(&msg);
            for ws in self.state.get_websockets() {
                let _ = ws.close(Some(1000), Some("job completed"));
            }
            return Response::from_json(&json!({"ended": job.0.0}));
        }
        // `/abort`: the relay resets (`ctx.abort()`), as a lost isolate does, so every socket drops without `end`.
        // BF-61's kill; only an operator's binding sends it, since the edge forwards only the socket routes.
        if path == "/abort" {
            crate::host::abort(&self.state, "graftyard logrelay abort");
            return Response::from_json(&json!({"aborted": true}));
        }
        // `/runner/<job>` and `/viewer/<job>`: a WebSocket upgrade the edge checked the credential of.
        let role = if path.starts_with("/runner/") {
            "runner"
        } else if path.starts_with("/viewer/") {
            "viewer"
        } else {
            return Response::error("not found", 404);
        };
        if req.headers().get("Upgrade")?.as_deref() != Some("websocket") {
            return Response::error("expected a WebSocket upgrade", 426);
        }
        let job = self.job_of(&req)?;
        let pair = WebSocketPair::new()?;
        self.state.accept_websocket_with_tags(&pair.server, &[role]);
        if role == "runner" {
            self.relay(job, |r| r.attached = true);
        } else {
            let hello = self.relay(job, |r| r.hello());
            let none = hello["type"] == "no-stream";
            pair.server.send_with_str(hello.to_string())?;
            if none {
                pair.server.close(Some(1000), Some("no runner stream"))?;
            }
        }
        Response::from_websocket(pair.client)
    }

    async fn websocket_message(&self, ws: WebSocket, message: WebSocketIncomingMessage) -> Result<()> {
        if !self.state.get_tags(&ws).iter().any(|t| t == "runner") {
            return Ok(());
        }
        let WebSocketIncomingMessage::String(text) = message else { return Ok(()) };
        let job = self.relay.borrow().as_ref().map(|r| r.job);
        let Some(job) = job.or_else(|| {
            self.state.id().name().and_then(|n| n.strip_prefix("logrelay:")?.parse().ok()).map(|n| JobId(DbId(n)))
        }) else {
            return Ok(());
        };
        let at = iso_now();
        for m in self.relay(job, |r| r.frame(&text, &at)) {
            self.broadcast(&m);
        }
        Ok(())
    }

    async fn websocket_close(&self, ws: WebSocket, _code: usize, _reason: String, _clean: bool) -> Result<()> {
        let _ = ws.close(Some(1000), Some("closed"));
        Ok(())
    }

    async fn websocket_error(&self, _ws: WebSocket, _error: Error) -> Result<()> {
        Ok(())
    }
}
