// The graftyard/edge working tree: a Rust workspace for the edge Worker. `files` is main
// before the scripted commits; `after` holds the versions the branches and merges write,
// matching the core area's pull request diffs.

const PNG =
  "iVBORw0KGgoAAAANSUhEUgAAAKAAAABQCAIAAAARP+ljAAABeElEQVR42u3cuwkCQRAG4G3AwMjQwB5sxTLsQhBTY7szsgkFEznvoeCxM+sHf+56H8fsa64slitpOMUjACyABbAAFsACWAADFsACWAD/IpvDtkoAAwYMGDBgwIABAwYMGDBgwIABA/4up/OlCvDjdwE3q/t8g+Mbl+y61YGDG5fsutVrcHDjkl03wiQrsnFi4FCzaMBz6cZZJsU0Ltl1Q62DAxpPAK/3tyr5pPTGBB4yPu6uM6Ud4KHHF2ona2iQgCeA4684x4cKeBo49UQB8BhwLt3eMQMeBM6o+z5ywP3AiUrveDEG3AOcWrfzFwB3gRvQfTUG3APcTIMQ4G5a0p3b2BvsDVaD1WCzaLNo62DrYDtZgO1FO01ymvRn58HRWlecB7vREQY4xWWJFHey3Kp0qxKwe9E6G3Q26E3Sm6S7UHeh/mD9wTr8AftGB2Bf2QEMGDBgwIABAwYMGDBgwIAB/xWwAAYsgAWwABbAAlgAC2DAAlhi5w6LTMVXDpBT1wAAAABJRU5ErkJggg==";

export const edgeImages: Record<string, string> = { "docs/images/topology.png": PNG };

