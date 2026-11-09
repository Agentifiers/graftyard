import createClient from "openapi-fetch";
import { ApiError, waitForMocks, type Schemas } from "@/api/client";

// The calls this area makes that GHES 3.22 does not describe, typed by hand in
// openapi-typescript's shape so the client and the mock check them the same way.
//
// - `/api/v3/orgs/{org}/invitations*` is github.com's invitation API. GHES 3.22 only has
//   `PUT /orgs/{org}/memberships/{username}`, which cannot invite by email or into teams.
// - `/api/v3/orgs/{org}/rename` stands in for a rename an org owner can run. GHES 3.22 only
//   renames through the site admin's `PATCH /admin/organizations/{org}`. A rename is a
//   queued job: it shows as pending until a site admin's maintenance window runs it, and
//   the owner can cancel it before then.
// - `/_t/ui/profiles/*` is the private UI API's profile README and pinned repositories,
//   which github.com serves only through GraphQL.

type Json<T> = { headers: { [name: string]: unknown }; content: { "application/json": T } };
type Empty = { headers: { [name: string]: unknown }; content?: never };
type ValidationFailed = Json<Schemas["validation-error"]>;
type NotFound = Json<Schemas["basic-error"]>;

export type InvitationRole = "admin" | "direct_member" | "billing_manager";

export type OrganizationInvitation = {
  id: number;
  node_id: string;
  login: string | null;
  email: string | null;
  role: InvitationRole;
  created_at: string;
  failed_at: string | null;
  failed_reason: string | null;
  inviter: Schemas["simple-user"];
  team_count: number;
  invitation_teams_url: string;
  invitation_source: "member" | "scim";
};

export type PendingRename = {
  // The login the org will have once the job runs, or null when nothing is queued.
  login: string | null;
  queued_at: string | null;
  message: string;
};

export type ProfileExtras = {
  login: string;
  // Markdown from the profile README, or null when there is none.
  readme: string | null;
  // At most six, in the order the owner chose.
  pinned: Schemas["minimal-repository"][];
};

type OrgPath = { org: string };

export type ExtPaths = {
  "/orgs/{org}/invitations": {
    get: {
      parameters: {
        query?: { per_page?: number; page?: number };
        header?: never;
        path: OrgPath;
        cookie?: never;
      };
      requestBody?: never;
      responses: { 200: Json<OrganizationInvitation[]>; 404: NotFound };
    };
    post: {
      parameters: { query?: never; header?: never; path: OrgPath; cookie?: never };
      requestBody: {
        content: {
          "application/json": {
            invitee_id?: number;
            email?: string;
            role?: InvitationRole;
            team_ids?: number[];
          };
        };
      };
      responses: { 201: Json<OrganizationInvitation>; 404: NotFound; 422: ValidationFailed };
    };
  };
  "/orgs/{org}/invitations/{invitation_id}": {
    delete: {
      parameters: {
        query?: never;
        header?: never;
        path: OrgPath & { invitation_id: number };
        cookie?: never;
      };
      requestBody?: never;
      responses: { 204: Empty; 404: NotFound };
    };
  };
  "/orgs/{org}/rename": {
    get: {
      parameters: { query?: never; header?: never; path: OrgPath; cookie?: never };
      requestBody?: never;
      responses: { 200: Json<PendingRename>; 404: NotFound };
    };
    post: {
      parameters: { query?: never; header?: never; path: OrgPath; cookie?: never };
      requestBody: { content: { "application/json": { login: string } } };
      responses: { 202: Json<PendingRename>; 404: NotFound; 422: ValidationFailed };
    };
    delete: {
      parameters: { query?: never; header?: never; path: OrgPath; cookie?: never };
      requestBody?: never;
      responses: { 204: Empty; 404: NotFound };
    };
  };
};

// The org settings GHES 3.22 shows in its UI but does not accept in `PATCH /orgs/{org}`.
export type MemberPrivileges = {
  members_can_delete_repositories: boolean;
  members_can_change_repo_visibility: boolean;
  members_can_invite_outside_collaborators: boolean;
  members_can_delete_issues: boolean;
  display_commenter_full_name_setting_enabled: boolean;
  readers_can_create_discussions: boolean;
};

