import createClient from "openapi-fetch";
import { waitForMocks, type Schemas } from "@/api/client";

// The private web API (`/_t/ui`) behind the account pages, for what GHES 3.22 REST has no
// route for: personal access tokens, two-factor setup, sessions, the security log, the
// developer settings, the sign-in pages, and the OAuth consent and device-code pages
// (`decisions/OPEN-QUESTIONS.md`, F55). Hand-written in openapi-typescript's shape so the
// client and the mock (web/src/mocks/areas/account-ui.ts) check each other.

export type AppPermissions = Schemas["app-permissions"];
export type PermissionKey = keyof AppPermissions;
export type Access = NonNullable<AppPermissions[PermissionKey]>;

type Json<T> = { headers: { [name: string]: unknown }; content: { "application/json": T } };
type Empty = { headers: { [name: string]: unknown }; content?: never };
type Body<T> = { content: { "application/json": T } };
type NoParams = { query?: never; header?: never; path?: never; cookie?: never };
type PathOf<T> = { query?: never; header?: never; path: T; cookie?: never };
type QueryOf<T> = { query?: T; header?: never; path?: never; cookie?: never };
type Invalid = Json<Schemas["validation-error"]>;
type Failed = Json<Schemas["basic-error"]>;

export type Expiry = string | null; // ISO date-time, or null for "no expiration"

type TokenBase = {
  id: number;
  token_last_eight: string;
  created_at: string;
  expires_at: Expiry;
  last_used_at: string | null;
};
export type FineGrainedToken = TokenBase & {
  kind: "fine_grained";
  name: string;
  description: string;
  resource_owner: string;
  repository_selection: "public" | "all" | "selected";
  repositories: string[]; // full names, when selected
  permissions: AppPermissions;
};
export type ClassicToken = TokenBase & { kind: "classic"; note: string; scopes: string[] };
export type Token = FineGrainedToken | ClassicToken;

type Issued = Exclude<keyof TokenBase, "expires_at">;
export type NewToken = Omit<FineGrainedToken, Issued> | Omit<ClassicToken, Issued>;

// The only response that carries a token's value. The page shows it once.
export type IssuedToken = { token: Token; value: string };

export type Security = {
  two_factor: {
    enabled: boolean;
    method: "app" | null;
    enabled_at: string | null;
    recovery_codes_generated_at: string | null;
  };
  password_changed_at: string;
};

export type Session = {
  id: number;
  kind: "web" | "mobile" | "cli";
  device: string;
  location: string;
  ip: string;
  signed_in_at: string;
  last_active_at: string;
  current: boolean;
};

export type SecurityEvent = {
  id: number;
  action: string;
  actor: string;
  created_at: string;
  ip: string;
  location: string;
  detail: string;
};

export type AppOwner = { login: string; avatar_url: string };

export type AuthorizedApp = {
  slug: string;
  name: string;
  owner: AppOwner;
  authorized_at: string;
  last_used_at: string | null;
  permissions: AppPermissions;
};

export type ClientSecret = {
  id: number;
  last_eight: string;
  created_at: string;
  last_used_at: string | null;
};

export type OAuthApp = {
  client_id: string;
  name: string;
  description: string;
  homepage_url: string;
  callback_url: string;
  device_flow: boolean;
  owner: AppOwner;
  users: number;
  secrets: ClientSecret[];
  created_at: string;
  updated_at: string;
};
export type OAuthAppInput = Pick<
  OAuthApp,
  "name" | "description" | "homepage_url" | "callback_url" | "device_flow"
>;

export type AppKey = { id: number; fingerprint: string; created_at: string };

export type AppManifest = {
  name: string;
  description: string;
  homepage_url: string;
  callback_urls: string[];
  request_oauth_on_install: boolean;
  setup_url: string;
  setup_on_update: boolean;
  hook_attributes: { active: boolean; url: string; secret: string };
  permissions: AppPermissions;
  events: string[];
  public: boolean; // false: only on the owner's account
};

