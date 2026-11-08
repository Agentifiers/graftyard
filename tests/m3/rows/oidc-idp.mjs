// A minimal OIDC provider for BF-36: discovery, an authorization endpoint that signs in the user the driver names
// (`login_hint`, no page), a token endpoint that checks the client secret and PKCE, and a JWKS. RS256 with one key made
// at start and kept in memory only. The issuer is the public URL in ISSUER_FILE (the tunnel's), read at each request,
// so the service can start before the tunnel. The client secret is read from SECRET_FILE (0600). Nothing secret is
// logged.
//   PORT=... ISSUER_FILE=... SECRET_FILE=... node tests/m3/rows/oidc-idp.mjs
import { createHash, createSign, generateKeyPairSync, randomBytes } from "node:crypto";
import { readFileSync } from "node:fs";
import { createServer } from "node:http";

const PORT = Number(process.env.PORT ?? 8787);
const CLIENT = process.env.OIDC_CLIENT ?? "graftyard-m2b";
const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
const jwk = publicKey.export({ format: "jwk" });
const KID = createHash("sha256").update(jwk.n).digest("hex").slice(0, 16);
const b64 = (b) => Buffer.from(b).toString("base64url");
const issuer = () => readFileSync(process.env.ISSUER_FILE, "utf8").trim();
const secret = () => readFileSync(process.env.SECRET_FILE, "utf8").trim();
// code -> { user, nonce, challenge, redirect, at }
const codes = new Map();
const log = (m) => console.log(`${new Date().toISOString()} ${m}`);

function sign(claims) {
  const h = b64(JSON.stringify({ alg: "RS256", kid: KID, typ: "JWT" }));
  const p = b64(JSON.stringify(claims));
  const s = createSign("RSA-SHA256").update(`${h}.${p}`).sign(privateKey);
  return `${h}.${p}.${b64(s)}`;
}

const json = (res, status, v) => { res.writeHead(status, { "content-type": "application/json" }); res.end(JSON.stringify(v)); };

createServer(async (req, res) => {
  const url = new URL(req.url, "http://idp");
  const iss = issuer();
  if (url.pathname === "/.well-known/openid-configuration") {
    return json(res, 200, { issuer: iss, authorization_endpoint: `${iss}/authorize`, token_endpoint: `${iss}/token`,
      jwks_uri: `${iss}/jwks`, response_types_supported: ["code"], subject_types_supported: ["public"],
      id_token_signing_alg_values_supported: ["RS256"], code_challenge_methods_supported: ["S256"] });
  }
  if (url.pathname === "/jwks") return json(res, 200, { keys: [{ ...jwk, kid: KID, alg: "RS256", use: "sig" }] });
  if (url.pathname === "/authorize") {
    const q = url.searchParams;
    if (q.get("client_id") !== CLIENT || q.get("response_type") !== "code" || q.get("code_challenge_method") !== "S256") {
      log(`authorize refused: client ${q.get("client_id") === CLIENT}`);
      return json(res, 400, { error: "invalid_request" });
    }
    const code = randomBytes(24).toString("base64url");
    codes.set(code, { user: q.get("login_hint") ?? "", nonce: q.get("nonce"), challenge: q.get("code_challenge"), redirect: q.get("redirect_uri"), at: Date.now() });
    const to = new URL(q.get("redirect_uri"));
    to.searchParams.set("code", code);
    to.searchParams.set("state", q.get("state") ?? "");
    log(`authorize: ${q.get("login_hint")}`);
    res.writeHead(302, { location: to.toString() });
    return res.end();
  }
  if (url.pathname === "/token" && req.method === "POST") {
    let body = "";
    for await (const ch of req) body += ch;
    const f = new URLSearchParams(body);
    const row = codes.get(f.get("code") ?? "");
    codes.delete(f.get("code") ?? "");
    const pkce = row && createHash("sha256").update(f.get("code_verifier") ?? "").digest("base64url") === row.challenge;
    const ok = row && f.get("client_id") === CLIENT && f.get("client_secret") === secret() && pkce && f.get("redirect_uri") === row.redirect && Date.now() - row.at < 600_000;
    log(`token: ${ok ? "issued" : `refused (code ${!!row}, pkce ${!!pkce})`}`);
    if (!ok) return json(res, 400, { error: "invalid_grant" });
    const now = Math.floor(Date.now() / 1000);
    const id_token = sign({ iss, aud: CLIENT, sub: `idp-${row.user}`, preferred_username: row.user, nonce: row.nonce, iat: now, exp: now + 300 });
    return json(res, 200, { access_token: randomBytes(16).toString("base64url"), token_type: "Bearer", expires_in: 300, id_token });
  }
  json(res, 404, { error: "not_found" });
}).listen(PORT, () => log(`listening on ${PORT}, kid ${KID}`));