export const edgeFiles: Record<string, string> = {
  "README.md": `# edge

The Rust edge Worker for Graftyard. It answers the GHES 3.22 REST and GraphQL wire
protocol, so \`gh\`, Octokit, GitHub Desktop, and \`git\` work unmodified.

## Layout

| Crate | What it owns |
| --- | --- |
| \`crates/wire\` | Headers, pagination, rate limits, errors, and the outbound client |
| \`crates/api\` | The REST and GraphQL routes |
| \`crates/raft\` | Read paths: leader, lease, and learner reads |
| \`crates/edge\` | The Worker entry point |

## Build

\`\`\`sh
cargo build --workspace
cargo test --workspace
\`\`\`

The byte-diff suite replays recorded GHES 3.22 responses and fails on any byte that
differs for a client that has not opted in:

\`\`\`sh
cargo test -p edge --test byte_diff
\`\`\`

## Deploy

\`wrangler deploy\` from the repository root. The edge never calls the Cloudflare API
on a request path. \`docs/images/topology.png\` draws the request flow.

## License

Apache-2.0. See \`LICENSE\`.
`,
  "Cargo.toml": `[workspace]
resolver = "2"
members = ["crates/wire", "crates/api", "crates/raft", "crates/edge"]

[workspace.package]
version = "0.3.0"
edition = "2021"
license = "Apache-2.0"
repository = "https://ghes.graftyard.test/graftyard/edge"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
worker = "0.4"
thiserror = "1"

[profile.release]
lto = true
opt-level = "z"
codegen-units = 1
`,
  "CHANGELOG.md": `# Changelog

## 0.1.0

- REST: repositories, issues, and comments answer as GHES 3.22 does.
- Pagination follows GHES: \`per_page\` defaults to 30 and caps at 100.

## 0.2.0

- REST: \`If-None-Match\` on repository GET answers 304.
- Errors use GHES's basic-error body.

## 0.3.0

- Learner groups are visible to the router, but serve no reads yet.
- The byte-diff suite runs in CI on every pull request.
`,
  "tools/record.sh": `#!/usr/bin/env bash
# Records GHES 3.22 responses for the byte-diff suite from a reference appliance.
set -euo pipefail

HOST="\${GHES_HOST:?set GHES_HOST to the reference appliance}"
OUT="\${1:-assets/fixtures/recordings}"

mkdir -p "$OUT"
while read -r method path; do
  name="$(echo "$method $path" | tr ' /{}' '____')"
  curl -sS -X "$method" -H "Authorization: Bearer $GHES_TOKEN" \\
    -D "$OUT/$name.headers" -o "$OUT/$name.body" "https://$HOST/api/v3$path"
done < tools/routes.txt

tar --zstd -cf assets/fixtures/ghes-3.22-recordings.tar.zst -C "$OUT" .
echo "recorded $(ls "$OUT" | wc -l) files"
`,
  "rustfmt.toml": `edition = "2021"
max_width = 100
use_small_heuristics = "Max"
`,
  ".gitignore": `/target
/.wrangler
*.log
`,
  ".gitattributes": `assets/fixtures/*.tar.zst filter=lfs diff=lfs merge=lfs -text
*.png binary
`,
  LICENSE: `                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   Copyright 2025 The Graftyard authors

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.
`,
  "wrangler.toml": `name = "graftyard-edge"
main = "dist/worker/shim.mjs"
compatibility_date = "2026-10-01"

[build]
command = "cargo install -q worker-build && worker-build --release"

[[durable_objects.bindings]]
name = "GROUP"
class_name = "Group"

[observability]
enabled = true
`,
  "assets/fixtures/ghes-3.22-recordings.tar.zst": `version https://git-lfs.github.com/spec/v1
oid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393
size 48213907
`,
  "docs/architecture.md": `# Architecture

A request enters the edge Worker, which parses it against the GHES 3.22 wire contract
and routes it to the group that owns the resource.

## Reads

Reads go to the leader when it is warm. A lease read answers without a round trip
while the lease holds. A learner read serves hot lists when the leader is cold.

## Writes

A write is acknowledged only after the quorum rule holds. The response renderer takes
the acknowledgement, so no 2xx can leave before it.

## Errors

Every error response carries the rate limit headers, because Octokit's throttling
plugin reads them from 403 and 429.

## Retries

The outbound client retries only idempotent reads, at most twice, with full jitter.
`,
  ".github/workflows/ci.yml": `name: ci
on:
  push:
    branches: [main]
  pull_request:

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --workspace

  rustfmt:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo fmt --all -- --check

  clippy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo clippy --workspace -- -D warnings

  byte-diff:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          lfs: true
      - run: cargo test -p edge --test byte_diff
`,
  "crates/wire/Cargo.toml": `[package]
name = "wire"
version.workspace = true
edition.workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
`,
  "crates/wire/src/lib.rs": `//! The GHES 3.22 wire contract: headers, pagination, errors, and retries.

pub mod error;
pub mod headers;
pub mod link;
pub mod pagination;
pub mod retry;

use std::time::Duration;

pub use error::ErrorResponse;
pub use pagination::Page;

/// The outbound client every crate uses to reach a group or the blob store.
pub fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(10))
        .retry(retry::should_retry)
        .build()
}
`,
  "crates/wire/src/retry.rs": `use std::time::Duration;

pub const MAX_RETRIES: u32 = 5;

pub fn should_retry(status: u16) -> bool {
    status >= 500
}

pub fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(100 * 2u64.pow(attempt))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_server_errors() {
        assert!(should_retry(503));
    }
}
`,
  "crates/wire/src/error.rs": `use serde::Serialize;

use crate::headers::RateLimit;

const DOCS: &str = "https://docs.github.com/enterprise-server@3.22/rest";

/// GHES's basic-error body: message, documentation_url, and status.
#[derive(Debug, Serialize)]
pub struct Body {
    pub message: String,
    pub documentation_url: &'static str,
    pub status: String,
}

#[derive(Debug)]
pub struct ErrorResponse {
    pub status: u16,
    pub body: Body,
}

impl ErrorResponse {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        let body = Body { message: message.into(), documentation_url: DOCS, status: status.to_string() };
        Self { status, body }
    }

    pub fn not_found() -> Self {
        Self::new(404, "Not Found")
    }

    pub fn unavailable() -> Self {
        Self::new(503, "Service Unavailable")
    }

    pub fn render(&self, limits: &RateLimit) -> Response {
        let mut res = Response::json(&self.body).with_status(self.status);
        if self.status < 400 {
            limits.apply(res.headers_mut());
        }
        res
    }
}
`,
  "crates/wire/src/headers.rs": `/// The x-ratelimit-* headers GHES sends on every response.
#[derive(Clone, Copy, Debug)]
pub struct RateLimit {
    pub limit: u32,
    pub remaining: u32,
    pub used: u32,
    pub reset: u64,
}

impl RateLimit {
    pub fn apply(&self, headers: &mut Headers) {
        headers.set("x-ratelimit-limit", &self.limit.to_string());
        headers.set("x-ratelimit-remaining", &self.remaining.to_string());
        headers.set("x-ratelimit-used", &self.used.to_string());
        headers.set("x-ratelimit-reset", &self.reset.to_string());
        headers.set("x-ratelimit-resource", "core");
    }
}

/// GHES echoes the API version it served.
pub const API_VERSION: &str = "2022-11-28";

pub fn apply_common(headers: &mut Headers) {
    headers.set("x-github-api-version-selected", API_VERSION);
    headers.set("x-github-media-type", "github.v3; format=json");
}
`,
  "crates/wire/src/link.rs": `use crate::pagination::Page;

/// The Link header for one page of a list. GHES sends none when there is one page.
pub fn link_header(base: &str, page: Page, last: u32) -> Option<String> {
    if last <= 1 {
        return None;
    }
    let mut rels = Vec::new();
    if page.number > 1 {
        rels.push(format!("<{}>; rel=\\"prev\\"", url(base, page.number - 1, page.per_page)));
        rels.push(format!("<{}>; rel=\\"first\\"", url(base, 1, page.per_page)));
    }
    if page.number < last {
        rels.push(format!("<{}>; rel=\\"next\\"", url(base, page.number + 1, page.per_page)));
        rels.push(format!("<{}>; rel=\\"last\\"", url(base, last, page.per_page)));
    }
    Some(rels.join(", "))
}

fn url(base: &str, page: u32, per_page: u32) -> String {
    format!("{base}?per_page={per_page}&page={page}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_page_has_no_link() {
        assert_eq!(link_header("/x", Page { number: 1, per_page: 30 }, 1), None);
    }
}
`,
  "crates/wire/src/pagination.rs": `/// One page of a list: GHES's per_page (default 30, at most 100) and a 1-based page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    pub number: u32,
    pub per_page: u32,
}

impl Page {
    pub fn parse(per_page: Option<&str>, page: Option<&str>) -> Self {
        let per_page = per_page.and_then(|s| s.parse().ok()).unwrap_or(30).clamp(1, 100);
        let number = page.and_then(|s| s.parse().ok()).unwrap_or(1).max(1);
        Self { number, per_page }
    }

    pub fn offset(&self) -> usize {
        ((self.number - 1) * self.per_page) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_per_page_is_the_default() {
        assert_eq!(Page::parse(Some(""), None).per_page, 30);
    }
}
`,
  "crates/api/Cargo.toml": `[package]
name = "api"
version.workspace = true
edition.workspace = true

[dependencies]
wire = { path = "../wire" }
raft = { path = "../raft" }
serde.workspace = true
serde_json.workspace = true
`,
  "crates/api/src/lib.rs": `//! REST and GraphQL routes, each answering exactly as GHES 3.22 does.

pub mod graphql;
pub mod rest;

pub use rest::router;
`,
  "crates/api/src/graphql.rs": `/// Persisted queries the web app sends. Anything else answers 400, as GHES does
/// for an unknown persisted query id.
pub fn persisted(id: &str) -> Option<&'static str> {
    match id {
        "repo-overview" => Some(include_str!("queries/repo_overview.graphql")),
        "pull-files" => Some(include_str!("queries/pull_files.graphql")),
        _ => None,
    }
}
`,
  "crates/api/src/rest/mod.rs": `pub mod issues;
pub mod repos;

use wire::ErrorResponse;

pub fn router() -> Router {
    Router::new()
        .get("/repos/:owner/:repo", repos::get)
        .get("/repos/:owner/:repo/issues", issues::list)
        .post("/repos/:owner/:repo/issues", issues::create)
        .get("/repos/:owner/:repo/issues/:number", issues::get)
        .fallback(|_| ErrorResponse::not_found())
}
`,
  "crates/api/src/rest/issues.rs": `use serde_json::Value;
use wire::{ErrorResponse, Page};

pub struct CreateIssue {
    pub title: String,
    pub body: Option<String>,
    pub labels: Option<Vec<String>>,
}

pub struct Invalid(pub &'static str);

pub async fn list(req: Request, ctx: Ctx) -> Result<Response, ErrorResponse> {
    let page = Page::parse(req.query("per_page"), req.query("page"));
    let issues = ctx.group(&req).issues(page).await?;
    Ok(Response::json(&issues))
}

pub async fn create(mut req: Request, ctx: Ctx) -> Result<Response, ErrorResponse> {
    let body: Value = req.json().await?;
    let input = parse_create(&body).map_err(|e| ErrorResponse::new(422, e.0))?;
    let acked = ctx.group(&req).create_issue(input).await?;
    Ok(Response::json(acked.value()).with_status(201))
}

pub async fn get(req: Request, ctx: Ctx) -> Result<Response, ErrorResponse> {
    let number: u64 = req.param("number").parse().map_err(|_| ErrorResponse::not_found())?;
    ctx.group(&req).issue(number).await?.map(|i| Response::json(&i)).ok_or_else(ErrorResponse::not_found)
}

fn parse_create(body: &Value) -> Result<CreateIssue, Invalid> {
    let title = body["title"].as_str().ok_or(Invalid("title"))?.to_owned();
    let text = body["body"].as_str().map(str::to_owned);
    let labels = body["labels"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect());
    Ok(CreateIssue { title, body: text, labels })
}
`,
  "crates/api/src/rest/repos.rs": `use wire::ErrorResponse;

pub async fn get(req: Request, ctx: Ctx) -> Result<Response, ErrorResponse> {
    let full_name = format!("{}/{}", req.param("owner"), req.param("repo"));
    let repo = ctx.repos().get(&full_name).await?.ok_or_else(ErrorResponse::not_found)?;
    if let Some(etag) = req.header("if-none-match") {
        if etag == repo.etag() {
            return Ok(Response::empty().with_status(304));
        }
    }
    Ok(Response::json(&repo).with_header("etag", &repo.etag()))
}
`,
  "crates/raft/Cargo.toml": `[package]
name = "raft"
version.workspace = true
edition.workspace = true

[dependencies]
serde.workspace = true
`,
  "crates/raft/src/lib.rs": `//! The read side of a group: which replica may answer a read.

pub mod read;

pub use read::ReadPath;
`,
  "crates/raft/src/read.rs": `use std::time::{Duration, Instant};

/// How long a leader may answer reads without a round trip after its lease starts.
pub const LEASE: Duration = Duration::from_secs(9);

/// The replica that answers a read, and why it may.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadPath {
    Leader,
    Lease,
}

impl ReadPath {
    pub fn for_lease(started: Instant, now: Instant) -> Self {
        if now.duration_since(started) < LEASE {
            ReadPath::Lease
        } else {
            ReadPath::Leader
        }
    }
}
`,
  "crates/edge/Cargo.toml": `[package]
name = "edge"
version.workspace = true
edition.workspace = true

[lib]
crate-type = ["cdylib"]

[dependencies]
api = { path = "../api" }
wire = { path = "../wire" }
worker.workspace = true
`,
  "crates/edge/src/lib.rs": `use worker::*;

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let router = api::router();
    let mut res = router.run(req, env).await?;
    wire::headers::apply_common(res.headers_mut());
    Ok(res)
}
`,
  "crates/edge/tests/byte_diff.rs": `//! Replays every recorded GHES 3.22 response and fails on the first byte that differs.

use std::path::Path;

#[test]
fn every_recording_matches() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fixtures");
    let recordings = fixtures::unpack(dir.join("ghes-3.22-recordings.tar.zst"));
    let mut failures = Vec::new();
    for rec in &recordings {
        let ours = replay(&rec.request);
        if ours.bytes() != rec.response.bytes() {
            failures.push(rec.name.clone());
        }
    }
    assert!(failures.is_empty(), "{} of {} differ: {:?}", failures.len(), recordings.len(), failures);
}
`,
};