export type DeveloperApp = Omit<AppManifest, "hook_attributes"> & {
  id: number;
  slug: string;
  client_id: string;
  owner: AppOwner;
  webhook: { active: boolean; url: string; secret_set: boolean };
  keys: AppKey[];
  installations_count: number;
  created_at: string;
  updated_at: string;
};

export type Channels = { email: boolean; web: boolean };
export type NotificationSettings = {
  default_email: string;
  automatically_watch_repositories: boolean;
  automatically_watch_teams: boolean;
  participating: Channels;
  watching: Channels;
  actions: Channels & { failed_only: boolean };
  dependabot: Channels;
  own_updates: boolean;
  routes: { org: string; email: string }[];
};

export type EmailVisibility = { visibility: "public" | "private"; block_push: boolean };

export type DeviceRequest = {
  user_code: string;
  app: { name: string; owner: string; client_id: string };
  scopes: string[];
  created_at: string;
  expires_at: string;
};

export type Consent = {
  app: {
    name: string;
    owner: string;
    client_id: string;
    homepage_url: string;
    kind: "oauth_app" | "github_app";
  };
  scopes: string[];
  // A GitHub App asks for permissions, not scopes.
  permissions: AppPermissions | null;
  redirect_uri: string;
  redirect_host: string;
  localhost: boolean;
  already_authorized: boolean;
  code_expires_in: number;
};

