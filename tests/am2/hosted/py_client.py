"""L0-08 hosted and A43 with the official Python SDK client (mcp 2.3.0), unmodified, at 2026-07-28, against graft-mcp.

Run with an out-of-repo venv:  ~/.cache/am2-venv/bin/python tests/am2/hosted/py_client.py [calls] [--drop-token]
The SDK's OAuthClientProvider does discovery, CIMD (`client_metadata_url`), PKCE, RFC 8707, and the RFC 9207 iss check.
The redirect handler hands the authorization URL to tests/am2/hosted/browser.mjs, the scripted browser. `--drop-token`
(A43): the first code redemption's answer is dropped by the HTTP transport after graft-mcp answered it. Prints one JSON
line.
"""

import asyncio
import json
import os
import subprocess
import sys
import time
from urllib.parse import parse_qs, urlparse

import httpx2
from mcp import Client
from mcp.client.auth import OAuthClientProvider
from mcp.client.streamable_http import streamable_http_client
from mcp.shared.auth import AuthorizationCodeResult, OAuthClientMetadata

HERE = os.path.dirname(os.path.abspath(__file__))
SUB = os.environ["CF_WORKERS_SUBDOMAIN"]
MCP = os.environ.get("GRAFT_MCP", f"https://graft-m3f-mcp.{SUB}.workers.dev")
AUX = os.environ.get("GRAFT_AUX", f"https://graft-m3f-mcp-aux.{SUB}.workers.dev")
CIMD = f"{AUX}/cimd/py-v2.json"
REDIRECT = "http://127.0.0.1:4711/callback"
ARGS = [a for a in sys.argv[1:] if not a.startswith("--")]
CALLS = int(ARGS[0]) if ARGS else 30
REPO = os.environ.get("AM2_REPO", "m3org/j4")
DROP = "--drop-token" in sys.argv


def member() -> str:
    """The member's PAT from $M3_STATE/m3-tokens.txt, as tests/am1/hosted/lib.mjs reads it."""
    with open(os.path.join(os.environ["M3_STATE"], "m3-tokens.txt")) as f:
        return dict(line.split(" ", 1) for line in f.read().strip().split("\n"))["member"].strip()


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


class Dropper(httpx2.AsyncHTTPTransport):
    """Records every /token call; once, drops a code redemption's answer after graft-mcp answered it."""

    def __init__(self):
        super().__init__()
        self.calls = []
        self.dropped = False

    async def handle_async_request(self, request):
        grant = None
        if str(request.url) == f"{MCP}/token":
            grant = parse_qs(request.content.decode()).get("grant_type", [None])[0]
        r = await super().handle_async_request(request)
        if str(request.url) == f"{MCP}/token":
            self.calls.append({"grant": grant, "status": r.status_code})
            if DROP and not self.dropped and grant == "authorization_code":
                self.dropped = True
                await r.aread()
                raise httpx2.ReadError("A43: the /token answer dropped after graft-mcp answered", request=request)
        return r


callback: dict = {}
consents = 0


async def redirect_handler(url: str) -> None:
    global consents
    consents += 1
    out = subprocess.run(["node", os.path.join(HERE, "browser.mjs"), url], capture_output=True, text=True, timeout=90)
    if out.returncode != 0:
        raise RuntimeError(out.stderr[-500:])
    callback["url"] = out.stdout.strip()


async def callback_handler() -> AuthorizationCodeResult:
    q = parse_qs(urlparse(callback["url"]).query)
    return AuthorizationCodeResult(code=q["code"][0], state=q.get("state", [None])[0], iss=q.get("iss", [None])[0])


async def session(out: dict, store: Mem, transport: Dropper) -> None:
    auth = OAuthClientProvider(
        server_url=MCP + "/",
        client_metadata=OAuthClientMetadata(redirect_uris=[REDIRECT], client_name="py-v2", grant_types=["authorization_code", "refresh_token"],
                                            response_types=["code"], token_endpoint_auth_method="none", scope="write:write"),
        storage=store, redirect_handler=redirect_handler, callback_handler=callback_handler, client_metadata_url=CIMD)
    async with httpx2.AsyncClient(auth=auth, timeout=60, transport=transport) as http:
        async with Client(streamable_http_client(MCP + "/", http_client=http), mode="2026-07-28") as c:
            out["protocol"] = c.protocol_version
            # list_issues through graft-mcp, interleaved with the same REST read straight to the edge (M0-WP15's method).
            ok, lat, direct, errs = 0, [], [], []
            args = dict(zip(("owner", "repo"), REPO.split("/")), per_page=5)
            hdrs = {"authorization": f"token {member()}", "accept": "application/vnd.graftyard.agent+json", "user-agent": "am2-direct"}
            async with httpx2.AsyncClient(timeout=60) as plain:
                for _ in range(CALLS):
                    s = time.time()
                    try:
                        r = await c.call_tool("list_issues", args)
                        if r.is_error:
                            errs.append(str(r.content)[:160])
                        else:
                            ok += 1
                    except Exception as e:  # noqa: BLE001 -- the row records every failure
                        errs.append(str(e)[:160])
                    lat.append(int((time.time() - s) * 1000))
                    s = time.time()
                    await plain.get(f"{os.environ['GRAFT_EDGE']}/api/v3/repos/{REPO}/issues?per_page=5", headers=hdrs)
                    direct.append(int((time.time() - s) * 1000))
            cp, dp = sorted(lat)[len(lat) // 2], sorted(direct)[len(direct) // 2]
            out.update(calls=CALLS, ok=ok, clientP50=cp, directP50=dp, overheadP50=cp - dp, errors=errs[:3])


async def main() -> None:
    store, transport = Mem(), Dropper()
    out: dict = {"client": "py-v2", "sdk": "mcp 2.3.0 (Python)", "drop": DROP}
    t0 = time.time()
    # A client that does not retry the code itself raises here; a second session starts again from the 401.
    for attempt in range(3):
        try:
            await session(out, store, transport)
            break
        except Exception as e:  # noqa: BLE001
            out[f"sessionError{attempt}"] = repr(e)[:240]
    out.update(authMs=int((time.time() - t0) * 1000), consents=consents, tokenCalls=transport.calls,
               codeRedemptions=sum(1 for x in transport.calls if x["grant"] == "authorization_code"),
               iss=parse_qs(urlparse(callback.get("url", "")).query).get("iss", [None])[0],
               tokenScope=store.t.scope if store.t else None, refreshToken=bool(store.t and store.t.refresh_token))
    print(json.dumps(out))


asyncio.run(main())