// Team defaults and creation rules, from the org's Teams settings page.
export type TeamSettings = {
  members_can_create_teams: boolean;
  default_privacy: "closed" | "secret";
  default_notification_setting: "notifications_enabled" | "notifications_disabled";
  team_discussions_enabled: boolean;
};

export type UiPaths = {
  "/orgs/{org}/team-settings": {
    get: {
      parameters: { query?: never; header?: never; path: OrgPath; cookie?: never };
      requestBody?: never;
      responses: { 200: Json<TeamSettings>; 404: NotFound };
    };
    patch: {
      parameters: { query?: never; header?: never; path: OrgPath; cookie?: never };
      requestBody: { content: { "application/json": Partial<TeamSettings> } };
      responses: { 200: Json<TeamSettings>; 404: NotFound; 422: ValidationFailed };
    };
  };
  "/orgs/{org}/member-privileges": {
    patch: {
      parameters: { query?: never; header?: never; path: OrgPath; cookie?: never };
      requestBody: { content: { "application/json": Partial<MemberPrivileges> } };
      responses: { 200: Json<Schemas["organization-full"]>; 404: NotFound };
    };
  };
  "/profiles/{login}": {
    get: {
      parameters: { query?: never; header?: never; path: { login: string }; cookie?: never };
      requestBody?: never;
      responses: { 200: Json<ProfileExtras>; 404: NotFound };
    };
  };
  "/profiles/{login}/contributions": {
    get: {
      parameters: { query?: never; header?: never; path: { login: string }; cookie?: never };
      requestBody?: never;
      // One entry per day, oldest first, covering the last 53 weeks.
      responses: { 200: Json<{ date: string; count: number }[]>; 404: NotFound };
    };
  };
  "/profiles/{login}/pins": {
    put: {
      parameters: { query?: never; header?: never; path: { login: string }; cookie?: never };
      requestBody: { content: { "application/json": { repository_ids: number[] } } };
      responses: { 200: Json<ProfileExtras>; 404: NotFound; 422: ValidationFailed };
    };
  };
};

export const ext = createClient<ExtPaths>({ baseUrl: "/api/v3" });
ext.use(waitForMocks);

export const ui = createClient<UiPaths>({ baseUrl: "/_t/ui" });
ui.use(waitForMocks);

// A failed write. A 422 carries GHES's `errors[]`, kept by field so a form can show each
// message under its input.
export class WriteError extends ApiError {
  readonly fields: Record<string, string>;
  constructor(status: number, body: unknown) {
    super(status, body);
    this.fields = {};
    const errors =
      body && typeof body === "object" && "errors" in body && Array.isArray(body.errors)
        ? (body.errors as { field?: string; code?: string; message?: string }[])
        : [];
    for (const e of errors) {
      if (e.field) this.fields[e.field] = e.message ?? CODES[e.code ?? ""] ?? `Is ${e.code}.`;
    }
    if (status === 422 && errors.length) {
      this.message = Object.values(this.fields).join(" ") || this.message;
    }
  }
}

// GHES validation codes, and the mock's own, as sentences.
const CODES: Record<string, string> = {
  missing_field: "Required.",
  invalid: "Not valid.",
  already_exists: "Already taken.",
  too_long: "Too long.",
  reserved_domain: "This domain is reserved and cannot receive mail.",
  last_owner: "An organization needs at least one owner.",
  already_member: "Already a member of this organization.",
  already_invited: "Already has a pending invitation.",
  cycle: "A team cannot be nested under itself or its own child team.",
  secret_nesting: "Secret teams cannot be nested or have child teams.",
  not_allowed: "The enterprise does not allow organizations to change this hook.",
  too_many: "Pin at most six repositories.",
};

type Result<T> = { data?: T; error?: unknown; response: Response };

// unwrap for writes: keeps the 422 body so forms can show field errors.
export async function send<T>(call: Promise<Result<T>>): Promise<T> {
  const { data, error, response } = await call;
  if (!response.ok) throw new WriteError(response.status, error);
  return data as T;
}

export function messageOf(e: unknown) {
  return e instanceof Error ? e.message : String(e);
}
