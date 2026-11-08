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

The app is split by area. An area owns a folder, its mock files, and its routes, and plugs into the shared layouts through two registries. It never edits a shared file, so areas can be built in parallel.

Shared, edited only by the shell:

- `src/api/ghes.d.ts` holds the GHES 3.22 types for every operation, generated from the whole description. Do not edit it.
- `src/api/client.ts` is the `openapi-fetch` client on `/api/v3`, `ApiError`, `unwrap`, and `waitForMocks`, the middleware any other client (such as one on `/api/graftyard/v1`) must also use.
- `src/api/query-client.ts` sets the retry policy. A query retries at most 2 times with full jitter and never retries a 4xx. A mutation never retries.
- `src/areas/nav.ts` is the nav registry. Each `src/areas/<area>/nav.ts` exports `nav: NavEntry[]` for the slots `repo` (tabs), `repo-settings`, `org` (tabs), `org-settings`, `user-settings`, `user-menu`, and `admin`. Settings sidebars and the user menu draw one heading per `group`. `web/scripts/check-nav.mjs` fails on a target that is not a route or uses a param other than `$owner`, `$repo`, or `$org`.
- `src/areas/slots.tsx` is the widget registry. Each `src/areas/<area>/slots.tsx` exports `slots: SlotEntry[]` built with `slot({...})`. The slots are `topbar`, `home`, `repo-actions`, `repo-overview`, `repo-sidebar`, `issue-sidebar`, and `pull-sidebar`, each with typed props. Every widget is its own `Region`.
- `src/components/region.tsx` is `Region`: one error boundary and Suspense boundary per page region, with "Try again".
- `src/components/panel.tsx` has `Panel` (a bordered box with a title bar) and `PageHeader`.
- `src/components/markdown.tsx` renders GitHub Flavored Markdown with raw HTML escaped.
- `src/components/github.tsx` has state badges and icons, labels, avatars, and relative times.
- `src/components/side-nav.tsx` and `tab-nav.tsx` draw the registries. `src/components/mock-panel.tsx` is the Mock button in the top bar.
- Layout routes: `__root.tsx` (top bar), `$owner.$repo.tsx` (repository header and tabs), `$owner.$repo.settings.tsx`, `orgs.$org.tsx` (organization header and tabs), `orgs.$org.settings.tsx`, `settings.tsx` (the viewer's settings), and `stafftools.tsx` (site admin). A settings index redirects to its slot's first section.
- `src/mocks/world.ts` holds the people, the org, its teams and members, the repositories, branch names and shas, and tags. `src/mocks/http.ts` holds `http` (openapi-msw typed by the full `paths`), `gate`, `page`, `notFound`, and `unprocessable`.

Owned by one area:

- `src/areas/<area>/` holds its `nav.ts`, `slots.tsx`, `queries.ts` (one `queryOptions` factory per page region), collections, and components.
- `src/mocks/areas/<area>*.ts` holds its records and handlers. Every module there that exports `handlers` is served.
- Its routes in `src/routes/`.
- `core` owns users, repositories, issues, comments, pull requests, files, and check runs, and `shell` owns the organization header's reads.

Rules for an area:

- Type every record by its GHES schema, and serve it through `http`, so a fixture that drifts from the wire contract fails typecheck. An API outside GHES 3.22, such as `/_t/ui/*` or `/api/graftyard/v1`, gets hand-written types in the area, a client that uses `waitForMocks`, and `createOpenApiHttp` on its own base URL.
- Start each handler with `gate("<fault key>")`, and give each mutation its own fault key, so the Mock panel can fail one region or one write.
- Use only the components in `src/components/ui` and `src/components/reui` and free HugeIcons. Keep corners square: `rounded-sm` at most, never `rounded-full`, `rounded-lg`, or larger.
- Show writes at once where the UI can (a TanStack DB collection or an optimistic update), never retry them, and roll them back visibly on failure. Forms use TanStack Form and show a 422's field errors.
- Link only to routes that exist. Add no npm dependency.

## Regenerate the types

```
npm run gen:api
```

`web/scripts/gen-api.mjs` downloads the GHES 3.22 description at the commit `crates/api/gh/README.md` pins (about 11 MB) and caches it under `node_modules/.cache`. Set `GHES_JSON=/path/to/ghes-3.22.json` to use a local copy instead. The script drops examples and writes every operation to `src/api/ghes.d.ts`.

## UI components

`components.json` uses the `base-nova` style, HugeIcons, and the `@reui` registry. Every stock shadcn component and every free ReUI primitive is installed. Vendored files are patched only where the strict tsconfig rejects them, and `rounded-full` became `rounded-sm` except on the radio dot. ReUI's `registry:block` items need a Pro license and are not used. Do not add lucide or any paid icon set.
