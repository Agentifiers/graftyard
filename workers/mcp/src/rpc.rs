//! The MCP endpoint's JSON-RPC over Streamable HTTP, stateless and JSON-only (`design/AGENT-LAYER.md`, journey A7,
//! step 7; `build/BUILD-PLAN.md`, AM2-WP2, WP2a). Two lanes, as the SDK clients L0-08 runs speak them:
//!
//! - **2026-07-28**: every request carries `params._meta["io.modelcontextprotocol/protocolVersion"]`, its client
//!   capabilities, and the `MCP-Protocol-Version` and `Mcp-Method` headers (`Mcp-Name` on `tools/call`), which must
//!   agree with the body. A client probes with `server/discover` (TypeScript 2.4.0 and Go v1.8.0) or goes straight to
//!   `tools/list` (Python 2.3.0 pinned), so no request depends on an earlier one. Results carry `resultType`, and
//!   `tools/list` and `server/discover` carry `ttlMs` and `cacheScope`.
//! - **2025-11-25**: `initialize`, then `notifications/initialized` (202), then requests with no envelope
//!   (TypeScript 1.32.2, and Go's and Python's fallbacks). No `Mcp-Session-Id` is issued, so no client sends DELETE,
//!   and GET (the SSE stream TypeScript v1 and Go open) answers 405.
//!
//! Each answer is one JSON body: `server/discover`, `initialize`, `tools/list`, `tools/call`, and `ping`; 202 for a
//! notification; 405 for GET and DELETE. Status codes and error codes follow the TypeScript SDK's server
//! (`@modelcontextprotocol/server`, `classifyInboundRequest` and `httpStatusForErrorCode`), which all four clients
//! passed against in M0-WP15 [measured, `tests/m0/results/m0-wp15.md`; the wire shape read from the pinned SDKs'
//! sources and run against all four clients locally, `tests/am2/results/am2-wp2a.md`].

use serde_json::{Value, json};

use crate::McpToken;
use crate::tools::{self, Edge};

pub const V2026: &str = "2026-07-28";
pub const V2025: &str = "2025-11-25";
/// The versions an `initialize` may name and get echoed.
const LEGACY: [&str; 5] = [V2025, "2025-06-18", "2025-03-26", "2024-11-05", "2024-10-07"];
const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CAPS: &str = "io.modelcontextprotocol/clientCapabilities";
pub const SERVER_NAME: &str = "graft-mcp";

/// One HTTP answer: status, extra headers, and body (`None` for an empty one).
#[derive(Debug, PartialEq)]
pub struct Out {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Option<Value>,
}

fn out(status: u16, body: Value) -> Out {
    Out { status, headers: Vec::new(), body: Some(body) }
}

fn error(status: u16, id: &Value, code: i64, message: &str, data: Option<Value>) -> Out {
    let mut e = json!({ "code": code, "message": message });
    if let Some(d) = data {
        e["data"] = d;
    }
    out(status, json!({ "jsonrpc": "2.0", "id": id, "error": e }))
}

fn result(id: &Value, r: Value) -> Out {
    out(200, json!({ "jsonrpc": "2.0", "id": id, "result": r }))
}

/// GET and DELETE: no stream and no session, so nothing to open or close.
pub fn not_post() -> Out {
    let mut o = error(405, &Value::Null, -32000, "Method not allowed.", None);
    o.headers.push(("allow", "POST".into()));
    o
}

/// One POST to the endpoint, already past the token door. `header` reads the request's lower-case headers.
pub async fn post<E: Edge>(
    tok: &McpToken,
    content_type: Option<&str>,
    header: impl Fn(&str) -> Option<String>,
    body: &[u8],
    edge: &E,
) -> Out {
    if !content_type
        .is_some_and(|c| c.split(';').next().is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json")))
    {
        let msg = "Unsupported Media Type: Content-Type must be application/json";
        return error(415, &Value::Null, -32000, msg, None);
    }
    let Ok(msg) = serde_json::from_slice::<Value>(body) else {
        return error(400, &Value::Null, -32700, "Parse error", None);
    };
    if msg.is_array() {
        return error(400, &Value::Null, -32600, "Batches are not served", None);
    }
    let id = msg.get("id").cloned();
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return error(400, id.as_ref().unwrap_or(&Value::Null), -32600, "Invalid Request", None);
    };
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let meta = params.get("_meta");
    let claim = meta.and_then(|m| m.get(META_VERSION));
    let hv = header("mcp-protocol-version");
    let Some(id) = id else {
        // A notification, on either lane: nothing to answer.
        return Out { status: 202, headers: Vec::new(), body: None };
    };
    if let Some(claim) = claim {
        modern(tok, &id, method, &params, claim, meta, hv.as_deref(), &header, edge).await
    } else if hv.as_deref().is_some_and(|v| v >= V2026) {
        error(400, &id, -32602, "A 2026-07-28 request carries its protocol version in _meta", None)
    } else {
        legacy(tok, &id, method, &params, hv.as_deref(), edge).await
    }
}

