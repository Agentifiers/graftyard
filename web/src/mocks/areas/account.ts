import { HttpResponse } from "msw";
import { gate, http, notFound, page, unprocessable } from "../http";
import { id, me, users } from "../world";
import { db as core } from "./core-db";
import {
  addGpgKey,
  audit,
  db,
  hexSecret,
  installationRepos,
  integrationOf,
  manifestCodes,
  normalizeKey,
  sshKeyProblem,
  storedToken,
  wireInstallation,
  type AppRecord,
} from "./account-db";

// The GHES 3.22 REST routes the account pages use: the profile write, emails, SSH, GPG,
// and signing keys, the viewer's installations, OAuth grants, and the App's own routes.
// GET /user stays core's: PATCH /user mutates the shared `me` it serves.

const DOCS = "https://docs.github.com/enterprise-server@3.22/rest";

// Validation failures with GHES's message text in `message`, which the pages show under
// the field the error names.
const invalid = (resource: string, field: string, code: string, message: string) =>
  HttpResponse.json(
    {
      message: "Validation Failed",
      documentation_url: DOCS,
      status: "422",
      errors: [{ resource, field, code, message }],
    },
    { status: 422 },
  );

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

const asList = (body: { emails: string[] } | string[] | string) =>
  typeof body === "string" ? [body] : Array.isArray(body) ? body : body.emails;

// The App routes authenticate as an App with a JWT whose `iss` is its id
// (https://docs.github.com/enterprise-server@3.22/apps). The settings pages mint one for
// the App they show; the mock reads `iss` and does not check the signature.
function appFromJwt(request: Request): AppRecord | undefined {
  const auth = request.headers.get("authorization") ?? "";
  const jwt = auth.replace(/^Bearer\s+/i, "");
  try {
    const claims = JSON.parse(atob(jwt.split(".")[1].replace(/-/g, "+").replace(/_/g, "/")));
    return db.apps.find((a) => String(a.id) === String(claims.iss) || a.client_id === claims.iss);
  } catch {
    return undefined;
  }
}
const unauthorized = () =>
  HttpResponse.json(
    { message: "A JSON web token could not be decoded", documentation_url: DOCS, status: "401" },
    { status: 401 },
  );