export type UiPaths = {
  "/settings/avatar": {
    parameters: NoParams;
    post: {
      parameters: NoParams;
      requestBody: Body<{ data_url: string }>;
      responses: { 200: Json<Schemas["private-user"]>; 422: Invalid };
    };
    delete: { parameters: NoParams; responses: { 200: Json<Schemas["private-user"]> } };
  };
  "/settings/account/login": {
    parameters: NoParams;
    patch: {
      parameters: NoParams;
      requestBody: Body<{ login: string }>;
      responses: { 200: Json<Schemas["private-user"]>; 422: Invalid };
    };
  };
  "/settings/account": {
    parameters: NoParams;
    delete: {
      parameters: NoParams;
      requestBody: Body<{ confirm_login: string; confirm_phrase: string }>;
      responses: { 202: Json<{ deleted_at: string }>; 422: Invalid };
    };
  };
  "/settings/emails/primary": {
    parameters: NoParams;
    patch: {
      parameters: NoParams;
      requestBody: Body<{ email: string }>;
      responses: { 200: Json<Schemas["email"][]>; 422: Invalid };
    };
  };
  "/settings/emails/visibility": {
    parameters: NoParams;
    get: { parameters: NoParams; responses: { 200: Json<EmailVisibility> } };
    patch: {
      parameters: NoParams;
      requestBody: Body<Partial<EmailVisibility>>;
      responses: { 200: Json<EmailVisibility> };
    };
  };
  "/settings/emails/verify": {
    parameters: NoParams;
    post: {
      parameters: NoParams;
      requestBody: Body<{ email: string }>;
      responses: { 202: Json<{ email: string }>; 422: Invalid };
    };
  };
  "/settings/tokens": {
    parameters: NoParams;
    get: { parameters: NoParams; responses: { 200: Json<Token[]> } };
    post: {
      parameters: NoParams;
      requestBody: Body<NewToken>;
      responses: { 201: Json<IssuedToken>; 422: Invalid };
    };
  };
  "/settings/tokens/{id}": {
    parameters: PathOf<{ id: number }>;
    delete: { parameters: PathOf<{ id: number }>; responses: { 204: Empty; 404: Failed } };
  };
  "/settings/tokens/{id}/regenerate": {
    parameters: PathOf<{ id: number }>;
    post: {
      parameters: PathOf<{ id: number }>;
      requestBody: Body<{ expires_at: Expiry }>;
      responses: { 200: Json<IssuedToken>; 404: Failed; 422: Invalid };
    };
  };
  "/settings/security": {
    parameters: NoParams;
    get: { parameters: NoParams; responses: { 200: Json<Security> } };
  };
  "/settings/security/two-factor/setup": {
    parameters: NoParams;
    post: {
      parameters: NoParams;
      responses: { 200: Json<{ secret: string; otpauth_uri: string }> };
    };
  };
  "/settings/security/two-factor": {
    parameters: NoParams;
    post: {
      parameters: NoParams;
      requestBody: Body<{ code: string }>;
      responses: { 200: Json<{ security: Security; recovery_codes: string[] }>; 422: Invalid };
    };
    delete: { parameters: NoParams; responses: { 200: Json<Security> } };
  };
  "/settings/security/recovery-codes": {
    parameters: NoParams;
    post: {
      parameters: NoParams;
      responses: { 200: Json<{ recovery_codes: string[] }>; 422: Invalid };
    };
  };
  "/settings/sessions": {
    parameters: NoParams;
    get: { parameters: NoParams; responses: { 200: Json<Session[]> } };
  };
  "/settings/sessions/{id}": {
    parameters: PathOf<{ id: number }>;
    delete: { parameters: PathOf<{ id: number }>; responses: { 204: Empty; 422: Invalid } };
  };
  "/settings/security-log": {
    parameters: QueryOf<{ q?: string }>;
    get: { parameters: QueryOf<{ q?: string }>; responses: { 200: Json<SecurityEvent[]> } };
  };
  "/settings/applications/apps": {
    parameters: NoParams;
    get: { parameters: NoParams; responses: { 200: Json<AuthorizedApp[]> } };
  };
  "/settings/applications/apps/{slug}": {
    parameters: PathOf<{ slug: string }>;
    delete: { parameters: PathOf<{ slug: string }>; responses: { 204: Empty; 404: Failed } };
  };
  "/settings/oauth-apps": {
    parameters: NoParams;
    get: { parameters: NoParams; responses: { 200: Json<OAuthApp[]> } };
    post: {
      parameters: NoParams;
      requestBody: Body<OAuthAppInput>;
      responses: { 201: Json<OAuthApp>; 422: Invalid };
    };
  };
  "/settings/oauth-apps/{client_id}": {
    parameters: PathOf<{ client_id: string }>;
    get: {
      parameters: PathOf<{ client_id: string }>;
      responses: { 200: Json<OAuthApp>; 404: Failed };
    };
    patch: {
      parameters: PathOf<{ client_id: string }>;
      requestBody: Body<OAuthAppInput>;
      responses: { 200: Json<OAuthApp>; 404: Failed; 422: Invalid };
    };
    delete: { parameters: PathOf<{ client_id: string }>; responses: { 204: Empty; 404: Failed } };
  };
  "/settings/oauth-apps/{client_id}/secrets": {
    parameters: PathOf<{ client_id: string }>;
    post: {
      parameters: PathOf<{ client_id: string }>;
      responses: { 201: Json<{ secret: ClientSecret; value: string }>; 404: Failed };
    };
  };
  "/settings/oauth-apps/{client_id}/secrets/{id}": {
    parameters: PathOf<{ client_id: string; id: number }>;
    delete: {
      parameters: PathOf<{ client_id: string; id: number }>;
      responses: { 204: Empty; 404: Failed };
    };
  };
  "/settings/apps": {
    parameters: NoParams;
    get: { parameters: NoParams; responses: { 200: Json<DeveloperApp[]> } };
    // The manifest flow: the page posts the manifest and gets a one-time code, which it
    // trades at POST /api/v3/app-manifests/{code}/conversions for the credentials.
    post: {
      parameters: NoParams;
      requestBody: Body<AppManifest>;
      responses: { 201: Json<{ slug: string; code: string }>; 422: Invalid };
    };
  };
  "/settings/apps/{slug}": {
    parameters: PathOf<{ slug: string }>;
    get: {
      parameters: PathOf<{ slug: string }>;
      responses: { 200: Json<DeveloperApp>; 404: Failed };
    };
    patch: {
      parameters: PathOf<{ slug: string }>;
      requestBody: Body<Partial<AppManifest>>;
      responses: { 200: Json<DeveloperApp>; 404: Failed; 422: Invalid };
    };
    delete: { parameters: PathOf<{ slug: string }>; responses: { 204: Empty; 404: Failed } };
  };
  "/settings/apps/{slug}/keys": {
    parameters: PathOf<{ slug: string }>;
    post: {
      parameters: PathOf<{ slug: string }>;
      responses: { 201: Json<{ key: AppKey; pem: string }>; 404: Failed };
    };
  };
  "/settings/apps/{slug}/keys/{id}": {
    parameters: PathOf<{ slug: string; id: number }>;
    delete: {
      parameters: PathOf<{ slug: string; id: number }>;
      responses: { 204: Empty; 404: Failed; 422: Invalid };
    };
  };
  "/settings/installations": {
    parameters: NoParams;
    post: {
      parameters: NoParams;
      requestBody: Body<{
        app_slug: string;
        account: string;
        repository_selection: "all" | "selected";
        repository_ids: number[];
      }>;
      responses: { 201: Json<Schemas["installation"]>; 404: Failed; 422: Invalid };
    };
  };
  "/settings/installations/{id}": {
    parameters: PathOf<{ id: number }>;
    patch: {
      parameters: PathOf<{ id: number }>;
      requestBody: Body<{ repository_selection: "all" | "selected" }>;
      responses: { 200: Json<Schemas["installation"]>; 404: Failed };
    };
    delete: { parameters: PathOf<{ id: number }>; responses: { 204: Empty; 404: Failed } };
  };
  "/settings/notifications": {
    parameters: NoParams;
    get: { parameters: NoParams; responses: { 200: Json<NotificationSettings> } };
    put: {
      parameters: NoParams;
      requestBody: Body<NotificationSettings>;
      responses: { 200: Json<NotificationSettings>; 422: Invalid };
    };
  };
  // Sets the viewer's password (at least 8 characters). No page uses it yet.
  "/settings/password": {
    parameters: NoParams;
    put: {
      parameters: NoParams;
      requestBody: Body<{ password: string }>;
      responses: { 204: Empty; 422: Invalid };
    };
  };
  "/login/session": {
    parameters: NoParams;
    post: {
      parameters: NoParams;
      requestBody: Body<{ login: string; password: string }>;
      responses: {
        200: Json<{ login: string; two_factor: boolean; attempt: string }>;
        401: Failed;
      };
    };
  };
  "/login/two-factor": {
    parameters: NoParams;
    post: {
      parameters: NoParams;
      requestBody: Body<{ attempt: string; code: string }>;
      responses: { 200: Json<{ login: string }>; 422: Invalid };
    };
  };
  "/login/device": {
    parameters: NoParams;
    post: {
      parameters: NoParams;
      requestBody: Body<{ user_code: string }>;
      responses: { 200: Json<DeviceRequest>; 422: Invalid };
    };
  };
  // Stands in for a client such as gh calling POST /login/device/code on the bare host, so
  // the device page has a fresh code to enter.
  "/login/device/codes": {
    parameters: NoParams;
    post: {
      parameters: NoParams;
      requestBody: Body<{ client_id: string; scope: string }>;
      responses: { 201: Json<DeviceRequest>; 404: Failed };
    };
  };
  "/login/device/authorize": {
    parameters: NoParams;
    post: {
      parameters: NoParams;
      requestBody: Body<{ user_code: string; decision: "authorize" | "deny" }>;
      responses: { 200: Json<{ status: "authorized" | "denied" }>; 422: Invalid };
    };
  };
  "/oauth/authorize": {
    parameters: QueryOf<ConsentQuery>;
    get: {
      parameters: QueryOf<ConsentQuery>;
      responses: { 200: Json<Consent>; 404: Failed; 422: Invalid };
    };
    post: {
      parameters: NoParams;
      requestBody: Body<ConsentQuery & { decision: "authorize" | "deny" }>;
      responses: { 200: Json<{ redirect_to: string; expires_at: string | null }>; 422: Invalid };
    };
  };
};

export type ConsentQuery = {
  client_id: string;
  redirect_uri?: string;
  scope?: string;
  state?: string;
};

export const UI_BASE = "/_t/ui";
export const ui = createClient<UiPaths>({ baseUrl: UI_BASE });
ui.use(waitForMocks);
