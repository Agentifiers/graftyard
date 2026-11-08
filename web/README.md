# Graftyard web app

The web app is build-plan package M4-WP5. This first slice runs against a mocked GHES 3.22 REST API in the browser.

It uses TanStack Start in SPA mode, TanStack Router, Query, DB, and Form, shadcn components on Base UI, ReUI free components, and HugeIcons free icons.

## Run it

```
npm ci
npm run dev          # Vite dev server on port 3000
npm run typecheck    # tsrs, the Rust port of the TypeScript 7 compiler
npm run format       # oxfmt; format:check fails on unformatted files
npm run check        # format:check, typecheck, and build
npm run build        # static assets and dist/client/_shell.html
npm run preview      # serves dist/client on $PORT (default 4173), unknown paths get _shell.html
```

The formatter and typechecker match the `deployments` repository's apps: oxfmt 0.56.0 and tsrs 0.18.0-ts7.1.0-dev.20260929, pinned exactly. `typescript` 5.9 stays only because `openapi-typescript` needs it to generate `src/api/ghes.d.ts`. oxfmt skips the generated files listed in `.oxfmtrc.json`.

The build has no server tier. Start prerenders only the root route into `_shell.html`. Every route renders in the browser. Do not add `createServerFn` or server routes.

## Layout

- `src/api/ghes.d.ts` holds the generated GHES 3.22 types for the operations this app uses. Do not edit it.
- `src/api/client.ts` is the `openapi-fetch` client on `/api/v3`.
- `src/api/queries.ts` has one `queryOptions` factory per page region. Keep data loading here, so the edge can later seed the first route's queries from inlined data.
- `src/api/query-client.ts` sets the retry policy. A query retries at most 2 times with full jitter and never retries a 4xx. A mutation never retries.
- `src/api/collections.ts` has the TanStack DB collections. The issues collection backs the issue list. The comments collection's `onInsert` POSTs a comment, so a new comment shows at once and rolls back if the POST fails.
- `src/components/region.tsx` is the `Region` component. Each page region is its own error boundary and Suspense boundary with its own queries.
- `src/routes/` holds the file routes.

## Mocks

`src/mocks/` holds the mocks. MSW runs in the browser through `public/mockServiceWorker.js`.

- `db.ts` is one in-memory store, seeded with a user, the `graftyard` org, 3 repositories, issues, pull requests with diffs, comments, and check runs. Every record is typed by its GHES schema. As in GHES, the issues list includes pull requests, each with a `pull_request` key.
- `handlers.ts` uses `openapi-msw`, so each path, parameter, request body, and response is checked against the same generated `paths` the client uses. The POST handlers change the store. A reload resets it.
- `browser.ts` starts the worker. The API client waits for it before the first request.

Mocking is on by default, in the production build too. Build with `VITE_MOCK_API=false` to call a real `/api/v3`.

## Fault injection

Add `?fail=<region>[,<region>]` to any URL. That region's handler answers 503 with a GHES error body. The setting persists in sessionStorage for the tab, and `?fail=` clears it.

Regions are `user`, `repos`, `repo`, `issues`, `issue`, `comments`, `post-comment`, `pulls`, `pull`, `files`, and `checks`. `post-comment` fails only new comments, which shows the rollback.

Example: `/graftyard/edge/pull/1?fail=checks` breaks only the Checks region.

## Regenerate the types

```
npm run gen:api
```

`web/scripts/gen-api.mjs` downloads the GHES 3.22 description at the commit `crates/api/gh/README.md` pins (about 11 MB) and caches it under `node_modules/.cache`. Set `GHES_JSON=/path/to/ghes-3.22.json` to use a local copy instead. The script keeps only the operations in its `OPERATIONS` list and the components they reach, drops examples, and writes `src/api/ghes.d.ts`. To use a new endpoint, add it to `OPERATIONS` and rerun the script.

## UI components

`components.json` uses the `base-nova` style, HugeIcons, and the `@reui` registry. Add components with the shadcn CLI: `npx shadcn@latest add <name>` for stock components and `npx shadcn@latest add @reui/<name>` for ReUI's free items. Do not add lucide or any paid icon set.
