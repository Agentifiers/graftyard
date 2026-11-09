import type { RequestHandler } from "msw";

// Every module in ./areas that exports `handlers` is served. An area adds its own files
// and never edits this one.
const areas = import.meta.glob<{ handlers: RequestHandler[] }>("./areas/*.ts", { eager: true });

export const handlers = Object.values(areas).flatMap((m) => m.handlers ?? []);
