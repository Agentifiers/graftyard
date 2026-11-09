import createClient from "openapi-fetch";
import { waitForMocks, type Schemas } from "@/api/client";

// The private /_t/ui API for what GHES 3.22's REST API has no endpoint for but its web
// UI does: saved and done notifications, mark as unread, and a watch's custom events.
// Hand-written in openapi-typescript's shape so openapi-fetch and openapi-msw type it.

type Thread = Schemas["thread"];

export const WATCH_EVENTS = [
  "issues",
  "pulls",
  "releases",
  "discussions",
  "security_alerts",
] as const;
export type WatchEvent = (typeof WATCH_EVENTS)[number];

export type CustomWatch = { events: WatchEvent[] | null };

type NoParams = { query?: never; header?: never; path?: never; cookie?: never };
type ThreadPath = { query?: never; header?: never; path: { thread_id: string }; cookie?: never };
type RepoPath = {
  query?: never;
  header?: never;
  path: { owner: string; repo: string };
  cookie?: never;
};
type Json<T> = { headers: { [name: string]: unknown }; content: { "application/json": T } };
type NoContent = { headers: { [name: string]: unknown }; content?: never };

export type UiPaths = {
  "/notifications/saved": {
    get: { parameters: NoParams; requestBody?: never; responses: { 200: Json<Thread[]> } };
  };
  "/notifications/saved/{thread_id}": {
    put: { parameters: ThreadPath; requestBody?: never; responses: { 204: NoContent } };
    delete: { parameters: ThreadPath; requestBody?: never; responses: { 204: NoContent } };
  };
  "/notifications/done": {
    get: { parameters: NoParams; requestBody?: never; responses: { 200: Json<Thread[]> } };
  };
  // Moves a done thread back to the inbox, as the web UI's "Move to inbox" does.
  "/notifications/done/{thread_id}": {
    delete: {
      parameters: ThreadPath;
      requestBody?: never;
      responses: { 204: NoContent; 404: Json<Schemas["basic-error"]> };
    };
  };
  // The web UI's "Mark as unread"; GHES's REST API only marks threads read.
  "/notifications/threads/{thread_id}/unread": {
    put: {
      parameters: ThreadPath;
      requestBody?: never;
      responses: { 204: NoContent; 404: Json<Schemas["basic-error"]> };
    };
  };
  "/repos/{owner}/{repo}/subscription/events": {
    get: { parameters: RepoPath; requestBody?: never; responses: { 200: Json<CustomWatch> } };
    put: {
      parameters: RepoPath;
      requestBody: { content: { "application/json": { events: WatchEvent[] } } };
      responses: { 200: Json<CustomWatch>; 422: Json<Schemas["validation-error"]> };
    };
    delete: { parameters: RepoPath; requestBody?: never; responses: { 204: NoContent } };
  };
};

export const UI_BASE = "/_t/ui";

export const ui = createClient<UiPaths>({ baseUrl: UI_BASE });
ui.use(waitForMocks);
