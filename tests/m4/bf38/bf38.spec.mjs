// BF-38 (build/BUILD-PLAN.md, M4-WP5) against a hosted edge: sign in, open a repository, a pull request's timeline,
// diff, and checks, then force the checks region's query to fail. The running job's live log needs LogRelay
// (M4-WP3) and is pending. Each page asserts that the route's regions render from the shell's inlined seed: no read
// of a seeded query reaches the network during the load, and the regions' content is on screen.
import { expect, test } from "@playwright/test";
import { mkdirSync } from "node:fs";

const F = JSON.parse(process.env.BF38_FACTS ?? "{}");
const PASSWORD = process.env.BF38_PASSWORD;
const SHOTS = process.env.BF38_SHOTS ?? "/tmp/bf38-shots";
mkdirSync(SHOTS, { recursive: true });
const R = `${F.owner}/${F.repo}`;
const pull = `/${R}/pull/${F.number}`;

test.describe.configure({ mode: "serial" });
let ctx;
let page;
test.beforeAll(async ({ browser }) => {
  ctx = await browser.newContext();
  await ctx.addInitScript(marks, MARKS);
  page = await ctx.newPage();
});
test.afterAll(async () => ctx?.close());

// When each route region first appears, on the page's clock, recorded by a MutationObserver from the first byte.
const MARKS = ["directory", "readme", "pull-title", "comments", "files-tab", "file-tree", "checks-tab", "statuses"];
function marks(ids) {
  window.__bf38 = {};
  new MutationObserver(() => {
    for (const id of ids) if (!window.__bf38[id] && document.querySelector(`[data-testid="${id}"]`)) window.__bf38[id] = performance.now();
  }).observe(document, { childList: true, subtree: true });
}

/** Loads `path` as a document and returns the seed's query keys, every `/api` and `/_t` read with its start time,
 * when the route's regions rendered, and first contentful paint. Reads under the agent media type are the agent
 * card's (chrome) and are left out. */
