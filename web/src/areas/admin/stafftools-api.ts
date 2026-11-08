import createClient from "openapi-fetch";
import { waitForMocks, type Schemas } from "@/api/client";

// The private stafftools reads under /_t/ui/stafftools. GHES 3.22 REST has no site-admin
// index of users, organizations, or repositories with suspension, LDAP, and lock state, no
// delivery log for global webhooks, and no list of the install's Apps, so the console reads
// these here and writes through GHES REST (`/admin/*`, `/users/{username}/site_admin`,
// `/users/{username}/suspended`).

export type LdapSync = "never" | "queued" | "synced";

export type StaffUser = Schemas["simple-user"] & {
  name: string | null;
  email: string | null;
  created_at: string;
  suspended_at: string | null;
  suspension_reason: string | null;
  ldap_dn: string | null;
  ldap_sync: LdapSync;
  ldap_synced_at: string | null;
  two_factor: boolean;
  public_repos: number;
};

export type StaffOrg = Schemas["organization-simple"] & {
  name: string | null;
  created_at: string;
  admins: string[];
  members: number;
  repositories: number;
};

// A repository with the state only a site admin sees: who holds a migration lock on it.
export type StaffRepo = Schemas["repository"] & {
  locked_by_migration: number | null;
};

export type HookDelivery = {
  id: number;
  guid: string;
  event: string;
  action: string | null;
  status_code: number;
  status: string;
  delivered_at: string;
  duration: number;
  request: { headers: Record<string, string>; payload: unknown };
  response: { headers: Record<string, string>; body: string };
};

export type InstallApp = {
  app_id: number;
  slug: string;
  name: string;
  owned: boolean;
  description: string;
};

type Json<T> = { content: { "application/json": T } };

export type StafftoolsPaths = {
  "/users": { get: { parameters: {}; responses: { 200: Json<StaffUser[]> } } };
  "/users/{username}": {
    get: { parameters: { path: { username: string } }; responses: { 200: Json<StaffUser> } };
  };
  "/repos": { get: { parameters: {}; responses: { 200: Json<StaffRepo[]> } } };
  "/orgs": { get: { parameters: {}; responses: { 200: Json<StaffOrg[]> } } };
  "/apps": { get: { parameters: {}; responses: { 200: Json<InstallApp[]> } } };
  "/hooks/{hook_id}/deliveries": {
    get: { parameters: { path: { hook_id: number } }; responses: { 200: Json<HookDelivery[]> } };
  };
};

export const STAFFTOOLS_BASE = "/_t/ui/stafftools";

export const stafftools = createClient<StafftoolsPaths>({ baseUrl: STAFFTOOLS_BASE });
stafftools.use(waitForMocks);
