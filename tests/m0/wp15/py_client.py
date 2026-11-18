"""M0-WP15, L0-08: the official Python SDK client (mcp 2.3.0) against graft-mcp, unmodified, at 2026-07-28.

Run with the out-of-repo venv:  ~/.cache/wp15-venv/bin/python wp15/py_client.py [calls]
The SDK's OAuthClientProvider does discovery, CIMD (`client_metadata_url`), PKCE, RFC 8707, and the RFC 9207 iss check.
The redirect handler hands the authorization URL to run/wp15-browser.ts, the scripted browser. Prints one JSON line.
"""

import os
import asyncio
import json
import subprocess
import sys
import time
from urllib.parse import parse_qs, urlparse

import httpx2
from mcp import Client
from mcp.client.auth import OAuthClientProvider
from mcp.client.streamable_http import streamable_http_client
from mcp.shared.auth import AuthorizationCodeResult, OAuthClientMetadata

ROOT = "/home/user/workspace/repo/tests/m0"
MCP = f"https://graft-wp15-mcp.{os.environ['CF_WORKERS_SUBDOMAIN']}.workers.dev"
CIMD = f"https://graft-wp15-aux.{os.environ['CF_WORKERS_SUBDOMAIN']}.workers.dev/cimd/py-v2.json"
REDIRECT = "http://127.0.0.1:4711/callback"
CALLS = int(sys.argv[1]) if len(sys.argv) > 1 else 30


class Mem:
    def __init__(self):
        self.t = None
        self.c = None

    async def get_tokens(self):
        return self.t

    async def set_tokens(self, t):
        self.t = t

    async def get_client_info(self):
        return self.c

    async def set_client_info(self, c):
        self.c = c


callback: dict = {}


async def redirect_handler(url: str) -> None:
    out = subprocess.run(["node", "run/wp15-browser.ts", url], cwd=ROOT, capture_output=True, text=True, timeout=60)
    if out.returncode != 0:
        raise RuntimeError(out.stderr[-500:])
    callback["url"] = out.stdout.strip()


async def callback_handler() -> AuthorizationCodeResult:
    q = parse_qs(urlparse(callback["url"]).query)
    return AuthorizationCodeResult(code=q["code"][0], state=q.get("state", [None])[0], iss=q.get("iss", [None])[0])


async def main() -> None:
    store = Mem()
    auth = OAuthClientProvider(
        server_url=MCP + "/",
        client_metadata=OAuthClientMetadata(redirect_uris=[REDIRECT], client_name="py-v2", grant_types=["authorization_code", "refresh_token"],
                                            response_types=["code"], token_endpoint_auth_method="none", scope="write:write"),
        storage=store, redirect_handler=redirect_handler, callback_handler=callback_handler, client_metadata_url=CIMD)
    out: dict = {"client": "py-v2", "sdk": "mcp 2.3.0 (Python)"}
    t0 = time.time()
    async with httpx2.AsyncClient(auth=auth, timeout=60) as http:
        async with Client(streamable_http_client(MCP + "/", http_client=http), mode="2026-07-28") as c:
            out["protocol"] = c.protocol_version
            tools = await c.list_tools()
            out["tools"] = [t.name for t in tools.tools]
            out["authMs"] = int((time.time() - t0) * 1000)
            ok, lat, errs = 0, [], []
            for i in range(CALLS):
                s = time.time()
                if i % 3 == 0:
                    r = await c.call_tool("get_me", {})
                elif i % 3 == 1:
                    r = await c.call_tool("get_ref", {"owner": "acme", "repo": "mono", "ref": "heads/main"})
                else:
                    r = await c.call_tool("get_issue", {"owner": "acme", "repo": "mono", "number": 1})
                lat.append(int((time.time() - s) * 1000))
                if not r.is_error:
                    ok += 1
                else:
                    errs.append(str(r.content)[:200])
            lat.sort()
            out.update(calls=CALLS, ok=ok, clientP50=lat[len(lat) // 2], errors=errs[:3],
                       iss=parse_qs(urlparse(callback.get("url", "")).query).get("iss", [None])[0],
                       tokenScope=store.t.scope if store.t else None, refreshToken=bool(store.t and store.t.refresh_token))
    print(json.dumps(out))


asyncio.run(main())
