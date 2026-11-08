import createClient from "openapi-fetch";
import { waitForMocks, type Schemas } from "@/api/client";

// The private /_t/ui paths repository administration reads where GHES 3.22's REST API has
// no route: the LFS switch has no GET, PATCH /repos/{owner}/{repo} takes neither
// has_discussions nor allow_auto_merge, branch protection is per branch with no list of
// rules (GHES's own UI reads GraphQL's branchProtectionRules), and the API lists the
// installation of one authenticated app, not every app on a repository.
// Types follow openapi-typescript's shape so openapi-fetch and openapi-msw check them.

type RepoPath = { owner: string; repo: string };
type Json<T> = { content: { "application/json": T } };
type Invalid = Json<Schemas["validation-error"]>;

export type AdminSettings = {
  // Enabling LFS is a background job, so the switch can be mid-way.
  lfs: "enabled" | "enabling" | "disabled";
  // Bytes stored in LFS, for the size line under the switch.
  lfs_bytes: number;
};

export type FeatureSettings = { has_discussions?: boolean; allow_auto_merge?: boolean };

// One branch protection rule: the pattern, what it requires, and the branches it covers.
export type ProtectionRule = {
  pattern: string;
  protection: Schemas["branch-protection"];
  matching_branches: string[];
  created_at: string;
};

export type UiPaths = {
  "/repos/{owner}/{repo}/admin-settings": {
    get: {
      parameters: { path: RepoPath };
      responses: { 200: Json<AdminSettings> };
    };
    patch: {
      parameters: { path: RepoPath };
      requestBody: Json<FeatureSettings>;
      responses: { 200: Json<Schemas["full-repository"]>; 403: Json<Schemas["basic-error"]> };
    };
  };
  "/repos/{owner}/{repo}/protection-rules": {
    get: {
      parameters: { path: RepoPath };
      responses: { 200: Json<ProtectionRule[]> };
    };
  };
  // Check names reported on the repository recently, for the required-checks picker.
  "/repos/{owner}/{repo}/check-names": {
    get: {
      parameters: { path: RepoPath };
      responses: { 200: Json<string[]> };
    };
  };
  "/repos/{owner}/{repo}/installations": {
    get: {
      parameters: { path: RepoPath };
      responses: { 200: Json<Schemas["installation"][]>; 422: Invalid };
    };
  };
};

export const ui = createClient<UiPaths>({ baseUrl: "/_t/ui" });
ui.use(waitForMocks);