// The versions branches and merges write.
export const edgeAfter = {
  retry: `use std::time::Duration;

pub const MAX_RETRIES: u32 = 2;
const BASE_MS: u64 = 300;
const CAP_MS: u64 = 4_000;

/// Only reads are safe to send twice.
pub fn should_retry(method: &Method, status: u16, attempt: u32) -> bool {
    method.is_idempotent_read() && status >= 500 && attempt < MAX_RETRIES
}

/// Full jitter: a uniform delay below an exponential cap.
pub fn backoff(attempt: u32, rand: f64) -> Duration {
    let cap = (BASE_MS << attempt).min(CAP_MS);
    Duration::from_millis((rand * cap as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_retries_a_post() {
        assert!(!should_retry(&Method::Post, 503, 0));
        assert!(should_retry(&Method::Get, 503, 0));
        assert!(!should_retry(&Method::Get, 503, 2));
    }
}
`,
  method: `#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Post,
    Patch,
    Delete,
}

impl Method {
    pub fn is_idempotent_read(&self) -> bool {
        matches!(self, Method::Get | Method::Head)
    }
}
`,
  libWithMethod: (src: string) =>
    src.replace("pub mod link;\n", "pub mod link;\npub mod method;\n"),
  libRetryClosure: (src: string) =>
    src.replace(
      ".retry(retry::should_retry)",
      ".retry(|req, res, n| retry::should_retry(req.method(), res.status(), n))",
    ),
  error: (src: string) =>
    src.replace(
      `        if self.status < 400 {
            limits.apply(res.headers_mut());
        }
`,
      `        // GHES sends the rate limit headers on errors too, and
        // Octokit's throttling plugin reads them from 403 and 429.
        limits.apply(res.headers_mut());
`,
    ),
  read: (src: string) =>
    src.replace(
      "    Lease,\n}\n",
      `    Lease,
    /// A learner that has applied at least the leader's commit index.
    Learner { min_index: u64 },
}

pub fn pick(leader_warm: bool, learner_index: u64, commit: u64) -> ReadPath {
    if !leader_warm && learner_index >= commit {
        return ReadPath::Learner { min_index: commit };
    }
    ReadPath::Lease
}
`,
    ),
  changelog: (src: string) =>
    src.replace(
      "# Changelog\n",
      "# Changelog\n\n## Unreleased\n\n- Errors carry the rate limit headers, so Octokit's throttling plugin can back off.\n",
    ),
  issues: (src: string) =>
    src.replace(
      `    let labels = body["labels"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect());`,
      `    let labels = body["labels"].as_array().map(|a| {
        a.iter()
            .filter_map(|l| l.as_str().or_else(|| l["name"].as_str()))
            .collect()
    });`,
    ),
  version: (src: string) => src.replace('version = "0.3.0"', 'version = "0.4.0-rc.1"'),
};
