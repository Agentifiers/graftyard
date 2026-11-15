#!/usr/bin/env python3
"""Minimal mock of the GitHub Actions broker/run-service/results protocol for an
UNMODIFIED actions/runner (JIT config, UseV2Flow). Stdlib only, plain HTTP.

Usage:
  python3 mock.py --port 8790 --mode success --out /tmp/runner-proto/logs/run-success
  then: cd runner && ./run.sh --jitconfig "$(cat OUT/jitconfig.txt)"

Modes:
  success  one job: `echo hello`, `echo $MATRIX_VAL`
  kill     one job: `echo start; sleep 120` (kill the runner yourself; server logs what it sees)
  cancel   like kill, but the server sends a JobCancellation message 10 s after acquirejob
  errors   before handing out the job, GET /message returns: RunnerSessionInvalid, then 503 x2,
           then 429 (Retry-After: 2), then the success job
  renewfail like kill (sleep 120), but every renewjob after the first returns HTTP 500
"""
import argparse, base64, hashlib, json, os, secrets, sys, threading, time, uuid
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit, parse_qs

# ---------------------------------------------------------------- RSA (stdlib)

def _is_probable_prime(n, rounds=40):
    if n < 2:
        return False
    for p in (2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37):
        if n % p == 0:
            return n == p
    d, s = n - 1, 0
    while d % 2 == 0:
        d //= 2; s += 1
    for _ in range(rounds):
        a = secrets.randbelow(n - 3) + 2
        x = pow(a, d, n)
        if x in (1, n - 1):
            continue
        for _ in range(s - 1):
            x = pow(x, 2, n)
            if x == n - 1:
                break
        else:
            return False
    return True

def _prime(bits):
    while True:
        c = secrets.randbits(bits) | (1 << (bits - 1)) | (1 << (bits - 2)) | 1
        if _is_probable_prime(c):
            return c

