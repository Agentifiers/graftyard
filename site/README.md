# Graftyard teaser site

A static Astro site that presents Graftyard as git hosting compatible with GHES 3.22. It is built with the taste-skill kit (https://github.com/Leonxlnx/taste-skill), and its films are silent cinetic loops (https://github.com/Leonxlnx/cinetic). It is dark by default; the sun button in the bar switches to light and the choice is kept in `localStorage`.

```
npm install
npm run dev       # local preview
npm run build     # static output in dist/
```

## Layout

- `src/layouts/Page.astro` is every page's frame: the head, the palette (dark and light), the bar, the footer, the theme button and the film player.
- `src/pages/index.astro` is the home page: the hero, Clients, GHES tests, Durability, Chaos and Questions.
- `src/pages/evidence.astro` is the evidence overview: GHES 3.22, chaos, tokens and latency, with hand-picked figures.
- `src/pages/evidence/all.astro` is every check, benchmark and baseline. `src/data/evidence.ts` reads it at build time from `tests/RESULTS.md` (every table) and `build/BUILD-PLAN.md` (each milestone's status table and every work package's acceptance table), so it follows those files with no edit here; milestones with no status table and no results yet are listed as not yet run.
- `src/pages/architecture.astro` is the architecture overview.
- `src/pages/why-not-artifacts.astro` is the case against Cloudflare Artifacts.
- `src/components/Logo.astro` is the Graftyard logo (graftyard-option-9.svg). On a dark page its branches take the text colour.
- `src/components/Film.astro` and `src/components/Table.astro` are the film frame and the results table, which stacks into labelled rows on a phone.
- `public/assets/` holds the parts of the taste-skill kit the pages use (MIT, `LICENSE.taste-skill`). Edit the pages, not these files.
- `public/film/` holds the rendered loops: `push-loop`, `ghes-loop` and `chaos-loop`, each as `.mp4`, `.webm` and a `.jpg` poster.
- `film/` is the cinetic Remotion project that renders the loops. See its `package.json` scripts.

## Deploying to graftyard.com

The site runs as the Worker `graftyard-site` (`wrangler.jsonc`, `worker.js`), which serves `dist/` on graftyard.com and www.graftyard.com. With `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` set:

```
npm run deploy    # fetch GitHub's status figures, astro build, then wrangler deploy
```

The workflow `.github/workflows/site-refresh.yml` runs the same deploy every day at 06:17 UTC on the srv runner pool, so the outage section's figures and its "as of" date move forward each day. It deploys with the repository secret `CLOUDFLARE_SITE_DEPLOY_TOKEN`, a Cloudflare token scoped to Workers scripts and routes on graftyard.com. If the status fetch fails, the job stops before it deploys, and the site keeps the previous figures. Run it by hand from the Actions tab (Site refresh, Run workflow).

## Page views (Cloudflare Basin)

`Page.astro` sends one beacon per page load to `POST /_v`: the path, the referring host, the viewport width and the theme. `worker.js` adds the time and the visitor's country and writes the row to the Basin stream `site_pageviews_stream`. It sets no cookie and stores no IP address or visitor id. The pipeline `site_pageviews` writes the rows to the Iceberg table `default.site_pageviews` in the R2 bucket `graftyard-site-analytics`, about a minute after the event.

To count views per page, set `WRANGLER_BASIN_SQL_AUTH_TOKEN` to the catalog token (the Amp project secret `GRAFTYARD_SITE_BASIN_TOKEN`) and run:

```
npm run views
```

## Keeping figures true

Every figure comes from `tests/RESULTS.md`, the results files it cites, and `design/DESIGN.md` at the repository root. Each page's front matter names its sources. Update the pages when those numbers change.
