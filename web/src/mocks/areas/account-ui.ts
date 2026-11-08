import { HttpResponse } from "msw";
import { createOpenApiHttp } from "openapi-msw";
import { SCOPE_NAMES } from "@/areas/account/permissions";
import { UI_BASE, type AppPermissions, type Token, type UiPaths } from "@/areas/account/ui-api";
import { gate, notFound } from "../http";
import { API, HOST, id, me, users } from "../world";
import {
  account,
  addInstallation,
  audit,
  AUTH_CODE_TTL_S,
  authCodes,
  authorizeApp,
  db,
  defaultAvatar,
  DEVICE_TTL_MS,
  deviceCodes,
  grantFor,
  hexSecret,
  lastEight,
  loginAttempts,
  makeRecoveryCodes,
  manifestCodes,
  myAvatar,
  pendingTwoFactor,
  recoveryCodes,
  RESERVED_LOGINS,
  security,
  setRecoveryCodes,
  storedToken,
  takenLogins,
  userCode,
  wireInstallation,
  type AppRecord,
  type OAuthClient,
} from "./account-db";

// The private web API behind the account pages, typed by web/src/areas/account/ui-api.ts.
const http = createOpenApiHttp<UiPaths>({ baseUrl: UI_BASE });

const invalid = (resource: string, field: string, code: string, message: string) =>
  HttpResponse.json(
    {
      message: "Validation Failed",
      documentation_url: "",
      status: "422",
      errors: [{ resource, field, code, message }],
    },
    { status: 422 },
  );
const failed = (status: number, message: string) =>
  HttpResponse.json({ message, documentation_url: "", status: String(status) }, { status });

const URL_RE = /^https?:\/\/[^\s/]+\.[^\s]+$|^https?:\/\/(localhost|127\.0\.0\.1)(:\d+)?(\/\S*)?$/;
const LOGIN_RE = /^[a-z\d](?:[a-z\d]|-(?=[a-z\d])){0,38}$/i;
const SIX_DIGITS = /^\d{6}$/;
const now = () => new Date().toISOString();

// Any six digits pass except 000000, which stands for a wrong code. The mock has no clock
// shared with an authenticator app.
const otpOk = (code: string) => SIX_DIGITS.test(code) && code !== "000000";

function checkToken(t: { kind: string; expires_at: string | null } & Record<string, unknown>) {
  if (t.expires_at && new Date(t.expires_at).getTime() <= Date.now()) {
    return invalid(
      "PersonalAccessToken",
      "expires_at",
      "invalid",
      "Expiration must be in the future",
    );
  }
  if (t.expires_at && new Date(t.expires_at).getTime() > Date.now() + 366 * 86_400_000) {
    return invalid(
      "PersonalAccessToken",
      "expires_at",
      "invalid",
      "Expiration must be within a year",
    );
  }
  return undefined;
}

// A real RSA key, so the downloaded .pem opens in openssl.
async function rsaKey() {
  const pair = await crypto.subtle.generateKey(
    {
      name: "RSASSA-PKCS1-v1_5",
      modulusLength: 2048,
      publicExponent: new Uint8Array([1, 0, 1]),
      hash: "SHA-256",
    },
    true,
    ["sign", "verify"],
  );
  const pkcs8 = new Uint8Array(await crypto.subtle.exportKey("pkcs8", pair.privateKey));
  const spki = new Uint8Array(await crypto.subtle.exportKey("spki", pair.publicKey));
  const b64 = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes));
  const pem = `-----BEGIN PRIVATE KEY-----\n${b64(pkcs8)
    .match(/.{1,64}/g)!
    .join("\n")}\n-----END PRIVATE KEY-----\n`;
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", spki));
  return { pem, fingerprint: `SHA256:${b64(digest).replace(/=+$/, "")}` };
}

function devApp(a: AppRecord) {
  const { webhook_secret, client_secrets, deliveries, ...rest } = a;
  return rest;
}

const slugify = (name: string) =>
  name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");

function checkManifest(
  m: {
    name?: string;
    homepage_url?: string;
    callback_urls?: string[];
    setup_url?: string;
    hook_attributes?: { active: boolean; url: string };
    events?: string[];
    permissions?: AppPermissions;
  },
  self?: AppRecord,
) {
  if (m.name !== undefined) {
    if (!m.name.trim())
      return invalid("Integration", "name", "missing_field", "Name can't be blank");
    if (m.name.length > 34)
      return invalid(
        "Integration",
        "name",
        "too_long",
        "Name is too long (maximum is 34 characters)",
      );
    const slug = slugify(m.name);
    if (
      db.apps.some(
        (a) => a !== self && (a.slug === slug || a.name.toLowerCase() === m.name!.toLowerCase()),
      )
    ) {
      return invalid("Integration", "name", "already_exists", "Name is already in use");
    }
  }
  if (m.homepage_url !== undefined && !URL_RE.test(m.homepage_url)) {
    return invalid(
      "Integration",
      "homepage_url",
      "invalid",
      "Homepage URL must be an http or https URL",
    );
  }
  for (const u of m.callback_urls ?? []) {
    if (!URL_RE.test(u))
      return invalid(
        "Integration",
        "callback_urls",
        "invalid",
        `Callback URL ${u} is not a valid URL`,
      );
  }
  if (m.setup_url && !URL_RE.test(m.setup_url)) {
    return invalid("Integration", "setup_url", "invalid", "Setup URL must be an http or https URL");
  }
  if (m.hook_attributes?.active && !URL_RE.test(m.hook_attributes.url)) {
    return invalid(
      "Integration",
      "hook_attributes.url",
      "invalid",
      "Webhook URL is required while the webhook is active",
    );
  }
  if (m.events?.length && !m.hook_attributes?.active && !self?.webhook.active) {
    return invalid("Integration", "events", "invalid", "Events need an active webhook");
  }
  return undefined;
}