def rsa_generate(bits=2048, e=65537):
    while True:
        p, q = _prime(bits // 2), _prime(bits // 2)
        if p == q:
            continue
        n = p * q
        if n.bit_length() != bits:
            continue
        phi = (p - 1) * (q - 1)
        try:
            d = pow(e, -1, phi)
        except ValueError:
            continue
        return dict(n=n, e=e, d=d, p=p, q=q, dp=d % (p - 1), dq=d % (q - 1), qi=pow(q, -1, p))

def _b(i, length):
    return i.to_bytes(length, "big")

def rsa_params_json(k):
    """Shape of .credentials_rsaparams: RSAParametersSerializable (IRSAKeyManager.cs) -> Newtonsoft
    byte[] as base64. .NET ImportParameters wants D == len(Modulus), P/Q/DP/DQ/InverseQ == len/2."""
    nl = (k["n"].bit_length() + 7) // 8
    hl = nl // 2
    enc = lambda i, l: base64.b64encode(_b(i, l)).decode()
    return {
        "d": enc(k["d"], nl), "dp": enc(k["dp"], hl), "dq": enc(k["dq"], hl),
        "exponent": enc(k["e"], 3), "inverseQ": enc(k["qi"], hl), "modulus": enc(k["n"], nl),
        "p": enc(k["p"], hl), "q": enc(k["q"], hl),
    }

_SHA256_DIGESTINFO = bytes.fromhex("3031300d060960864801650304020105000420")

def rs256_verify(n, e, signing_input, sig):
    nl = (n.bit_length() + 7) // 8
    em = _b(pow(int.from_bytes(sig, "big"), e, n), nl)
    t = _SHA256_DIGESTINFO + hashlib.sha256(signing_input).digest()
    return em == b"\x00\x01" + b"\xff" * (nl - len(t) - 3) + b"\x00" + t

def b64u_dec(s):
    return base64.urlsafe_b64decode(s + "=" * (-len(s) % 4))

def b64u(b):
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()

def fake_jwt(claims):
    # JobDispatcher.cs:105-121 parses this with JsonWebToken.Create (3 dot-separated parts, JSON
    # header/payload) to read orch_id; nothing on the runner verifies the signature.
    h = b64u(json.dumps({"typ": "JWT", "alg": "RS256"}).encode())
    p = b64u(json.dumps(claims).encode())
    return f"{h}.{p}.{b64u(b'mock-signature')}"

# ---------------------------------------------------------------- template / context helpers

def ctx_dict(d):
    """DictionaryContextData compact form: {"t":2,"d":[{"k":..,"v":..}]} (PipelineContextDataJsonConverter.cs)."""
    return {"t": 2, "d": [{"k": k, "v": v} for k, v in d.items()]}

def tmpl_map(pairs):
    """MappingToken: {"type":2,"map":[{"Key":..,"Value":..}]} (TemplateTokenJsonConverter.cs, MappingToken.cs:240)."""
    return {"type": 2, "map": [{"Key": k, "Value": v} for k, v in pairs]}

def expr(e):
    return {"type": 3, "expr": e}  # BasicExpressionToken

def script_step(name, script, env=None):
    step = {
        "type": "action",                     # StepType.Action (Step.cs / StepConverter.cs)
        "reference": {"type": "script"},      # ActionSourceType.Script (ActionStepDefinitionReference.cs)
        "id": str(uuid.uuid4()),
        "name": name,
        "contextName": name,
        "displayName": f"Run {script.splitlines()[0]}",
        "condition": "success()",
        "inputs": tmpl_map([("script", script)]),
    }
    if env:
        step["environment"] = tmpl_map(list(env.items()))
    return step

# ---------------------------------------------------------------- server state

class State:
    def __init__(self, args):
        self.args = args
        self.base = f"http://127.0.0.1:{args.port}"
        self.lock = threading.Lock()
        self.cv = threading.Condition(self.lock)
        self.messages = []          # queued TaskAgentMessage dicts
        self.next_message_id = 1
        self.session_id = None
        self.sessions = 0
        self.oauth_tokens = set()
        self.client_id = str(uuid.uuid4())
        self.key = rsa_generate()
        self.plan_id = str(uuid.uuid4())
        self.job_id = str(uuid.uuid4())
        self.timeline_id = str(uuid.uuid4())
        self.request_id = "req-" + uuid.uuid4().hex[:8]
        self.job_token = ("opaque-job-token-" + secrets.token_hex(8)) if args.opaque_token else fake_jwt({"orch_id": f"{self.plan_id}.build.__default", "scp": "Actions.Results:" +
                                   f"{self.plan_id}:{self.job_id}", "exp": int(time.time()) + 7200})
        self.acquired = False
        self.get_count = 0
        self.last_renew = None
        self.log_path = os.path.join(args.out, "calls.jsonl")
        self.t0 = time.time()

    def log(self, rec):
        rec = {"t": datetime.now(timezone.utc).isoformat(timespec="milliseconds"),
               "dt": round(time.time() - self.t0, 3), **rec}
        with self.lock:
            with open(self.log_path, "a") as f:
                f.write(json.dumps(rec) + "\n")
        print(f"[{rec['dt']:8.3f}] {rec.get('method','')} {rec.get('path','')} -> {rec.get('status','')} {rec.get('note','')}", flush=True)

    # ---- JIT config (Runner.cs:234-268 writes each key as a file under the runner root)
    def jit_config(self):
        runner = {
            "agentId": 1,
            "agentName": "graftyard-mock-runner",
            "poolId": 1,
            "poolName": "Default",
            "ephemeral": True,
            "disableUpdate": True,
            "gitHubUrl": f"{self.base}/mock-owner/mock-repo",
            "workFolder": "_work",
            "useV2Flow": True,
            "serverUrlV2": f"{self.base}/broker/",   # trailing slash matters: new Uri(base, "message")
        }
        creds = {
            "scheme": "OAuth",
            "data": {
                "clientId": self.client_id,
                "authorizationUrl": f"{self.base}/oauth/token",
                "requireFipsCryptography": "False",
            },
        }
        files = {
            ".runner": runner,
            ".credentials": creds,
            ".credentials_rsaparams": rsa_params_json(self.key),
        }
        enc = {k: base64.b64encode(json.dumps(v).encode()).decode() for k, v in files.items()}
        return base64.b64encode(json.dumps(enc).encode()).decode(), files

    # ---- the job message
    def job_message(self):
        mode = self.args.mode
        if mode in ("kill", "cancel", "renewfail"):
            steps = [script_step("__run", "echo start-long-step; sleep 120; echo should-not-print")]
        else:
            steps = [script_step("__run", "echo hello"),
                     script_step("__run_2", "echo $MATRIX_VAL", env={"MATRIX_VAL": expr("matrix.val")})]
        now = datetime.now(timezone.utc)
        return {
            "messageType": "RunnerJobRequest",
            "plan": {"scopeIdentifier": str(uuid.uuid4()), "planType": "actions", "version": 1,
                     "planId": self.plan_id},
            "timeline": {"id": self.timeline_id},
            "jobId": self.job_id,
            "jobDisplayName": "build (from-matrix)",
            "jobName": "build",
            "requestId": 1,
            "lockedUntil": (now + timedelta(minutes=10)).isoformat(),
            "billingOwnerId": "owner-1",
            "resources": {"endpoints": [{
                "name": "SystemVssConnection",
                "url": f"{self.base}/run/",           # renewjob/completejob go here, trailing slash matters
                "authorization": {"scheme": "OAuth", "parameters": {"AccessToken": self.job_token}},
                "data": {},
            }]},
            "contextData": {
                "github": ctx_dict({
                    "repository": "mock-owner/mock-repo", "repository_owner": "mock-owner",
                    "event_name": "push", "ref": "refs/heads/main", "sha": "0" * 40,
                    "server_url": self.base, "api_url": f"{self.base}/api/v3",
                    "graphql_url": f"{self.base}/api/graphql", "workflow": "ci",
                    "run_id": "1", "run_number": "1", "run_attempt": "1", "actor": "mock-user",
                    "job": "build",
                }),
                "matrix": ctx_dict({"val": "from-matrix"}),
                "strategy": ctx_dict({"fail-fast": True, "job-index": 0, "job-total": 1, "max-parallel": 1}),
                "needs": {"t": 2},
                "inputs": {"t": 2},
            },
            "variables": {
                "system.github.token": {"value": "ghs_mocktoken", "isSecret": True},
                "system.github.job": {"value": "build"},
                **({} if self.args.results_url == "none" else
                   {"system.github.results_endpoint": {"value": self.args.results_url or f"{self.base}/results/"}}),
            },
            "steps": steps,
            "workspace": {},
            "fileTable": [".github/workflows/ci.yml"],
            "mask": [],
        }

    def minimal_job_message(self):
        """Smallest message proven to complete (see --minimal). Everything not here is optional."""
        return {
            "messageType": "RunnerJobRequest",
            "plan": {"planId": self.plan_id},
            "timeline": {"id": self.timeline_id},
            "jobId": self.job_id,
            "resources": {"endpoints": [{
                "name": "SystemVssConnection",
                "url": f"{self.base}/run/",
                "authorization": {"scheme": "OAuth", "parameters": {"AccessToken": self.job_token}},
            }]},
            "contextData": {
                "github": ctx_dict({"repository": "mock-owner/mock-repo"}),
                "matrix": ctx_dict({"val": "from-matrix"}),
            },
            "variables": {"system.github.results_endpoint": {"value": f"{self.base}/results/"}},
            "steps": [script_step("__run", "echo hello"),
                      script_step("__run_2", "echo $MATRIX_VAL", env={"MATRIX_VAL": expr("matrix.val")})],
        }

    def enqueue(self, mtype, body):
        with self.cv:
            self.messages.append({"messageId": self.next_message_id, "messageType": mtype,
                                  "body": json.dumps(body)})
            self.next_message_id += 1
            self.cv.notify_all()

# ---------------------------------------------------------------- handler

class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    S: State = None

    def log_message(self, *a):
        pass

    def _read_body(self):
        if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
            data = b""
            while True:
                size = int(self.rfile.readline().strip(), 16)
                if size == 0:
                    self.rfile.readline()
                    break
                data += self.rfile.read(size)
                self.rfile.readline()
            return data
        n = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(n) if n else b""

    def _send(self, status, obj=None, raw=None, ctype="application/json", extra=None):
        data = raw if raw is not None else (json.dumps(obj).encode() if obj is not None else b"")
        try:
            self.send_response(status)
            if data:
                self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(data)))
            for k, v in (extra or {}).items():
                self.send_header(k, v)
            self.end_headers()
            if data:
                self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError) as e:
            self.S.log({"method": self.command, "path": self.path, "note": f"client gone before response: {e!r}"})
            return False
        return True

    def _auth(self):
        a = self.headers.get("Authorization", "")
        if not a.startswith("Bearer "):
            return "none"
        tok = a[7:]
        if tok in self.S.oauth_tokens:
            return "runner-oauth"
        if tok == self.S.job_token:
            return "job-token(SystemVssConnection)"
        return "unknown-bearer"

    def do_GET(self): self.route()
    def do_POST(self): self.route()
    def do_PUT(self): self.route()
    def do_DELETE(self): self.route()

    def route(self):
        S = self.S
        u = urlsplit(self.path)
        q = {k: v[0] for k, v in parse_qs(u.query).items()}
        body = self._read_body()
        ctype = self.headers.get("Content-Type", "")
        try:
            jbody = json.loads(body) if body and "json" in ctype else None
        except ValueError:
            jbody = None
        rec = {"method": self.command, "path": u.path, "query": q, "auth": self._auth(),
               "ua": self.headers.get("User-Agent", "")[:120], "ctype": ctype}
        if jbody is not None:
            rec["body"] = jbody
        elif body and not u.path.startswith("/blob/"):
            rec["body_text"] = body.decode(errors="replace")[:2000]
        p = u.path

        # -------- OAuth token exchange (VssOAuthTokenHttpClient.cs:32-78)
        if p == "/oauth/token" and self.command == "POST":
            form = {k: v[0] for k, v in parse_qs(body.decode()).items()}
            assertion = form.get("client_assertion", "")
            h, pl, sg = assertion.split(".")
            claims = json.loads(b64u_dec(pl))
            header = json.loads(b64u_dec(h))
            ok_sig = rs256_verify(S.key["n"], S.key["e"], f"{h}.{pl}".encode(), b64u_dec(sg))
            ok_claims = (claims.get("iss") == S.client_id and claims.get("sub") == S.client_id
                         and claims.get("aud") == f"{S.base}/oauth/token" and claims.get("exp", 0) > time.time() - 300)
            rec["body"] = {"grant_type": form.get("grant_type"), "client_assertion_type": form.get("client_assertion_type"),
                           "assertion_header": header, "assertion_claims": claims,
                           "signature_valid": ok_sig, "claims_valid": ok_claims}
            if not (ok_sig and ok_claims and form.get("grant_type") == "client_credentials"):
                rec["status"] = 400
                S.log(rec)
                return self._send(400, {"error": "invalid_client", "error_description": "bad assertion"})
            tok = "oauth-" + secrets.token_hex(16)
            with S.lock:
                S.oauth_tokens.add(tok)
            rec["status"] = 200
            S.log(rec)
            return self._send(200, {"access_token": tok, "token_type": "bearer", "expires_in": 3600})

        # -------- broker
        if p == "/broker/session" and self.command == "POST":
            with S.cv:
                S.session_id = str(uuid.uuid4())
                S.sessions += 1
                first = S.sessions == 1
            resp = {"sessionId": S.session_id, "ownerName": (jbody or {}).get("ownerName"),
                    "agent": (jbody or {}).get("agent"), "brokerSession": True}
            rec["status"] = 200; rec["response"] = resp
            S.log(rec)
            self._send(200, resp)
            if first and S.args.mode != "errors":
                S.enqueue("RunnerJobRequest", S.job_ref())
            return
        if p == "/broker/session" and self.command == "DELETE":
            rec["status"] = 200
            S.log(rec)
            return self._send(200)
        if p == "/broker/acknowledge" and self.command == "POST":
            rec["status"] = 200
            S.log(rec)
            return self._send(200, {})
        if p == "/broker/message" and self.command == "GET":
            return self.get_message(rec, q)

        # -------- run service
        if p == "/run/acquirejob" and self.command == "POST":
            with S.lock:
                already = S.acquired
                S.acquired = True
            if already:
                rec["status"] = 409
                S.log(rec)
                return self._send(409, {"source": "actions-run-service", "statusCode": 409, "errorMessage": "already acquired"})
            msg = S.minimal_job_message() if S.args.minimal else S.job_message()
            with open(os.path.join(S.args.out, "acquirejob_response.json"), "w") as f:
                json.dump(msg, f, indent=2)
            rec["status"] = 200; rec["note"] = "job message handed out"
            S.log(rec)
            self._send(200, msg)
            if S.args.mode == "cancel":
                threading.Timer(10, lambda: S.enqueue("JobCancellation", {"jobId": S.job_id, "timeout": "00:00:30"})).start()
            return
        if p == "/run/renewjob" and self.command == "POST":
            now = time.time()
            if S.args.mode == "renewfail" and S.last_renew is not None:
                rec["status"] = 500; rec["note"] = "renewfail mode: 500 after first renew"
                S.log(rec)
                return self._send(500, raw=b"boom", ctype="text/plain")
            rec["note"] = f"since last renew: {round(now - S.last_renew, 1) if S.last_renew else None}s"
            S.last_renew = now
            rec["status"] = 200
            S.log(rec)
            return self._send(200, {"lockedUntil": (datetime.now(timezone.utc) + timedelta(minutes=10)).isoformat()})
        if p == "/run/completejob" and self.command == "POST":
            with open(os.path.join(S.args.out, "completejob.json"), "w") as f:
                json.dump(jbody, f, indent=2)
            rec["status"] = 200; rec["note"] = f"conclusion={(jbody or {}).get('conclusion')}"
            S.log(rec)
            return self._send(200)

        # -------- results service (ResultsHttpClient.cs)
        if p.startswith("/results/twirp/"):
            method = p.rsplit("/", 1)[-1]
            b = jbody or {}
            if method in ("GetStepLogsSignedBlobURL", "GetJobLogsSignedBlobURL", "GetJobDiagLogsSignedBlobURL",
                          "GetStepSummarySignedBlobURL"):
                name = f"{method.replace('Get','').replace('SignedBlobURL','')}-{b.get('step_backend_id', 'job')}"
                url = f"{S.base}/blob/{name}?sig=mock"
                key = {"GetStepLogsSignedBlobURL": "logs_url", "GetJobLogsSignedBlobURL": "logs_url",
                       "GetJobDiagLogsSignedBlobURL": "diag_logs_url", "GetStepSummarySignedBlobURL": "summary_url"}[method]
                resp = {key: url, "blob_storage_type": "BLOB_STORAGE_TYPE_UNSPECIFIED", "soft_size_limit": 10 * 1024 * 1024}
                rec["status"] = 200; rec["response"] = resp
                S.log(rec)
                return self._send(200, resp)
            rec["status"] = 200
            S.log(rec)
            return self._send(200, {"ok": True})
        if p.startswith("/blob/") and self.command == "PUT":
            name = p[len("/blob/"):]
            path = os.path.join(S.args.out, "blobs", name)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "ab") as f:
                f.write(body)
            rec["note"] = f"{len(body)} bytes"
            rec["blob_headers"] = {k: v for k, v in self.headers.items() if k.lower().startswith("x-ms") or k.lower() == "content-type"}
            rec["status"] = 201
            S.log(rec)
            return self._send(201)

        rec["status"] = 404; rec["note"] = "UNHANDLED"
        S.log(rec)
        return self._send(404, {"message": "not found"})

    def get_message(self, rec, q):
        S = self.S
        with S.cv:
            S.get_count += 1
            n = S.get_count
        if S.args.mode == "errors":
            if n == 1:
                rec["status"] = 400; rec["note"] = "RunnerSessionInvalid"
                S.log(rec)
                return self._send(400, {"source": "actions-broker-listener", "errorKind": "RunnerSessionInvalid",
                                        "statusCode": 400, "errorMessage": "mock: session invalid"})
            if n in (2, 3):
                rec["status"] = 503; rec["note"] = "plain 503"
                S.log(rec)
                return self._send(503, raw=b"busy", ctype="text/plain")
            if n == 4:
                rec["status"] = 429; rec["note"] = "429 Retry-After: 2"
                S.log(rec)
                return self._send(429, raw=b"slow down", ctype="text/plain", extra={"Retry-After": "2"})
            if n == 5:
                S.enqueue("RunnerJobRequest", S.job_ref())
        if q.get("sessionId") != S.session_id:
            rec["note"] = f"sessionId mismatch (current {S.session_id})"
        deadline = time.time() + S.args.poll
        with S.cv:
            while not S.messages and time.time() < deadline:
                S.cv.wait(timeout=max(0.0, deadline - time.time()))
            msg = S.messages.pop(0) if S.messages else None
        if msg is None:
            rec["status"] = 200; rec["note"] = (rec.get("note", "") + " long-poll timeout, empty 200").strip()
            S.log(rec)
            return self._send(200)
        rec["status"] = 200; rec["response"] = msg; rec["note"] = f"deliver {msg['messageType']}"
        if not self._send(200, msg):
            with S.cv:  # runner had abandoned this poll; put the message back
                S.messages.insert(0, msg)
            rec["note"] += " (FAILED, requeued)"
        S.log(rec)

