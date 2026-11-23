#!/usr/bin/env python3
"""AM2-WP2a, `graft-mcp`'s MCP endpoint and tools: planted bugs in the two protocol lanes (`workers/mcp/src/rpc.rs`)
and the tools' rules (`workers/mcp/src/tools.rs`): the trust view on every text field, the grant's repositories, the
write scope, `ToolError`'s fields, the scoped token, and the lease and feed calls. Plants one mutant at a time, runs
the tests that guard it (`workers/mcp/tests/wp2a.rs`), and restores the file byte for byte. Each line prints CAUGHT,
MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant90.py          # every mutant
    python3 tests/m0carried/plant90.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
RPC = 'workers/mcp/src/rpc.rs'
TOOLS = 'workers/mcp/src/tools.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-mcp', '--test', 'wp2a']

MUTANTS = [
    # The trust view.
    ('a viewed body passes through raw', TOOLS,
     '        if m.contains_key("body") {\n', '        if false {\n', T),
    ('body_html, which renders the hidden text, reaches the model', TOOLS,
     '        m.remove("body_html");\n', '', T),
    ("a check run's raw summary reaches the model", TOOLS,
     '            o.remove("summary");\n', '', T),
    ('text the edge did not view passes through', TOOLS,
     '            m.insert("body".into(), visible.clone().map_or(Value::Null, Value::String));\n',
     '            m.insert("body".into(), visible.clone().map_or_else(|| m["body"].clone(), Value::String));\n', T),
    ('reads do not ask for the trust view', TOOLS,
     '        Ok(self.send("GET", &with_query(path, q), self.headers(AGENT_MEDIA_TYPE), None).await?.1)\n',
     '        Ok(self.send("GET", &with_query(path, q), self.headers("application/json"), None).await?.1)\n', T),
    # The grant's repositories, toolsets, and write scope.
    ('a repository outside the grant is called', TOOLS,
     '        if !self.tok.repos.iter().any(|g| g.eq_ignore_ascii_case(&full)) {\n', '        if false {\n', T),
    ('a repository name may add path segments', TOOLS,
     '    url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>().replace(\'+\', "%20")\n',
     '    s.to_owned()\n', T),
    ('a read token sees write tools', TOOLS,
     '    tok.tools.has(t.set) && t.needs <= tok.write\n', '    tok.tools.has(t.set)\n', T),
    ('a token sees toolsets it was not granted', TOOLS,
     '    tok.tools.has(t.set) && t.needs <= tok.write\n', '    t.needs <= tok.write\n', T),
    ('a hidden tool runs when called by name', RPC,
     '    if !tools::TOOLS.iter().any(|t| t.name == name && tools::visible_to(t, tok)) {\n',
     '    if !tools::TOOLS.iter().any(|t| t.name == name) {\n', T),
    ('a pull request is opened ready for review by default', TOOLS,
     '"draft": args.get("draft").and_then(Value::as_bool).unwrap_or(true)',
     '"draft": args.get("draft").and_then(Value::as_bool).unwrap_or(false)', T),
    # ToolError, leases, and the feed.
    ('ToolError drops Retry-After', TOOLS,
     '        let retry_after_s = n("retry-after").and_then(|s| u32::try_from(s).ok());\n',
     '        let retry_after_s = None;\n', T),
    ('ToolError drops the rate fields', TOOLS,
     '        let rate = n("x-ratelimit-limit").map(|limit| RateInfo {\n',
     '        let rate = None::<u64>.map(|limit| RateInfo {\n', T),
    ("a lease's session is not named to the edge", TOOLS,
     '            h.push(("x-graftyard-agent-session", s.to_owned()));\n', '            let _ = s;\n', T),
    ("the feed's wait is not capped", TOOLS,
     '            q.push(("wait", n("wait").unwrap_or(0).min(FEED_WAIT_MAX_S).to_string()));\n',
     '            q.push(("wait", n("wait").unwrap_or(0).to_string()));\n', T),
    # The 2026-07-28 lane.
    ('the header version may disagree with _meta', RPC,
     '    if hv != Some(v) {\n', '    if false {\n', T),
    ('Mcp-Method may disagree with the method', RPC,
     '    if header("mcp-method").as_deref() != Some(method) {\n', '    if false {\n', T),
    ('an unsupported version gets no data.supported to retry with', RPC,
     '        let data = json!({ "supported": [V2026], "requested": v });\n',
     '        let data = json!({ "requested": v });\n', T),
    ("tools/list lacks the cache fields TypeScript v2 and Python require", RPC,
     '        r["ttlMs"] = json!(0);\n', '', T),
    ('discover advertises listChanged, which opens an SSE subscription', RPC,
     '                "capabilities": { "tools": { "listChanged": false } },\n',
     '                "capabilities": { "tools": { "listChanged": true } },\n', T),
    # The 2025-11-25 lane and HTTP.
    ('a notification gets a JSON body instead of 202', RPC,
     '        return Out { status: 202, headers: Vec::new(), body: None };\n',
     '        return out(200, json!({}));\n', T),
    ('GET is served instead of 405', RPC,
     '    let mut o = error(405, &Value::Null, -32000, "Method not allowed.", None);\n',
     '    let mut o = error(200, &Value::Null, -32000, "Method not allowed.", None);\n', T),
    ('initialize does not echo a supported legacy version', RPC,
     '            let v = if LEGACY.contains(&asked) { asked } else { V2025 };\n', '            let v = V2025;\n', T),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, cmd = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                verdict = 'MISSED'
            elif 'error[E' in out or 'could not compile' in out:
                verdict = 'DID NOT COMPILE'
            else:
                verdict = 'CAUGHT'
                caught += 1
        finally:
            open(p, 'wb').write(orig)
        print(f'{i} {name}: {verdict}', flush=True)
    print(f'{caught} of {len(pick)} caught')


if __name__ == '__main__':
    main()