#[allow(clippy::too_many_arguments)]
async fn modern<E: Edge>(
    tok: &McpToken,
    id: &Value,
    method: &str,
    params: &Value,
    claim: &Value,
    meta: Option<&Value>,
    hv: Option<&str>,
    header: &impl Fn(&str) -> Option<String>,
    edge: &E,
) -> Out {
    let Some(v) = claim.as_str() else {
        return error(400, id, -32602, "Invalid _meta envelope", Some(json!({ "envelope": META_VERSION })));
    };
    if meta.and_then(|m| m.get(META_CAPS)).is_none_or(|c| !c.is_object()) {
        return error(400, id, -32602, "Invalid _meta envelope", Some(json!({ "envelope": META_CAPS })));
    }
    if v != V2026 {
        let data = json!({ "supported": [V2026], "requested": v });
        return error(400, id, -32022, &format!("Unsupported protocol version: {v}"), Some(data));
    }
    if hv != Some(v) {
        return error(400, id, -32020, "MCP-Protocol-Version does not match the request's _meta", None);
    }
    if header("mcp-method").as_deref() != Some(method) {
        return error(400, id, -32020, "Mcp-Method does not match the request's method", None);
    }
    if method == "tools/call" && header("mcp-name").as_deref() != params.get("name").and_then(Value::as_str) {
        return error(400, id, -32020, "Mcp-Name does not match the tool called", None);
    }
    let cached = |mut r: Value| {
        r["resultType"] = json!("complete");
        r["ttlMs"] = json!(0);
        r["cacheScope"] = json!("private");
        r
    };
    match method {
        "server/discover" => result(
            id,
            cached(json!({
                "supportedVersions": [V2026],
                "capabilities": { "tools": { "listChanged": false } },
                "_meta": { "io.modelcontextprotocol/serverInfo": server_info() },
            })),
        ),
        "tools/list" => result(id, cached(json!({ "tools": tools::list(tok) }))),
        "tools/call" => match call(tok, params, edge).await {
            Ok(mut r) => {
                r["resultType"] = json!("complete");
                result(id, r)
            }
            Err(o) => o(id),
        },
        _ => error(404, id, -32601, &format!("Method not found: {method}"), None),
    }
}

async fn legacy<E: Edge>(tok: &McpToken, id: &Value, method: &str, params: &Value, hv: Option<&str>, edge: &E) -> Out {
    if let Some(v) = hv
        && method != "initialize"
        && !LEGACY.contains(&v)
    {
        return error(400, id, -32000, &format!("Unsupported protocol version: {v}"), None);
    }
    match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(V2025);
            let v = if LEGACY.contains(&asked) { asked } else { V2025 };
            result(
                id,
                json!({ "protocolVersion": v, "capabilities": { "tools": { "listChanged": false } }, "serverInfo": server_info() }),
            )
        }
        "ping" => result(id, json!({})),
        "tools/list" => result(id, json!({ "tools": tools::list(tok) })),
        "tools/call" => match call(tok, params, edge).await {
            Ok(r) => result(id, r),
            Err(o) => o(id),
        },
        _ => error(200, id, -32601, &format!("Method not found: {method}"), None),
    }
}

/// A `tools/call`'s result: the tool's JSON as one text block, or its `ToolError` with `isError`. An unknown tool is
/// the JSON-RPC error -32602, as the TypeScript SDK's server answers it.
async fn call<E: Edge>(tok: &McpToken, params: &Value, edge: &E) -> Result<Value, Box<dyn FnOnce(&Value) -> Out>> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
    if !tools::TOOLS.iter().any(|t| t.name == name && tools::visible_to(t, tok)) {
        return Err(Box::new(move |id| error(200, id, -32602, &format!("Tool {name} not found"), None)));
    }
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    Ok(match tools::call(&name, &args, tok, edge).await {
        Ok(v) => json!({ "content": [{ "type": "text", "text": v.to_string() }] }),
        Err(e) => json!({ "content": [{ "type": "text", "text": json!(e).to_string() }], "isError": true }),
    })
}

fn server_info() -> Value {
    json!({ "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") })
}
