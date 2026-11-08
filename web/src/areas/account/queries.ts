import { queryOptions } from "@tanstack/react-query";
import { api, ApiError } from "@/api/client";
import { ui, type ConsentQuery } from "./ui-api";

// One factory per page region. REST keys start with the REST path segments; the private
// web API's keys start with "_t", "ui".

type FieldError = { field?: string; message?: string; code: string };

// A 422 keeps its field errors, so a form can show each under its field. Its message is
// the first error's text, which is what a toast shows.
export class ValidationError extends ApiError {
  readonly errors: FieldError[];
  constructor(body: { message?: string; errors?: FieldError[] }) {
    const errors = body.errors ?? [];
    super(422, {
      message: errors.find((e) => e.message)?.message ?? body.message ?? "Validation Failed",
    });
    this.errors = errors;
  }
}

type Result<T> = { data?: T; error?: unknown; response: Response };

export async function call<T>(request: Promise<Result<T>>): Promise<T> {
  const { data, error, response } = await request;
  if (response.status === 422 && error && typeof error === "object") {
    throw new ValidationError(error as { message?: string; errors?: FieldError[] });
  }
  if (!response.ok || error !== undefined) throw new ApiError(response.status, error);
  return data as T;
}

export const emailsQuery = () =>
  queryOptions({
    queryKey: ["user", "emails"],
    queryFn: ({ signal }) =>
      call(api.GET("/user/emails", { params: { query: { per_page: 100 } }, signal })),
  });

export const emailVisibilityQuery = () =>
  queryOptions({
    queryKey: ["_t", "ui", "settings", "emails", "visibility"],
    queryFn: ({ signal }) => call(ui.GET("/settings/emails/visibility", { signal })),
  });

export const sshKeysQuery = () =>
  queryOptions({
    queryKey: ["user", "keys"],
    queryFn: ({ signal }) =>
      call(api.GET("/user/keys", { params: { query: { per_page: 100 } }, signal })),
  });

export const signingKeysQuery = () =>
  queryOptions({
    queryKey: ["user", "ssh_signing_keys"],
    queryFn: ({ signal }) =>
      call(api.GET("/user/ssh_signing_keys", { params: { query: { per_page: 100 } }, signal })),
  });

export const gpgKeysQuery = () =>
  queryOptions({
    queryKey: ["user", "gpg_keys"],
    queryFn: ({ signal }) =>
      call(api.GET("/user/gpg_keys", { params: { query: { per_page: 100 } }, signal })),
  });

export const tokensQuery = () =>
  queryOptions({
    queryKey: ["_t", "ui", "settings", "tokens"],
    queryFn: ({ signal }) => call(ui.GET("/settings/tokens", { signal })),
  });

export const securityQuery = () =>
  queryOptions({
    queryKey: ["_t", "ui", "settings", "security"],
    queryFn: ({ signal }) => call(ui.GET("/settings/security", { signal })),
  });

export const sessionsQuery = () =>
  queryOptions({
    queryKey: ["_t", "ui", "settings", "sessions"],
    queryFn: ({ signal }) => call(ui.GET("/settings/sessions", { signal })),
  });

export const securityLogQuery = (q: string) =>
  queryOptions({
    queryKey: ["_t", "ui", "settings", "security-log", q],
    queryFn: ({ signal }) =>
      call(ui.GET("/settings/security-log", { params: { query: { q } }, signal })),
  });

export const grantsQuery = () =>
  queryOptions({
    queryKey: ["applications", "grants"],
    queryFn: ({ signal }) =>
      call(api.GET("/applications/grants", { params: { query: { per_page: 100 } }, signal })),
  });

export const authorizedAppsQuery = () =>
  queryOptions({
    queryKey: ["_t", "ui", "settings", "applications", "apps"],
    queryFn: ({ signal }) => call(ui.GET("/settings/applications/apps", { signal })),
  });

export const installationsQuery = () =>
  queryOptions({
    queryKey: ["user", "installations"],
    queryFn: ({ signal }) =>
      call(api.GET("/user/installations", { params: { query: { per_page: 100 } }, signal })),
  });