function checkOAuthApp(
  b: { name: string; homepage_url: string; callback_url: string },
  self?: OAuthClient,
) {
  if (!b.name.trim())
    return invalid("OauthApplication", "name", "missing_field", "Application name can't be blank");
  if (
    db.oauthApps.some(
      (a) =>
        a !== self && a.owner.login === me.login && a.name.toLowerCase() === b.name.toLowerCase(),
    )
  ) {
    return invalid(
      "OauthApplication",
      "name",
      "already_exists",
      "Application name is already in use",
    );
  }
  if (!URL_RE.test(b.homepage_url)) {
    return invalid(
      "OauthApplication",
      "homepage_url",
      "invalid",
      "Homepage URL must be an http or https URL",
    );
  }
  if (!URL_RE.test(b.callback_url) && !/^[a-z][a-z0-9+.-]*:\/\//.test(b.callback_url)) {
    return invalid(
      "OauthApplication",
      "callback_url",
      "invalid",
      "Authorization callback URL is not a valid URL",
    );
  }
  return undefined;
}

// What a client's redirect_uri may be: the registered callback's host and path prefix,
// and any port on localhost for a localhost callback, as GitHub allows for native apps.
function redirectAllowed(registered: string, asked: string) {
  try {
    const r = new URL(registered);
    const a = new URL(asked);
    if (r.protocol !== a.protocol) return false;
    const local = (h: string) => h === "localhost" || h === "127.0.0.1";
    if (local(r.hostname) && local(a.hostname))
      return a.pathname.startsWith(r.pathname.replace(/\/$/, ""));
    return r.host === a.host && a.pathname.startsWith(r.pathname);
  } catch {
    return false;
  }
}

function findClient(clientId: string) {
  const oauth = db.oauthApps.find((a) => a.client_id === clientId);
  if (oauth) return { kind: "oauth_app" as const, oauth };
  const app = db.apps.find((a) => a.client_id === clientId);
  if (app) return { kind: "github_app" as const, app };
  return undefined;
}

const deviceView = (row: (typeof deviceCodes)[number]) => {
  const client = db.oauthApps.find((a) => a.client_id === row.client_id);
  return {
    user_code: row.user_code,
    app: {
      name: client?.name ?? "Unknown app",
      owner: client?.owner.login ?? "",
      client_id: row.client_id,
    },
    scopes: row.scopes,
    created_at: new Date(row.created_ms).toISOString(),
    expires_at: new Date(row.created_ms + DEVICE_TTL_MS).toISOString(),
  };
};

// XXXX-XXXX, case and hyphen forgiven.
const normalizeCode = (code: string) => {
  const raw = code.toUpperCase().replace(/[^A-Z0-9]/g, "");
  return raw.length === 8 ? `${raw.slice(0, 4)}-${raw.slice(4)}` : raw;
};

function findDeviceCode(code: string) {
  const row = deviceCodes.find((d) => d.user_code === normalizeCode(code));
  if (!row)
    return {
      error: invalid(
        "DeviceCode",
        "user_code",
        "invalid",
        "That code is not valid. Check it and try again.",
      ),
    };
  if (Date.now() - row.created_ms > DEVICE_TTL_MS) {
    return {
      error: invalid(
        "DeviceCode",
        "user_code",
        "expired",
        "That code has expired. Start again on your device for a new one.",
      ),
    };
  }
  if (row.state !== "pending") {
    return {
      error: invalid(
        "DeviceCode",
        "user_code",
        "already_used",
        "That code has already been used. Start again on your device for a new one.",
      ),
    };
  }
  return { row };
}