export const handlers = [
  // ---- Profile ----

  http.patch("/user", async ({ request, response }) => {
    const fault = await gate("account-profile-save");
    if (fault) return response.untyped(fault);
    const body = (await request.json()) ?? {};
    if (body.name !== undefined && body.name.length > 255) {
      return response.untyped(
        invalid("User", "name", "too_long", "Name is too long (maximum is 255 characters)"),
      );
    }
    if (body.bio !== undefined && body.bio.length > 160) {
      return response.untyped(
        invalid("User", "bio", "too_long", "Bio is too long (maximum is 160 characters)"),
      );
    }
    if (body.blog && !/^https?:\/\/\S+\.\S+/.test(body.blog)) {
      return response.untyped(
        invalid(
          "User",
          "blog",
          "invalid",
          "URL must be a full address, such as https://example.com",
        ),
      );
    }
    if (body.email) {
      const mine = db.emails.find((e) => e.email === body.email);
      if (!mine)
        return response.untyped(
          invalid("User", "email", "invalid", "Public email must be one of your addresses"),
        );
      if (!mine.verified) {
        return response.untyped(
          invalid("User", "email", "unverified", "Public email must be a verified address"),
        );
      }
    }
    for (const k of [
      "name",
      "blog",
      "company",
      "location",
      "bio",
      "twitter_username",
      "hireable",
    ] as const) {
      if (body[k] !== undefined) Object.assign(me, { [k]: body[k] === "" ? null : body[k] });
    }
    if (body.email !== undefined) {
      me.email = body.email || null;
      for (const e of db.emails) e.visibility = e.email === body.email ? "public" : null;
    }
    me.updated_at = new Date().toISOString();
    audit("user.update", "Updated profile");
    return response(200).json(me);
  }),

  // ---- Emails ----

  http.get("/user/emails", async ({ query, response }) => {
    const fault = await gate("account-emails");
    if (fault) return response.untyped(fault);
    return response(200).json(page(db.emails, query));
  }),

  http.get("/user/public_emails", async ({ query, response }) => {
    const fault = await gate("account-emails");
    if (fault) return response.untyped(fault);
    return response(200).json(
      page(
        db.emails.filter((e) => e.visibility === "public"),
        query,
      ),
    );
  }),

  http.post("/user/emails", async ({ request, response }) => {
    const fault = await gate("account-email-add");
    if (fault) return response.untyped(fault);
    const added = asList((await request.json()) ?? []).map((e) => e.trim().toLowerCase());
    for (const email of added) {
      if (!EMAIL.test(email))
        return response.untyped(invalid("UserEmail", "email", "invalid", "Email is invalid"));
      if (db.emails.some((e) => e.email === email)) {
        return response.untyped(
          invalid("UserEmail", "email", "already_exists", "Email is already on your account"),
        );
      }
      if (db.otherEmails.has(email)) {
        return response.untyped(
          invalid("UserEmail", "email", "already_exists", "Email is already in use"),
        );
      }
    }
    const created = added.map((email) => ({
      email,
      primary: false,
      verified: false,
      visibility: null,
    }));
    db.emails.push(...created);
    audit("user.add_email", `Added ${added.join(", ")}`);
    // The mock "clicks" the verification link a few seconds later, so the badge changes.
    setTimeout(() => created.forEach((e) => (e.verified = true)), 8000);
    return response(201).json(created);
  }),

  http.delete("/user/emails", async ({ request, response }) => {
    const fault = await gate("account-email-delete");
    if (fault) return response.untyped(fault);
    const removed = asList((await request.json()) ?? []);
    for (const email of removed) {
      const e = db.emails.find((x) => x.email === email);
      if (!e) return response.untyped(notFound());
      if (e.primary) {
        return response.untyped(
          invalid("UserEmail", "email", "invalid", "You cannot delete your primary email address"),
        );
      }
    }
    for (const email of removed)
      db.emails.splice(
        db.emails.findIndex((e) => e.email === email),
        1,
      );
    if (me.email && removed.includes(me.email)) me.email = null;
    audit("user.remove_email", `Removed ${removed.join(", ")}`);
    return response(204).empty();
  }),

  // ---- SSH authentication keys ----

  http.get("/user/keys", async ({ query, response }) => {
    const fault = await gate("account-keys");
    if (fault) return response.untyped(fault);
    return response(200).json(page(db.sshKeys, query));
  }),

  http.get("/user/keys/{key_id}", async ({ params, response }) => {
    const fault = await gate("account-keys");
    if (fault) return response.untyped(fault);
    const k = db.sshKeys.find((x) => x.id === Number(params.key_id));
    return k ? response(200).json(k) : response.untyped(notFound());
  }),

  http.post("/user/keys", async ({ request, response }) => {
    const fault = await gate("account-key-add");
    if (fault) return response.untyped(fault);
    const { title, key } = await request.json();
    if (!title?.trim())
      return response.untyped(
        invalid("PublicKey", "title", "missing_field", "Title can't be blank"),
      );
    const problem = sshKeyProblem(key, "authentication");
    if (problem) return response.untyped(invalid("PublicKey", "key", "custom", problem));
    const created = db.key(title.trim(), normalizeKey(key), new Date().toISOString(), null);
    db.sshKeys.push(created);
    audit("public_key.create", `Added SSH key ${created.title}`);
    return response(201).json(created);
  }),

  http.delete("/user/keys/{key_id}", async ({ params, response }) => {
    const fault = await gate("account-key-delete");
    if (fault) return response.untyped(fault);
    const i = db.sshKeys.findIndex((k) => k.id === Number(params.key_id));
    if (i < 0) return response.untyped(notFound());
    const [k] = db.sshKeys.splice(i, 1);
    audit("public_key.delete", `Deleted SSH key ${k.title}`);
    return response(204).empty();
  }),

  // ---- SSH signing keys ----

  http.get("/user/ssh_signing_keys", async ({ query, response }) => {
    const fault = await gate("account-keys");
    if (fault) return response.untyped(fault);
    return response(200).json(page(db.signingKeys, query));
  }),

  http.get("/user/ssh_signing_keys/{ssh_signing_key_id}", async ({ params, response }) => {
    const fault = await gate("account-keys");
    if (fault) return response.untyped(fault);
    const k = db.signingKeys.find((x) => x.id === Number(params.ssh_signing_key_id));
    return k ? response(200).json(k) : response.untyped(notFound());
  }),

  http.post("/user/ssh_signing_keys", async ({ request, response }) => {
    const fault = await gate("account-key-add");
    if (fault) return response.untyped(fault);
    const { title, key } = await request.json();
    if (!title?.trim())
      return response.untyped(
        invalid("SshSigningKey", "title", "missing_field", "Title can't be blank"),
      );
    const problem = sshKeyProblem(key, "signing");
    if (problem) return response.untyped(invalid("SshSigningKey", "key", "custom", problem));
    const created = {
      id: id(),
      key: normalizeKey(key),
      title: title.trim(),
      created_at: new Date().toISOString(),
    };
    db.signingKeys.push(created);
    audit("ssh_signing_key.create", `Added SSH signing key ${created.title}`);
    return response(201).json(created);
  }),

  http.delete("/user/ssh_signing_keys/{ssh_signing_key_id}", async ({ params, response }) => {
    const fault = await gate("account-key-delete");
    if (fault) return response.untyped(fault);
    const i = db.signingKeys.findIndex((k) => k.id === Number(params.ssh_signing_key_id));
    if (i < 0) return response.untyped(notFound());
    const [k] = db.signingKeys.splice(i, 1);
    audit("ssh_signing_key.delete", `Deleted SSH signing key ${k.title}`);
    return response(204).empty();
  }),

  // ---- GPG keys ----

  http.get("/user/gpg_keys", async ({ query, response }) => {
    const fault = await gate("account-gpg-keys");
    if (fault) return response.untyped(fault);
    return response(200).json(page(db.gpgKeys, query));
  }),

  http.get("/user/gpg_keys/{gpg_key_id}", async ({ params, response }) => {
    const fault = await gate("account-gpg-keys");
    if (fault) return response.untyped(fault);
    const k = db.gpgKeys.find((x) => x.id === Number(params.gpg_key_id));
    return k ? response(200).json(k) : response.untyped(notFound());
  }),

  http.post("/user/gpg_keys", async ({ request, response }) => {
    const fault = await gate("account-gpg-add");
    if (fault) return response.untyped(fault);
    const { name, armored_public_key } = await request.json();
    const result = await addGpgKey(name?.trim() || null, armored_public_key);
    if (typeof result === "string")
      return response.untyped(invalid("GpgKey", "key", "custom", result));
    audit("gpg_key.create", `Added GPG key ${result.key_id}`);
    return response(201).json(result);
  }),

  http.delete("/user/gpg_keys/{gpg_key_id}", async ({ params, response }) => {
    const fault = await gate("account-gpg-delete");
    if (fault) return response.untyped(fault);
    const i = db.gpgKeys.findIndex((k) => k.id === Number(params.gpg_key_id));
    if (i < 0) return response.untyped(notFound());
    const [k] = db.gpgKeys.splice(i, 1);
    db.gpgFingerprints.delete(k.id);
    audit("gpg_key.delete", `Deleted GPG key ${k.key_id}`);
    return response(204).empty();
  }),

  // ---- The viewer's installations ----

  http.get("/user/installations", async ({ query, response }) => {
    const fault = await gate("account-installations");
    if (fault) return response.untyped(fault);
    const mine = db.installations.filter(
      (i) =>
        i.account &&
        "login" in i.account &&
        (i.account.login === me.login || i.account.login === "graftyard"),
    );
    return response(200).json({
      total_count: mine.length,
      installations: page(mine, query).map(wireInstallation),
    });
  }),

  http.get(
    "/user/installations/{installation_id}/repositories",
    async ({ params, query, response }) => {
      const fault = await gate("account-installation-repos");
      if (fault) return response.untyped(fault);
      const inst = db.installations.find((i) => i.id === Number(params.installation_id));
      if (!inst) return response.untyped(notFound());
      const repos = installationRepos(inst);
      return response(200).json({
        total_count: repos.length,
        repository_selection: inst.repository_selection,
        repositories: page(repos, query),
      });
    },
  ),

  http.put(
    "/user/installations/{installation_id}/repositories/{repository_id}",
    async ({ params, response }) => {
      const fault = await gate("account-installation-repo");
      if (fault) return response.untyped(fault);
      const inst = db.installations.find((i) => i.id === Number(params.installation_id));
      const repoId = Number(params.repository_id);
      if (!inst || !core.repos().some((r) => r.id === repoId)) return response.untyped(notFound());
      if (inst.repository_selection === "selected" && !inst.repository_ids.includes(repoId)) {
        inst.repository_ids.push(repoId);
        inst.updated_at = new Date().toISOString();
      }
      return response(204).empty();
    },
  ),

  http.delete(
    "/user/installations/{installation_id}/repositories/{repository_id}",
    async ({ params, response }) => {
      const fault = await gate("account-installation-repo");
      if (fault) return response.untyped(fault);
      const inst = db.installations.find((i) => i.id === Number(params.installation_id));
      if (!inst) return response.untyped(notFound());
      if (inst.repository_selection === "all") {
        return response.untyped(
          unprocessable(
            "Installation",
            "repository_selection",
            "Change the installation to selected repositories first",
          ),
        );
      }
      if (
        inst.repository_ids.length === 1 &&
        inst.repository_ids[0] === Number(params.repository_id)
      ) {
        return response.untyped(
          invalid(
            "Installation",
            "repositories",
            "invalid",
            "An installation on selected repositories needs at least one",
          ),
        );
      }
      inst.repository_ids = inst.repository_ids.filter((id) => id !== Number(params.repository_id));
      inst.updated_at = new Date().toISOString();
      return response(204).empty();
    },
  ),

  // ---- OAuth grants ----

  http.get("/applications/grants", async ({ query, response }) => {
    const fault = await gate("account-grants");
    if (fault) return response.untyped(fault);
    const client = query.get("client_id");
    const list = client ? db.grants.filter((g) => g.app.client_id === client) : db.grants;
    return response(200).json(page(list, query));
  }),

  http.get("/applications/grants/{grant_id}", async ({ params, response }) => {
    const fault = await gate("account-grants");
    if (fault) return response.untyped(fault);
    const g = db.grants.find((x) => x.id === Number(params.grant_id));
    return g ? response(200).json(g) : response.untyped(notFound());
  }),

  http.delete("/applications/grants/{grant_id}", async ({ params, response }) => {
    const fault = await gate("account-grant-revoke");
    if (fault) return response.untyped(fault);
    const i = db.grants.findIndex((g) => g.id === Number(params.grant_id));
    if (i < 0) return response.untyped(notFound());
    const [g] = db.grants.splice(i, 1);
    audit("oauth_authorization.destroy", `Revoked ${g.app.name}`);
    return response(204).empty();
  }),

  // ---- The App's own routes, authenticated as the App ----

  http.get("/app", async ({ request, response }) => {
    const fault = await gate("account-app");
    if (fault) return response.untyped(fault);
    const app = appFromJwt(request);
    return app ? response(200).json(integrationOf(app)) : response.untyped(unauthorized());
  }),

  http.get("/apps/{app_slug}", async ({ params, response }) => {
    const fault = await gate("account-app");
    if (fault) return response.untyped(fault);
    const app = db.apps.find((a) => a.slug === params.app_slug);
    return app ? response(200).json(integrationOf(app)) : response.untyped(notFound());
  }),

  http.get("/app/installations", async ({ request, query, response }) => {
    const fault = await gate("account-app-installations");
    if (fault) return response.untyped(fault);
    const app = appFromJwt(request);
    if (!app) return response.untyped(unauthorized());
    const list = db.installations.filter((i) => i.app_id === app.id).map(wireInstallation);
    return response(200).json(page(list, query));
  }),

  http.get("/app/installations/{installation_id}", async ({ request, params, response }) => {
    const fault = await gate("account-app-installations");
    if (fault) return response.untyped(fault);
    const app = appFromJwt(request);
    if (!app) return response.untyped(unauthorized());
    const inst = db.installations.find(
      (i) => i.id === Number(params.installation_id) && i.app_id === app.id,
    );
    return inst ? response(200).json(wireInstallation(inst)) : response.untyped(notFound());
  }),

  http.delete("/app/installations/{installation_id}", async ({ request, params, response }) => {
    const fault = await gate("account-app-uninstall");
    if (fault) return response.untyped(fault);
    const app = appFromJwt(request);
    if (!app) return response.untyped(unauthorized());
    const i = db.installations.findIndex(
      (x) => x.id === Number(params.installation_id) && x.app_id === app.id,
    );
    if (i < 0) return response.untyped(notFound());
    db.installations.splice(i, 1);
    app.installations_count--;
    audit("integration_installation.destroy", `Uninstalled ${app.name}`);
    return response(204).empty();
  }),

  http.put(
    "/app/installations/{installation_id}/suspended",
    async ({ request, params, response }) => {
      const fault = await gate("account-app-suspend");
      if (fault) return response.untyped(fault);
      const app = appFromJwt(request);
      const inst = db.installations.find(
        (i) => i.id === Number(params.installation_id) && i.app_id === app?.id,
      );
      if (!inst) return response.untyped(notFound());
      inst.suspended_at = new Date().toISOString();
      inst.suspended_by = users.mira;
      return response(204).empty();
    },
  ),

  http.delete(
    "/app/installations/{installation_id}/suspended",
    async ({ request, params, response }) => {
      const fault = await gate("account-app-suspend");
      if (fault) return response.untyped(fault);
      const app = appFromJwt(request);
      const inst = db.installations.find(
        (i) => i.id === Number(params.installation_id) && i.app_id === app?.id,
      );
      if (!inst) return response.untyped(notFound());
      inst.suspended_at = null;
      inst.suspended_by = null;
      return response(204).empty();
    },
  ),

  http.post(
    "/app/installations/{installation_id}/access_tokens",
    async ({ request, params, response }) => {
      const fault = await gate("account-app-token");
      if (fault) return response.untyped(fault);
      const app = appFromJwt(request);
      const inst = db.installations.find(
        (i) => i.id === Number(params.installation_id) && i.app_id === app?.id,
      );
      if (!app || !inst) return response.untyped(notFound());
      if (inst.suspended_at) {
        return response.untyped(
          HttpResponse.json(
            {
              message: "This installation has been suspended",
              documentation_url: DOCS,
              status: "403",
            },
            { status: 403 },
          ),
        );
      }
      // A stateless installation token: "ghs_", the App id, "_", then a compact JWS
      // (`design/SHARED.md`, "Identity and tokens").
      const expires = new Date(Date.now() + 3_600_000);
      const enc = (o: object) =>
        btoa(JSON.stringify(o)).replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");
      const jws = `${enc({ alg: "EdDSA", kid: "k1" })}.${enc({ app: app.id, installation: inst.id, exp: Math.floor(expires.getTime() / 1000) })}.${storedToken("ghp_").slice(4)}`;
      return response(201).json({
        token: `ghs_${app.id}_${jws}`,
        expires_at: expires.toISOString(),
        permissions: inst.permissions,
        repository_selection: inst.repository_selection,
      });
    },
  ),

  http.get("/app/hook/config", async ({ request, response }) => {
    const fault = await gate("account-app-hook");
    if (fault) return response.untyped(fault);
    const app = appFromJwt(request);
    if (!app) return response.untyped(unauthorized());
    return response(200).json({
      url: app.webhook.url,
      content_type: "json",
      secret: app.webhook_secret ? "********" : undefined,
      insecure_ssl: "0",
    });
  }),

  http.patch("/app/hook/config", async ({ request, response }) => {
    const fault = await gate("account-app-hook-save");
    if (fault) return response.untyped(fault);
    const app = appFromJwt(request);
    if (!app) return response.untyped(unauthorized());
    const body = await request.json();
    if (body.url !== undefined && body.url !== "" && !/^https?:\/\/\S+/.test(body.url)) {
      return response.untyped(
        invalid("Hook", "url", "invalid", "URL must start with http:// or https://"),
      );
    }
    if (body.url !== undefined) app.webhook.url = body.url;
    if (body.secret) {
      app.webhook_secret = body.secret;
      app.webhook.secret_set = true;
    }
    app.updated_at = new Date().toISOString();
    return response(200).json({
      url: app.webhook.url,
      content_type: "json",
      secret: "********",
      insecure_ssl: "0",
    });
  }),

  http.get("/app/hook/deliveries", async ({ request, response }) => {
    const fault = await gate("account-app-deliveries");
    if (fault) return response.untyped(fault);
    const app = appFromJwt(request);
    if (!app) return response.untyped(unauthorized());
    return response(200).json(
      app.deliveries.map(({ request: _q, response: _r, url: _u, ...item }) => item),
    );
  }),

  http.get("/app/hook/deliveries/{delivery_id}", async ({ request, params, response }) => {
    const fault = await gate("account-app-deliveries");
    if (fault) return response.untyped(fault);
    const d = appFromJwt(request)?.deliveries.find((x) => x.id === Number(params.delivery_id));
    return d ? response(200).json(d) : response.untyped(notFound());
  }),

  http.post(
    "/app/hook/deliveries/{delivery_id}/attempts",
    async ({ request, params, response }) => {
      const fault = await gate("account-app-redeliver");
      if (fault) return response.untyped(fault);
      const app = appFromJwt(request);
      const d = app?.deliveries.find((x) => x.id === Number(params.delivery_id));
      if (!app || !d) return response.untyped(notFound());
      const again = { ...db.delivery(app, d.event, d.action, 200, 0), redelivery: true };
      app.deliveries.unshift(again);
      return response(202).json({});
    },
  ),

  // The manifest flow's last step: trade the one-time code for the App's credentials.
  http.post("/app-manifests/{code}/conversions", async ({ params, response }) => {
    const fault = await gate("account-app-create");
    if (fault) return response.untyped(fault);
    const row = manifestCodes.get(params.code);
    if (!row || row.expires_ms < Date.now()) return response.untyped(notFound());
    manifestCodes.delete(params.code);
    const app = db.apps.find((a) => a.slug === row.slug);
    if (!app) return response.untyped(notFound());
    const secret = hexSecret(40);
    app.client_secrets.push({
      id: id(),
      last_eight: secret.slice(-8),
      created_at: new Date().toISOString(),
      last_used_at: null,
    });
    return response(201).json({
      ...integrationOf(app),
      client_id: app.client_id,
      client_secret: secret,
      webhook_secret: app.webhook_secret,
      pem: row.pem,
    });
  }),
];