export const installationReposQuery = (id: number) =>
  queryOptions({
    queryKey: ["user", "installations", id, "repositories"],
    queryFn: ({ signal }) =>
      call(
        api.GET("/user/installations/{installation_id}/repositories", {
          params: { path: { installation_id: id }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const oauthAppsQuery = () =>
  queryOptions({
    queryKey: ["_t", "ui", "settings", "oauth-apps"],
    queryFn: ({ signal }) => call(ui.GET("/settings/oauth-apps", { signal })),
  });

export const oauthAppQuery = (clientId: string) =>
  queryOptions({
    queryKey: ["_t", "ui", "settings", "oauth-apps", clientId],
    queryFn: ({ signal }) =>
      call(
        ui.GET("/settings/oauth-apps/{client_id}", {
          params: { path: { client_id: clientId } },
          signal,
        }),
      ),
  });

export const devAppsQuery = () =>
  queryOptions({
    queryKey: ["_t", "ui", "settings", "apps"],
    queryFn: ({ signal }) => call(ui.GET("/settings/apps", { signal })),
  });

export const devAppQuery = (slug: string) =>
  queryOptions({
    queryKey: ["_t", "ui", "settings", "apps", slug],
    queryFn: ({ signal }) =>
      call(ui.GET("/settings/apps/{slug}", { params: { path: { slug } }, signal })),
  });

export const appQuery = (slug: string) =>
  queryOptions({
    queryKey: ["apps", slug],
    queryFn: ({ signal }) =>
      call(api.GET("/apps/{app_slug}", { params: { path: { app_slug: slug } }, signal })),
  });

// The App's own routes authenticate as the App: a JWT whose `iss` is the App id, signed
// with its private key. The page has no private key, and the mock reads only `iss`.
export function appJwt(appId: number) {
  const enc = (o: object) =>
    btoa(JSON.stringify(o)).replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");
  const iat = Math.floor(Date.now() / 1000) - 60;
  return `${enc({ alg: "RS256", typ: "JWT" })}.${enc({ iat, exp: iat + 600, iss: String(appId) })}.unsigned`;
}
const asApp = (appId: number) => ({ Authorization: `Bearer ${appJwt(appId)}` });

export const appInstallationsQuery = (appId: number) =>
  queryOptions({
    queryKey: ["app", appId, "installations"],
    queryFn: ({ signal }) =>
      call(
        api.GET("/app/installations", {
          params: { query: { per_page: 100 } },
          headers: asApp(appId),
          signal,
        }),
      ),
  });

export const appHookConfigQuery = (appId: number) =>
  queryOptions({
    queryKey: ["app", appId, "hook", "config"],
    queryFn: ({ signal }) => call(api.GET("/app/hook/config", { headers: asApp(appId), signal })),
  });

export const appDeliveriesQuery = (appId: number) =>
  queryOptions({
    queryKey: ["app", appId, "hook", "deliveries"],
    queryFn: ({ signal }) =>
      call(
        api.GET("/app/hook/deliveries", {
          params: { query: { per_page: 50 } },
          headers: asApp(appId),
          signal,
        }),
      ),
  });

export const appDeliveryQuery = (appId: number, id: number) =>
  queryOptions({
    queryKey: ["app", appId, "hook", "deliveries", id],
    queryFn: ({ signal }) =>
      call(
        api.GET("/app/hook/deliveries/{delivery_id}", {
          params: { path: { delivery_id: id } },
          headers: asApp(appId),
          signal,
        }),
      ),
  });

export { asApp };

export const notificationSettingsQuery = () =>
  queryOptions({
    queryKey: ["_t", "ui", "settings", "notifications"],
    queryFn: ({ signal }) => call(ui.GET("/settings/notifications", { signal })),
  });

export const consentQuery = (q: ConsentQuery) =>
  queryOptions({
    queryKey: ["_t", "ui", "oauth", "authorize", q],
    // An unknown client or a foreign redirect_uri is an answer, not a load failure: the
    // page refuses the request instead of offering a retry that cannot succeed.
    queryFn: ({ signal }) =>
      call(ui.GET("/oauth/authorize", { params: { query: q }, signal })).then(
        (consent) => ({ ok: true as const, consent }),
        (e: unknown) => {
          if (e instanceof ApiError && (e.status === 404 || e.status === 422))
            return { ok: false as const, message: e.message };
          throw e;
        },
      ),
    staleTime: 0,
  });