async function load(path, ids) {
  const agent = new Set();
  const onReq = (req) => { if (/agent/.test(req.headers().accept ?? "")) agent.add(req.url()); };
  page.on("request", onReq);
  await page.goto(path, { waitUntil: "domcontentloaded" });
  for (const id of ids) await expect(page.getByTestId(id).first()).toBeVisible();
  await page.waitForTimeout(1500);
  page.off("request", onReq);
  const t = await page.evaluate((ids) => ({
    seed: JSON.parse(document.getElementById("__graft_seed")?.textContent ?? "null"),
    rendered: Math.max(...ids.map((id) => window.__bf38[id] ?? Infinity)),
    fcp: performance.getEntriesByName("first-contentful-paint")[0]?.startTime ?? null,
    reads: performance.getEntriesByType("resource")
      .filter((e) => /^\/(api|_t)\//.test(new URL(e.name).pathname))
      .map((e) => ({ url: e.name, path: new URL(e.name).pathname, at: Math.round(e.startTime) })),
  }), ids);
  return {
    keys: (t.seed?.queries ?? []).map(([k]) => k),
    rendered: Math.round(t.rendered),
    fcp: t.fcp && Math.round(t.fcp),
    reads: t.reads.filter((r) => !agent.has(r.url)).map(({ path, at }) => ({ path, at })),
  };
}

/** The request paths that would serve a seeded query key (the query factories' URLs). */
function seededPath(k) {
  const [a, ...rest] = k;
  if (a === "user") return "/api/v3/user";
  if (a === "_t") {
    const [, o, r, what, ref, path] = rest;
    return what === "tree-commits" ? `/_t/ui/repos/${o}/${r}/tree-commits/${ref}${path ? "/" + path : ""}` : `/_t/ui/repos/${o}/${r}/${what}`;
  }
  const [o, r, ...tail] = rest;
  if (tail[0] === "pulls" && ["files", "commits", "threads", "events", "merge-requirements", "viewed"].includes(tail[2]))
    return `/_t/ui/repos/${o}/${r}/pulls/${tail[1]}/${tail[2]}`;
  return `/api/v3/repos/${o}/${r}${tail.length ? "/" + tail.join("/") : ""}`;
}
/** Seeded reads that started before the route's regions rendered or before first contentful paint. */
function seededReads({ keys, reads, rendered, fcp }) {
  const paths = new Set(keys.map(seededPath));
  return reads.filter((r) => paths.has(r.path) && (r.at < rendered || (fcp !== null && r.at < fcp)));
}
const results = {};
test.afterAll(() => console.log(`BF38_RESULTS ${JSON.stringify(results)}`));

test("sign in with the web password", async () => {
  expect(F.repo, "BF38_FACTS from seed.mjs").toBeTruthy();
  expect(PASSWORD, "BF38_PASSWORD").toBeTruthy();
  await page.goto("/login");
  await page.locator('input[name="login"]').fill(F.login);
  await page.locator('input[name="password"]').fill(PASSWORD);
  await page.getByRole("button", { name: /sign in/i }).click();
  await expect(page.getByTestId("signed-in")).toBeVisible();
  const cookies = await ctx.cookies();
  const s = cookies.find((c) => c.name === "__Host-gy_session");
  expect(s?.httpOnly && s?.secure).toBeTruthy();
  await page.screenshot({ path: `${SHOTS}/1-signed-in.png` });
  results.signIn = { cookie: s.name, httpOnly: s.httpOnly, secure: s.secure, sameSite: s.sameSite };
});

test("repository overview renders from the shell", async () => {
  const r = await load(`/${R}`, ["directory", "readme"]);
  await expect(page.getByTestId("directory")).toContainText("src");
  await expect(page.getByTestId("readme")).toContainText("BF-38's repository");
  await page.screenshot({ path: `${SHOTS}/2-repository.png` });
  const reads = seededReads(r);
  results.overview = { seeded: r.keys.length, seededReads: reads, renderedMs: r.rendered, fcpMs: r.fcp, reads: r.reads };
  expect(r.keys.length).toBeGreaterThanOrEqual(4);
  expect(reads, "a seeded region fetched its data").toEqual([]);
  await expect(page.locator("[data-region-error]")).toHaveCount(0);
});

test("pull request timeline renders from the shell, and mergeable resolves", async () => {
  const gql = [];
  const onGql = async (res) => {
    if (new URL(res.url()).pathname !== "/api/graphql") return;
    try { gql.push((await res.json())?.data?.repository?.pullRequest?.mergeable ?? null); } catch { gql.push("unreadable"); }
  };
  page.on("response", onGql);
  const r = await load(pull, ["pull-title", "comments"]);
  await expect(page.getByTestId("pull-title")).toContainText("Honor Retry-After");
  await expect(page.getByTestId("comments")).toContainText("the checks region should fail alone");
  await expect(page.getByTestId("comments")).toContainText("Cap this at 10 s");
  // `mergeable` is UNKNOWN until the test merge is computed; the merge box polls the persisted query until it is not.
  await expect.poll(() => gql.filter((m) => m && m !== "UNKNOWN").length, { timeout: 90_000 }).toBeGreaterThan(0);
  await expect(page.getByTestId("merge-checking")).toHaveCount(0);
  page.off("response", onGql);
  await page.screenshot({ path: `${SHOTS}/3-timeline.png`, fullPage: true });
  const reads = seededReads(r);
  results.timeline = { seeded: r.keys.length, seededReads: reads, renderedMs: r.rendered, fcpMs: r.fcp, reads: r.reads, mergeable: gql };
  expect(reads, "a seeded region fetched its data").toEqual([]);
  await expect(page.locator("[data-region-error]")).toHaveCount(0);
});

test("pull request diff renders from the shell", async () => {
  const r = await load(`${pull}?tab=files`, ["pull-title", "files-tab", "file-tree"]);
  const tree = page.getByTestId("file-tree");
  for (const f of ["retry.ts", "budget.ts", "budget.md"]) await expect(tree).toContainText(f);
  await expect(page.getByTestId("files-tab")).toContainText("Retry-After wins over the jitter");
  await page.screenshot({ path: `${SHOTS}/4-diff.png` });
  const reads = seededReads(r);
  results.diff = { seeded: r.keys.length, seededReads: reads, renderedMs: r.rendered, fcpMs: r.fcp, reads: r.reads };
  expect(reads, "a seeded region fetched its data").toEqual([]);
  await expect(page.locator("[data-region-error]")).toHaveCount(0);
});

test("pull request checks render from the shell", async () => {
  const r = await load(`${pull}?tab=checks`, ["pull-title", "checks-tab", "statuses"]);
  await expect(page.getByTestId("checks-summary")).toHaveText("1 of 3 checks passed, 1 failed, 1 running");
  await expect(page.getByTestId("statuses")).toContainText("ci/preview");
  await page.screenshot({ path: `${SHOTS}/5-checks.png` });
  const reads = seededReads(r);
  results.checks = { seeded: r.keys.length, seededReads: reads, renderedMs: r.rendered, fcpMs: r.fcp, reads: r.reads };
  expect(reads, "a seeded region fetched its data").toEqual([]);
  await expect(page.locator("[data-region-error]")).toHaveCount(0);
});

test("with the checks query failing, only the checks region shows an error", async () => {
  // The shell seeds check-runs, so the fault removes that entry from the seed and answers its read 503.
  let failed = 0;
  await page.route(`**/api/v3/repos/${R}/commits/*/check-runs*`, (route) => {
    failed++;
    return route.fulfill({ status: 503, headers: { "retry-after": "1", "content-type": "application/json" }, body: JSON.stringify({ message: "Service Unavailable" }) });
  });
  await page.route((u) => u.pathname === pull, async (route) => {
    const res = await route.fetch();
    const html = (await res.text()).replace(/(<script id="__graft_seed" type="application\/json">)(.*?)(<\/script>)/s, (_, a, body, z) => {
      const s = JSON.parse(body);
      s.queries = s.queries.filter(([k]) => k.at(-1) !== "check-runs");
      return a + JSON.stringify(s).replace(/</g, "\\u003c") + z;
    });
    return route.fulfill({ response: res, body: html });
  });
  await page.goto(`${pull}?tab=checks`);
  await expect(page.locator('[data-region-error="checks"]')).toBeVisible({ timeout: 60_000 });
  await expect(page.locator("[data-region-error]")).toHaveCount(1);
  await expect(page.getByTestId("pull-title")).toBeVisible();
  await expect(page.getByRole("complementary")).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/6-checks-fault.png` });
  // The same fault on the timeline: the merge box and tab bar read check-runs too, and show no error.
  await page.goto(pull);
  await expect(page.getByTestId("comments")).toBeVisible();
  await page.waitForTimeout(8000);
  await expect(page.locator("[data-region-error]")).toHaveCount(0);
  await page.screenshot({ path: `${SHOTS}/7-timeline-checks-fault.png` });
  results.fault = { checkRunReads503: failed };
  expect(failed).toBeGreaterThanOrEqual(3);
  await page.unrouteAll();
});

test.fixme("a running job's live log streams (needs LogRelay, M4-WP3)", async () => {});