export const handlers = [
  // ---- Profile, account ----

  http.post("/settings/avatar", async ({ request, response }) => {
    const fault = await gate("account-avatar");
    if (fault) return response.untyped(fault);
    const { data_url } = await request.json();
    if (!/^data:image\/(png|jpeg|gif|webp|svg\+xml);base64,/.test(data_url)) {
      return response.untyped(
        invalid("Avatar", "file", "invalid", "Upload a PNG, JPEG, GIF, or WebP image"),
      );
    }
    if (data_url.length > 1_400_000) {
      return response.untyped(
        invalid("Avatar", "file", "too_large", "Images must be 1 MB or smaller"),
      );
    }
    me.avatar_url = data_url;
    myAvatar.custom = true;
    audit("user.update_avatar", "Uploaded a new profile picture");
    return response(200).json(me);
  }),

  http.delete("/settings/avatar", async ({ response }) => {
    const fault = await gate("account-avatar");
    if (fault) return response.untyped(fault);
    me.avatar_url = defaultAvatar();
    myAvatar.custom = false;
    audit("user.update_avatar", "Removed the profile picture");
    return response(200).json(me);
  }),

  http.patch("/settings/account/login", async ({ request, response }) => {
    const fault = await gate("account-rename");
    if (fault) return response.untyped(fault);
    const { login } = await request.json();
    if (!LOGIN_RE.test(login)) {
      return response.untyped(
        invalid(
          "User",
          "login",
          "invalid",
          "Username may only contain letters, digits, and single hyphens, and cannot begin or end with a hyphen",
        ),
      );
    }
    if (login.toLowerCase() === me.login.toLowerCase()) {
      return response.untyped(
        invalid("User", "login", "unchanged", "That is already your username"),
      );
    }
    if (RESERVED_LOGINS.has(login.toLowerCase()) || takenLogins().has(login.toLowerCase())) {
      return response.untyped(
        invalid("User", "login", "already_exists", `Username ${login} is not available`),
      );
    }
    const old = me.login;
    me.login = login;
    me.url = `${API}/users/${login}`;
    me.html_url = `${HOST}/${login}`;
    if (!myAvatar.custom) me.avatar_url = defaultAvatar();
    me.updated_at = now();
    audit("user.rename", `Renamed ${old} to ${login}`);
    return response(200).json(me);
  }),

  http.delete("/settings/account", async ({ request, response }) => {
    const fault = await gate("account-delete");
    if (fault) return response.untyped(fault);
    const { confirm_login, confirm_phrase } = await request.json();
    if (confirm_login !== me.login) {
      return response.untyped(
        invalid("User", "confirm_login", "invalid", "Type your username exactly"),
      );
    }
    if (confirm_phrase !== "delete my account") {
      return response.untyped(
        invalid("User", "confirm_phrase", "invalid", "Type delete my account to confirm"),
      );
    }
    account.deleted_at = now();
    audit("user.destroy", "Scheduled the account for deletion");
    return response(202).json({ deleted_at: account.deleted_at });
  }),

  // ---- Emails beyond REST ----

  http.patch("/settings/emails/primary", async ({ request, response }) => {
    const fault = await gate("account-email-primary");
    if (fault) return response.untyped(fault);
    const { email } = await request.json();
    const target = db.emails.find((e) => e.email === email);
    if (!target)
      return response.untyped(
        invalid("UserEmail", "email", "invalid", "That address is not on your account"),
      );
    if (!target.verified) {
      return response.untyped(
        invalid(
          "UserEmail",
          "email",
          "unverified",
          "Verify the address before you make it primary",
        ),
      );
    }
    const wasPublic = db.emailVisibility.visibility === "public";
    for (const e of db.emails) {
      e.primary = e === target;
      e.visibility = e.primary && wasPublic ? "public" : e.primary ? "private" : null;
    }
    audit("user.change_primary_email", `Made ${email} primary`);
    return response(200).json(db.emails);
  }),

  http.get("/settings/emails/visibility", async ({ response }) => {
    const fault = await gate("account-email-prefs");
    if (fault) return response.untyped(fault);
    return response(200).json(db.emailVisibility);
  }),

  http.patch("/settings/emails/visibility", async ({ request, response }) => {
    const fault = await gate("account-email-visibility");
    if (fault) return response.untyped(fault);
    const body = await request.json();
    Object.assign(db.emailVisibility, body);
    const primary = db.emails.find((e) => e.primary);
    if (primary) primary.visibility = db.emailVisibility.visibility;
    if (body.visibility === "private" && me.email === primary?.email) me.email = null;
    audit(
      "user.email_visibility",
      `Primary email ${db.emailVisibility.visibility}, block pushes ${db.emailVisibility.block_push ? "on" : "off"}`,
    );
    return response(200).json(db.emailVisibility);
  }),

  http.post("/settings/emails/verify", async ({ request, response }) => {
    const fault = await gate("account-email-verify");
    if (fault) return response.untyped(fault);
    const { email } = await request.json();
    const target = db.emails.find((e) => e.email === email);
    if (!target)
      return response.untyped(
        invalid("UserEmail", "email", "invalid", "That address is not on your account"),
      );
    if (target.verified)
      return response.untyped(
        invalid("UserEmail", "email", "already_verified", "That address is already verified"),
      );
    setTimeout(() => (target.verified = true), 5000);
    return response(202).json({ email });
  }),

  // ---- Personal access tokens ----

  http.get("/settings/tokens", async ({ response }) => {
    const fault = await gate("account-tokens");
    if (fault) return response.untyped(fault);
    return response(200).json(
      [...db.tokens].sort((a, b) => b.created_at.localeCompare(a.created_at)),
    );
  }),

  http.post("/settings/tokens", async ({ request, response }) => {
    const fault = await gate("account-token-create");
    if (fault) return response.untyped(fault);
    const body = await request.json();
    const bad = checkToken(body);
    if (bad) return response.untyped(bad);
    let token: Token;
    let value: string;
    if (body.kind === "fine_grained") {
      if (!body.name.trim())
        return response.untyped(
          invalid("PersonalAccessToken", "name", "missing_field", "Token name can't be blank"),
        );
      if (
        db.tokens.some(
          (t) =>
            t.kind === "fine_grained" && t.name.toLowerCase() === body.name.trim().toLowerCase(),
        )
      ) {
        return response.untyped(
          invalid(
            "PersonalAccessToken",
            "name",
            "already_exists",
            "Token name has already been taken",
          ),
        );
      }
      if (body.repository_selection === "selected" && body.repositories.length === 0) {
        return response.untyped(
          invalid(
            "PersonalAccessToken",
            "repositories",
            "missing_field",
            "Select at least one repository",
          ),
        );
      }
      if (body.resource_owner !== me.login && body.resource_owner !== "graftyard") {
        return response.untyped(
          invalid(
            "PersonalAccessToken",
            "resource_owner",
            "invalid",
            "You cannot create tokens for that owner",
          ),
        );
      }
      value = storedToken("github_pat_");
      token = {
        ...body,
        name: body.name.trim(),
        id: id(),
        token_last_eight: lastEight(value),
        created_at: now(),
        last_used_at: null,
      };
      audit("personal_access_token.create", `Created fine-grained token ${token.name}`);
    } else {
      if (!body.note.trim())
        return response.untyped(
          invalid("OauthAccess", "note", "missing_field", "Note can't be blank"),
        );
      if (
        db.tokens.some(
          (t) => t.kind === "classic" && t.note.toLowerCase() === body.note.trim().toLowerCase(),
        )
      ) {
        return response.untyped(
          invalid("OauthAccess", "note", "already_exists", "Note has already been taken"),
        );
      }
      const unknown = body.scopes.find((s) => !SCOPE_NAMES.has(s));
      if (unknown)
        return response.untyped(
          invalid("OauthAccess", "scopes", "invalid", `Unknown scope ${unknown}`),
        );
      value = storedToken("ghp_");
      token = {
        ...body,
        note: body.note.trim(),
        id: id(),
        token_last_eight: lastEight(value),
        created_at: now(),
        last_used_at: null,
      };
      audit("oauth_access.create", `Created classic token ${token.note}`);
    }
    db.tokens.push(token);
    return response(201).json({ token, value });
  }),

  http.delete("/settings/tokens/{id}", async ({ params, response }) => {
    const fault = await gate("account-token-delete");
    if (fault) return response.untyped(fault);
    const i = db.tokens.findIndex((t) => t.id === Number(params.id));
    if (i < 0) return response.untyped(notFound());
    const [t] = db.tokens.splice(i, 1);
    audit(
      "personal_access_token.destroy",
      `Deleted token ${t.kind === "classic" ? t.note : t.name}`,
    );
    return response(204).empty();
  }),

  http.post("/settings/tokens/{id}/regenerate", async ({ params, request, response }) => {
    const fault = await gate("account-token-regenerate");
    if (fault) return response.untyped(fault);
    const t = db.tokens.find((x) => x.id === Number(params.id));
    if (!t) return response.untyped(notFound());
    const { expires_at } = await request.json();
    const bad = checkToken({ kind: t.kind, expires_at });
    if (bad) return response.untyped(bad);
    const value = storedToken(t.kind === "classic" ? "ghp_" : "github_pat_");
    Object.assign(t, {
      token_last_eight: lastEight(value),
      expires_at,
      created_at: now(),
      last_used_at: null,
    });
    audit(
      "personal_access_token.regenerate",
      `Regenerated token ${t.kind === "classic" ? t.note : t.name}`,
    );
    return response(200).json({ token: t, value });
  }),

  // ---- Two-factor, sessions, security log ----

  http.get("/settings/security", async ({ response }) => {
    const fault = await gate("account-security");
    if (fault) return response.untyped(fault);
    return response(200).json(security);
  }),

  http.post("/settings/security/two-factor/setup", async ({ response }) => {
    const fault = await gate("account-2fa-setup");
    if (fault) return response.untyped(fault);
    const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    pendingTwoFactor.secret = Array.from(
      crypto.getRandomValues(new Uint8Array(16)),
      (b) => alphabet[b % 32],
    ).join("");
    return response(200).json({
      secret: pendingTwoFactor.secret,
      otpauth_uri: `otpauth://totp/Graftyard:${me.login}?secret=${pendingTwoFactor.secret}&issuer=Graftyard`,
    });
  }),

  http.post("/settings/security/two-factor", async ({ request, response }) => {
    const fault = await gate("account-2fa-enable");
    if (fault) return response.untyped(fault);
    const { code } = await request.json();
    if (!pendingTwoFactor.secret)
      return response.untyped(invalid("TwoFactor", "code", "invalid", "Start setup again"));
    if (!otpOk(code))
      return response.untyped(
        invalid(
          "TwoFactor",
          "code",
          "invalid",
          "Two-factor code verification failed. Try the next code.",
        ),
      );
    pendingTwoFactor.secret = "";
    security.two_factor = {
      enabled: true,
      method: "app",
      enabled_at: now(),
      recovery_codes_generated_at: now(),
    };
    me.two_factor_authentication = true;
    setRecoveryCodes(makeRecoveryCodes());
    audit("user.two_factor_enable", "Enabled two-factor authentication with an app");
    return response(200).json({ security, recovery_codes: recoveryCodes });
  }),

  http.delete("/settings/security/two-factor", async ({ response }) => {
    const fault = await gate("account-2fa-disable");
    if (fault) return response.untyped(fault);
    security.two_factor = {
      enabled: false,
      method: null,
      enabled_at: null,
      recovery_codes_generated_at: null,
    };
    me.two_factor_authentication = false;
    setRecoveryCodes([]);
    audit("user.two_factor_disable", "Disabled two-factor authentication");
    return response(200).json(security);
  }),

  http.post("/settings/security/recovery-codes", async ({ response }) => {
    const fault = await gate("account-recovery-codes");
    if (fault) return response.untyped(fault);
    if (!security.two_factor.enabled) {
      return response.untyped(
        invalid("TwoFactor", "recovery_codes", "invalid", "Enable two-factor authentication first"),
      );
    }
    setRecoveryCodes(makeRecoveryCodes());
    security.two_factor.recovery_codes_generated_at = now();
    audit("user.two_factor_recovery_codes_regenerated", "Generated new recovery codes");
    return response(200).json({ recovery_codes: recoveryCodes });
  }),

  http.get("/settings/sessions", async ({ response }) => {
    const fault = await gate("account-sessions");
    if (fault) return response.untyped(fault);
    return response(200).json(db.sessions);
  }),

  http.delete("/settings/sessions/{id}", async ({ params, response }) => {
    const fault = await gate("account-session-revoke");
    if (fault) return response.untyped(fault);
    const i = db.sessions.findIndex((s) => s.id === Number(params.id));
    if (i < 0)
      return response.untyped(
        invalid("Session", "id", "missing", "That session has already ended"),
      );
    if (db.sessions[i].current) {
      return response.untyped(
        invalid(
          "Session",
          "id",
          "invalid",
          "You cannot revoke the session you are using. Sign out instead.",
        ),
      );
    }
    const [s] = db.sessions.splice(i, 1);
    audit("user.session_revoke", `Revoked the session on ${s.device}`);
    return response(204).empty();
  }),

  http.get("/settings/security-log", async ({ query, response }) => {
    const fault = await gate("account-security-log");
    if (fault) return response.untyped(fault);
    const q = (query.get("q") ?? "").trim().toLowerCase();
    const action = q.match(/action:(\S+)/)?.[1];
    const text = q.replace(/action:\S+/, "").trim();
    const list = db.securityLog.filter(
      (e) =>
        (!action || e.action.startsWith(action)) &&
        (!text || `${e.action} ${e.detail} ${e.location}`.toLowerCase().includes(text)),
    );
    return response(200).json(list);
  }),

  // ---- Applications ----

  http.get("/settings/applications/apps", async ({ response }) => {
    const fault = await gate("account-authorized-apps");
    if (fault) return response.untyped(fault);
    return response(200).json(db.authorizedApps);
  }),

  http.delete("/settings/applications/apps/{slug}", async ({ params, response }) => {
    const fault = await gate("account-authorized-app-revoke");
    if (fault) return response.untyped(fault);
    const i = db.authorizedApps.findIndex((a) => a.slug === params.slug);
    if (i < 0) return response.untyped(notFound());
    const [a] = db.authorizedApps.splice(i, 1);
    audit("oauth_authorization.destroy", `Revoked ${a.name}`);
    return response(204).empty();
  }),

  // ---- Developer settings: OAuth apps ----

  http.get("/settings/oauth-apps", async ({ response }) => {
    const fault = await gate("account-oauth-apps");
    if (fault) return response.untyped(fault);
    return response(200).json(
      db.oauthApps.filter((a) => a.owner.login === me.login && !a.first_party),
    );
  }),

  http.post("/settings/oauth-apps", async ({ request, response }) => {
    const fault = await gate("account-oauth-app-create");
    if (fault) return response.untyped(fault);
    const body = await request.json();
    const bad = checkOAuthApp(body);
    if (bad) return response.untyped(bad);
    const created: OAuthClient = {
      ...body,
      name: body.name.trim(),
      client_id: `Ov23li${hexSecret(14)}`,
      owner: db.appOwner(users.mira),
      users: 0,
      secrets: [],
      created_at: now(),
      updated_at: now(),
    };
    db.oauthApps.push(created);
    audit("oauth_application.create", `Registered OAuth app ${created.name}`);
    return response(201).json(created);
  }),

  http.get("/settings/oauth-apps/{client_id}", async ({ params, response }) => {
    const fault = await gate("account-oauth-app");
    if (fault) return response.untyped(fault);
    const a = db.oauthApps.find(
      (x) => x.client_id === params.client_id && x.owner.login === me.login,
    );
    return a ? response(200).json(a) : response.untyped(notFound());
  }),

  http.patch("/settings/oauth-apps/{client_id}", async ({ params, request, response }) => {
    const fault = await gate("account-oauth-app-save");
    if (fault) return response.untyped(fault);
    const a = db.oauthApps.find(
      (x) => x.client_id === params.client_id && x.owner.login === me.login,
    );
    if (!a) return response.untyped(notFound());
    const body = await request.json();
    const bad = checkOAuthApp(body, a);
    if (bad) return response.untyped(bad);
    Object.assign(a, body, { name: body.name.trim(), updated_at: now() });
    audit("oauth_application.update", `Updated OAuth app ${a.name}`);
    return response(200).json(a);
  }),

  http.delete("/settings/oauth-apps/{client_id}", async ({ params, response }) => {
    const fault = await gate("account-oauth-app-delete");
    if (fault) return response.untyped(fault);
    const i = db.oauthApps.findIndex(
      (x) => x.client_id === params.client_id && x.owner.login === me.login,
    );
    if (i < 0) return response.untyped(notFound());
    const [a] = db.oauthApps.splice(i, 1);
    db.grants.splice(
      0,
      db.grants.length,
      ...db.grants.filter((g) => g.app.client_id !== a.client_id),
    );
    audit("oauth_application.destroy", `Deleted OAuth app ${a.name}`);
    return response(204).empty();
  }),

  http.post("/settings/oauth-apps/{client_id}/secrets", async ({ params, response }) => {
    const fault = await gate("account-oauth-secret");
    if (fault) return response.untyped(fault);
    const a = db.oauthApps.find(
      (x) => x.client_id === params.client_id && x.owner.login === me.login,
    );
    if (!a) return response.untyped(notFound());
    const value = hexSecret(40);
    const secret = {
      id: id(),
      last_eight: lastEight(value),
      created_at: now(),
      last_used_at: null,
    };
    a.secrets.push(secret);
    audit("oauth_application.generate_client_secret", `New client secret for ${a.name}`);
    return response(201).json({ secret, value });
  }),

  http.delete("/settings/oauth-apps/{client_id}/secrets/{id}", async ({ params, response }) => {
    const fault = await gate("account-oauth-secret-delete");
    if (fault) return response.untyped(fault);
    const a = db.oauthApps.find(
      (x) => x.client_id === params.client_id && x.owner.login === me.login,
    );
    const i = a?.secrets.findIndex((s) => s.id === Number(params.id)) ?? -1;
    if (!a || i < 0) return response.untyped(notFound());
    a.secrets.splice(i, 1);
    audit("oauth_application.remove_client_secret", `Deleted a client secret of ${a.name}`);
    return response(204).empty();
  }),

  // ---- Developer settings: GitHub Apps ----

  http.get("/settings/apps", async ({ response }) => {
    const fault = await gate("account-apps");
    if (fault) return response.untyped(fault);
    return response(200).json(db.apps.filter((a) => a.owner.login === me.login).map(devApp));
  }),

  http.post("/settings/apps", async ({ request, response }) => {
    const fault = await gate("account-app-create");
    if (fault) return response.untyped(fault);
    const m = await request.json();
    const bad = checkManifest(m);
    if (bad) return response.untyped(bad);
    const record = db.appRecord(
      slugify(m.name),
      m.name.trim(),
      users.mira,
      { ...m, hook: { active: m.hook_attributes.active, url: m.hook_attributes.url } },
      now(),
    );
    record.owner = db.appOwner(users.mira);
    record.owner.login = me.login;
    record.webhook_secret = m.hook_attributes.secret || null;
    record.webhook.secret_set = Boolean(m.hook_attributes.secret);
    const key = await rsaKey();
    record.keys = [{ id: id(), fingerprint: key.fingerprint, created_at: now() }];
    db.apps.push(record);
    const code = hexSecret(20);
    manifestCodes.set(code, {
      slug: record.slug,
      expires_ms: Date.now() + 3_600_000,
      pem: key.pem,
    });
    audit("integration.create", `Registered GitHub App ${record.name}`);
    return response(201).json({ slug: record.slug, code });
  }),

  http.get("/settings/apps/{slug}", async ({ params, response }) => {
    const fault = await gate("account-app");
    if (fault) return response.untyped(fault);
    const a = db.apps.find((x) => x.slug === params.slug && x.owner.login === me.login);
    return a ? response(200).json(devApp(a)) : response.untyped(notFound());
  }),

  http.patch("/settings/apps/{slug}", async ({ params, request, response }) => {
    const fault = await gate("account-app-save");
    if (fault) return response.untyped(fault);
    const a = db.apps.find((x) => x.slug === params.slug && x.owner.login === me.login);
    if (!a) return response.untyped(notFound());
    const m = await request.json();
    const bad = checkManifest(m, a);
    if (bad) return response.untyped(bad);
    const { hook_attributes, ...rest } = m;
    Object.assign(a, rest, { updated_at: now() });
    if (hook_attributes) {
      a.webhook.active = hook_attributes.active;
      a.webhook.url = hook_attributes.url;
      if (hook_attributes.secret) {
        a.webhook_secret = hook_attributes.secret;
        a.webhook.secret_set = true;
      }
    }
    for (const inst of db.installations.filter((i) => i.app_id === a.id)) {
      inst.permissions = a.permissions;
      inst.events = a.events;
    }
    audit("integration.update", `Updated GitHub App ${a.name}`);
    return response(200).json(devApp(a));
  }),

  http.delete("/settings/apps/{slug}", async ({ params, response }) => {
    const fault = await gate("account-app-delete");
    if (fault) return response.untyped(fault);
    const i = db.apps.findIndex((x) => x.slug === params.slug && x.owner.login === me.login);
    if (i < 0) return response.untyped(notFound());
    const [a] = db.apps.splice(i, 1);
    db.installations.splice(
      0,
      db.installations.length,
      ...db.installations.filter((x) => x.app_id !== a.id),
    );
    audit("integration.destroy", `Deleted GitHub App ${a.name}`);
    return response(204).empty();
  }),

  http.post("/settings/apps/{slug}/keys", async ({ params, response }) => {
    const fault = await gate("account-app-key");
    if (fault) return response.untyped(fault);
    const a = db.apps.find((x) => x.slug === params.slug && x.owner.login === me.login);
    if (!a) return response.untyped(notFound());
    const { pem, fingerprint } = await rsaKey();
    const key = { id: id(), fingerprint, created_at: now() };
    a.keys.push(key);
    audit("integration.generate_private_key", `New private key for ${a.name}`);
    return response(201).json({ key, pem });
  }),

  http.delete("/settings/apps/{slug}/keys/{id}", async ({ params, response }) => {
    const fault = await gate("account-app-key-delete");
    if (fault) return response.untyped(fault);
    const a = db.apps.find((x) => x.slug === params.slug && x.owner.login === me.login);
    const i = a?.keys.findIndex((k) => k.id === Number(params.id)) ?? -1;
    if (!a || i < 0) return response.untyped(notFound());
    if (a.keys.length === 1) {
      return response.untyped(
        invalid(
          "IntegrationKey",
          "id",
          "invalid",
          "An App needs at least one private key. Generate another first.",
        ),
      );
    }
    a.keys.splice(i, 1);
    audit("integration.remove_private_key", `Deleted a private key of ${a.name}`);
    return response(204).empty();
  }),

  // ---- Installing an App ----

  http.post("/settings/installations", async ({ request, response }) => {
    const fault = await gate("account-install");
    if (fault) return response.untyped(fault);
    const body = await request.json();
    const app = db.apps.find((a) => a.slug === body.app_slug);
    if (!app) return response.untyped(notFound());
    const target =
      body.account === me.login ? users.mira : body.account === "graftyard" ? users.org : undefined;
    if (!target)
      return response.untyped(
        invalid("Installation", "account", "invalid", "You cannot install on that account"),
      );
    if (!app.public && app.owner.login !== body.account) {
      return response.untyped(
        invalid(
          "Installation",
          "account",
          "invalid",
          `${app.name} can only be installed on ${app.owner.login}`,
        ),
      );
    }
    if (db.installations.some((i) => i.app_id === app.id && i.target_id === target.id)) {
      return response.untyped(
        invalid(
          "Installation",
          "account",
          "already_exists",
          `${app.name} is already installed on ${target.login}`,
        ),
      );
    }
    if (body.repository_selection === "selected" && body.repository_ids.length === 0) {
      return response.untyped(
        invalid("Installation", "repositories", "missing_field", "Select at least one repository"),
      );
    }
    const inst = addInstallation(app, target, body.repository_selection, body.repository_ids);
    audit("integration_installation.create", `Installed ${app.name} on ${target.login}`);
    return response(201).json(wireInstallation(inst));
  }),

  http.patch("/settings/installations/{id}", async ({ params, request, response }) => {
    const fault = await gate("account-installation-save");
    if (fault) return response.untyped(fault);
    const inst = db.installations.find((i) => i.id === Number(params.id));
    if (!inst) return response.untyped(notFound());
    const { repository_selection } = await request.json();
    inst.repository_selection = repository_selection;
    inst.updated_at = now();
    return response(200).json(wireInstallation(inst));
  }),

  http.delete("/settings/installations/{id}", async ({ params, response }) => {
    const fault = await gate("account-uninstall");
    if (fault) return response.untyped(fault);
    const i = db.installations.findIndex((x) => x.id === Number(params.id));
    if (i < 0) return response.untyped(notFound());
    const [inst] = db.installations.splice(i, 1);
    const app = db.apps.find((a) => a.id === inst.app_id);
    if (app) app.installations_count--;
    audit("integration_installation.destroy", `Uninstalled ${app?.name ?? inst.app_slug}`);
    return response(204).empty();
  }),

  // ---- Notifications ----

  http.get("/settings/notifications", async ({ response }) => {
    const fault = await gate("account-notifications");
    if (fault) return response.untyped(fault);
    return response(200).json(db.notifications);
  }),

  http.put("/settings/notifications", async ({ request, response }) => {
    const fault = await gate("account-notifications-save");
    if (fault) return response.untyped(fault);
    const body = await request.json();
    const verified = new Set(db.emails.filter((e) => e.verified).map((e) => e.email));
    if (!verified.has(body.default_email)) {
      return response.untyped(
        invalid(
          "NotificationSettings",
          "default_email",
          "unverified",
          "Notification email must be a verified address",
        ),
      );
    }
    const route = body.routes.find((r) => !verified.has(r.email));
    if (route) {
      return response.untyped(
        invalid(
          "NotificationSettings",
          "routes",
          "unverified",
          `${route.email} is not a verified address`,
        ),
      );
    }
    Object.assign(db.notifications, body);
    return response(200).json(db.notifications);
  }),

  // ---- Sign-in ----

  http.post("/login/session", async ({ request, response }) => {
    const fault = await gate("login");
    if (fault) return response.untyped(fault);
    const { login, password } = await request.json();
    const known = [
      me.login,
      ...Object.values(users)
        .filter((u) => u.type === "User")
        .map((u) => u.login),
    ];
    const who =
      known.find((l) => l.toLowerCase() === login.trim().toLowerCase()) ??
      (db.emails.some((e) => e.email === login.trim().toLowerCase()) ? me.login : undefined);
    if (!who || password === "wrong" || !password) {
      audit("user.failed_login", `Failed sign-in as ${login.trim()}`);
      return response.untyped(failed(401, "Incorrect username or password."));
    }
    const attempt = hexSecret(16);
    loginAttempts.set(attempt, who);
    const twoFactor = who !== me.login || security.two_factor.enabled;
    if (!twoFactor) audit("user.login", "Signed in with a password");
    return response(200).json({ login: who, two_factor: twoFactor, attempt });
  }),

  http.post("/login/two-factor", async ({ request, response }) => {
    const fault = await gate("login-2fa");
    if (fault) return response.untyped(fault);
    const { attempt, code } = await request.json();
    const who = loginAttempts.get(attempt);
    if (!who)
      return response.untyped(
        invalid("Session", "attempt", "expired", "Your sign-in timed out. Start again."),
      );
    const recovery = recoveryCodes.indexOf(code.trim().toLowerCase());
    if (recovery >= 0 && who === me.login) {
      setRecoveryCodes(recoveryCodes.filter((_, i) => i !== recovery));
      audit("user.login", "Signed in with a recovery code");
    } else if (!otpOk(code)) {
      return response.untyped(
        invalid("Session", "code", "invalid", "Two-factor authentication failed."),
      );
    } else if (who === me.login) {
      audit("user.login", "Signed in with a password and an authenticator app");
    }
    loginAttempts.delete(attempt);
    return response(200).json({ login: who });
  }),

  // ---- Device flow ----

  http.post("/login/device/codes", async ({ request, response }) => {
    const fault = await gate("oauth-device-code");
    if (fault) return response.untyped(fault);
    const { client_id, scope } = await request.json();
    const client = db.oauthApps.find((a) => a.client_id === client_id && a.device_flow);
    if (!client) return response.untyped(failed(404, "Not Found"));
    const row = {
      user_code: userCode(),
      client_id,
      scopes: scope.split(/[\s,]+/).filter(Boolean),
      created_ms: Date.now(),
      state: "pending" as const,
    };
    deviceCodes.unshift(row);
    return response(201).json(deviceView(row));
  }),

  http.post("/login/device", async ({ request, response }) => {
    const fault = await gate("oauth-device");
    if (fault) return response.untyped(fault);
    const { user_code } = await request.json();
    const found = findDeviceCode(user_code);
    if ("error" in found) return response.untyped(found.error!);
    return response(200).json(deviceView(found.row));
  }),

  http.post("/login/device/authorize", async ({ request, response }) => {
    const fault = await gate("oauth-device-authorize");
    if (fault) return response.untyped(fault);
    const { user_code, decision } = await request.json();
    const found = findDeviceCode(user_code);
    if ("error" in found) return response.untyped(found.error!);
    // Spent before anything is granted, so one code never yields two tokens.
    found.row.state = decision === "authorize" ? "authorized" : "denied";
    const client = db.oauthApps.find((a) => a.client_id === found.row.client_id);
    if (decision === "authorize" && client) {
      grantFor(client, found.row.scopes);
      audit(
        "oauth_authorization.create",
        `Authorized ${client.name} by device code (${found.row.scopes.join(", ")})`,
      );
    }
    return response(200).json({
      status: found.row.state === "authorized" ? "authorized" : "denied",
    });
  }),

  // ---- Authorization-code consent ----

  http.get("/oauth/authorize", async ({ query, response }) => {
    const fault = await gate("oauth-authorize");
    if (fault) return response.untyped(fault);
    const clientId = query.get("client_id") ?? "";
    const client = findClient(clientId);
    if (!client)
      return response.untyped(
        failed(404, "This application is not registered, or its client_id is wrong."),
      );
    const registered =
      client.kind === "oauth_app" ? [client.oauth.callback_url] : client.app.callback_urls;
    const redirect = query.get("redirect_uri") || registered[0] || "";
    if (!registered.some((r) => redirectAllowed(r, redirect))) {
      return response.untyped(
        invalid(
          "Authorization",
          "redirect_uri",
          "redirect_uri_mismatch",
          "The redirect_uri is not associated with this application.",
        ),
      );
    }
    const scopes = (query.get("scope") ?? "").split(/[\s,]+/).filter(Boolean);
    const unknown = scopes.find((s) => !SCOPE_NAMES.has(s));
    if (client.kind === "oauth_app" && unknown) {
      return response.untyped(
        invalid("Authorization", "scope", "invalid_scope", `Unknown scope ${unknown}`),
      );
    }
    const host = new URL(redirect).host;
    const meta = client.kind === "oauth_app" ? client.oauth : client.app;
    const already =
      client.kind === "oauth_app"
        ? db.grants.some(
            (g) => g.app.client_id === clientId && scopes.every((s) => g.scopes.includes(s)),
          )
        : db.authorizedApps.some((a) => a.slug === client.app.slug);
    return response(200).json({
      app: {
        name: meta.name,
        owner: meta.owner.login,
        client_id: clientId,
        homepage_url: meta.homepage_url,
        kind: client.kind,
      },
      scopes: client.kind === "oauth_app" ? scopes : [],
      permissions: client.kind === "github_app" ? client.app.permissions : null,
      redirect_uri: redirect,
      redirect_host: host,
      localhost: /^(localhost|127\.0\.0\.1)(:\d+)?$/.test(host),
      already_authorized: already,
      code_expires_in: AUTH_CODE_TTL_S,
    });
  }),

  http.post("/oauth/authorize", async ({ request, response }) => {
    const fault = await gate("oauth-authorize-submit");
    if (fault) return response.untyped(fault);
    const body = await request.json();
    const client = findClient(body.client_id);
    if (!client)
      return response.untyped(invalid("Authorization", "client_id", "invalid", "Unknown client"));
    const registered =
      client.kind === "oauth_app" ? [client.oauth.callback_url] : client.app.callback_urls;
    const redirect = body.redirect_uri || registered[0] || "";
    if (!registered.some((r) => redirectAllowed(r, redirect))) {
      return response.untyped(
        invalid(
          "Authorization",
          "redirect_uri",
          "redirect_uri_mismatch",
          "The redirect_uri is not associated with this application.",
        ),
      );
    }
    const target = new URL(redirect);
    if (body.state) target.searchParams.set("state", body.state);
    if (body.decision === "deny") {
      target.searchParams.set("error", "access_denied");
      target.searchParams.set("error_description", "The user has denied your application access.");
      return response(200).json({ redirect_to: target.toString(), expires_at: null });
    }
    const code = hexSecret(20);
    const expires = Date.now() + AUTH_CODE_TTL_S * 1000;
    authCodes.set(code, { client_id: body.client_id, expires_ms: expires });
    target.searchParams.set("code", code);
    const scopes = (body.scope ?? "").split(/[\s,]+/).filter(Boolean);
    if (client.kind === "oauth_app") grantFor(client.oauth, scopes);
    else authorizeApp(client.app);
    const name = client.kind === "oauth_app" ? client.oauth.name : client.app.name;
    audit(
      "oauth_authorization.create",
      `Authorized ${name}${scopes.length ? ` (${scopes.join(", ")})` : ""}`,
    );
    return response(200).json({
      redirect_to: target.toString(),
      expires_at: new Date(expires).toISOString(),
    });
  }),
];