def _job_ref(self):
    # RunnerJobRequestRef.cs: snake_case DataMember names
    return {"id": str(uuid.uuid4()), "runner_request_id": self.request_id, "should_acknowledge": True,
            "run_service_url": f"{self.base}/run/", "billing_owner_id": "owner-1"}
State.job_ref = _job_ref

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8790)
    ap.add_argument("--mode", choices=["success", "kill", "cancel", "errors", "renewfail"], default="success")
    ap.add_argument("--out", required=True)
    ap.add_argument("--poll", type=float, default=20.0, help="long-poll hold seconds for GET /message")
    ap.add_argument("--results-url", default=None,
                    help="override system.github.results_endpoint; 'none' omits the variable")
    ap.add_argument("--minimal", action="store_true", help="hand out the minimal job message")
    ap.add_argument("--opaque-token", action="store_true", help="SystemVssConnection AccessToken is not JWT-shaped")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    S = State(args)
    jit, files = S.jit_config()
    with open(os.path.join(args.out, "jitconfig.txt"), "w") as f:
        f.write(jit)
    with open(os.path.join(args.out, "jit_files.json"), "w") as f:
        json.dump({k: v for k, v in files.items() if k != ".credentials_rsaparams"}, f, indent=2)
    Handler.S = S
    srv = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    srv.daemon_threads = True
    print(f"mock listening on {S.base} mode={args.mode} out={args.out}", flush=True)
    srv.serve_forever()

if __name__ == "__main__":
    main()
