// BF-61 (build/BUILD-PLAN.md, M4-WP5) against a hosted edge: view a running job's live log in the web app, then kill
// its `LogRelay`. Pass: lines appear within 2 s; after the kill, the viewer falls back to step log blobs; the final
// log is complete. `BF61_JOB` is `{ "repo": "owner/name", "run": id, "job": id }` for a `tests/m4/wp3/hosted/live.yml`
// job a runner is running, whose relay someone aborts while it runs; the step waits for that drop. A line's lag is
// its arrival on the page's socket less the runner's own timestamp in its text (`line N at HH:MM:SS.nnnnnnnnn`),
// because the relay's `line.at` is the site's clock to the second.
import { expect, test } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";

const F = JSON.parse(process.env.BF38_FACTS ?? "{}");
const PASSWORD = process.env.BF38_PASSWORD;
const SHOTS = process.env.BF38_SHOTS ?? "/tmp/bf61-shots";
const JOB = process.env.BF61_JOB && JSON.parse(process.env.BF61_JOB);
mkdirSync(SHOTS, { recursive: true });

const COUNT = /line (\d+) at (\d\d):(\d\d):(\d\d\.\d+)/;
/** The runner's timestamp of a `line N at …` text, on the day of `near` (UTC). */
function runnerTime(text, near) {
  const m = text.match(COUNT);
  if (!m) return null;
  const d = new Date(near);
  return { n: Number(m[1]), at: Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate(), +m[2], +m[3]) + Math.round(+m[4] * 1000) };
}
const pct = (xs, p) => [...xs].sort((a, b) => a - b)[Math.min(xs.length - 1, Math.floor(xs.length * p))];

test("a running job's live log falls back to the saved log when its LogRelay is killed", async ({ page }) => {
  test.skip(!JOB, "BF61_JOB names no running job");
  test.setTimeout(20 * 60_000);
  const r = { job: JOB, live: [], frames: [] };
  const lines = r.live;

  await page.goto("/login");
  await page.locator('input[name="login"]').fill(F.login);
  await page.locator('input[name="password"]').fill(PASSWORD);
  await page.getByRole("button", { name: /sign in/i }).click();
  await expect(page.getByTestId("signed-in")).toBeVisible();

  // The page's own relay socket: every frame with its arrival, and when it closes.
  let closedAt = null;
  page.on("websocket", (ws) => {
    if (!ws.url().includes("/_t/logrelay/repos/")) return;
    r.frames.push({ at: Date.now(), type: "open", url: new URL(ws.url()).pathname });
    ws.on("framereceived", (f) => {
      const now = Date.now();
      const m = JSON.parse(f.payload);
      r.frames.push({ at: now, type: m.type });
      if (m.type === "line") {
        const t = runnerTime(m.line.text, now);
        if (t) lines.push({ n: t.n, lag: now - t.at, arrived: now });
      }
    });
    ws.on("close", () => {
      closedAt ??= Date.now();
      r.frames.push({ at: closedAt, type: "close" });
    });
  });

  await page.goto(`/${JOB.repo}/actions/runs/${JOB.run}/job/${JOB.job}`);
  await expect(page.getByTestId("log-live")).toBeVisible();
  await page.screenshot({ path: `${SHOTS}/1-live.png` });

  // Before the kill: lines stream. Then wait for the relay to drop the socket (the abort).
  await expect.poll(() => closedAt, { timeout: 15 * 60_000, intervals: [500] }).not.toBeNull();
  r.closedAt = new Date(closedAt).toISOString();
  r.endBeforeClose = r.frames.some((f) => f.type === "end");
  const fallbackAt = await (async () => {
    await expect(page.getByTestId("log-fallback")).toBeVisible({ timeout: 15_000 });
    return Date.now();
  })();
  r.fallbackMs = fallbackAt - closedAt;
  await page.waitForTimeout(4000);
  await page.screenshot({ path: `${SHOTS}/2-fallback.png` });
  r.linesShownAfterFallback = Number(await page.locator("[data-log-lines]").getAttribute("data-log-lines"));

  // The job ends on its own; the page then reads the final log from the blobs.
  const web = { "x-graftyard-web": "1" };
  const api = `/api/v3/repos/${JOB.repo}/actions/jobs/${JOB.job}`;
  await expect
    .poll(async () => (await (await page.request.get(api, { headers: web })).json()).status, { timeout: 15 * 60_000, intervals: [5000] })
    .toBe("completed");
  const job = await (await page.request.get(api, { headers: web })).json();
  r.conclusion = job.conclusion;
  const text = await (await page.request.get(`${api}/logs`, { headers: web })).text();
  const saved = text.split("\n").filter(Boolean);
  writeFileSync(`${SHOTS}/final.log`, text);
  const counted = new Set(saved.map((l) => l.match(COUNT)?.[1]).filter(Boolean).map(Number));
  r.final = {
    lines: saved.length,
    counted: counted.size,
    missingCount: Array.from({ length: 600 }, (_, i) => i + 1).filter((n) => !counted.has(n)),
    lastStep: saved.some((l) => l.endsWith(" last step")),
    liveLinesMissing: lines.filter((l) => !counted.has(l.n)).map((l) => l.n),
  };
  // The page's fallback polls `/logs` every 3 s while the job runs and reads it once more at the end.
  await expect.poll(async () => Number(await page.locator("[data-log-lines]").getAttribute("data-log-lines")), { timeout: 60_000 }).toBeGreaterThanOrEqual(600);
  r.final.pageLines = Number(await page.locator("[data-log-lines]").getAttribute("data-log-lines"));
  r.final.pageSource = await page.locator("[data-log-source]").getAttribute("data-log-source");
  await page.screenshot({ path: `${SHOTS}/3-final.png` });

  const lags = lines.map((l) => l.lag);
  r.lag = { n: lags.length, p50: pct(lags, 0.5), p99: pct(lags, 0.99), max: Math.max(...lags), over2s: lags.filter((x) => x > 2000).length };
  r.live = { n: lines.length, first: lines[0]?.n, last: lines.at(-1)?.n };
  r.frames = r.frames.filter((f) => f.type !== "line");
  console.log(`BF61_RESULTS ${JSON.stringify(r)}`);

  expect(r.lag.n).toBeGreaterThan(10);
  expect(r.lag.over2s, "a line later than 2 s").toBe(0);
  expect(r.endBeforeClose, "the relay ended the stream instead of being killed").toBe(false);
  expect(r.fallbackMs).toBeLessThan(15_000);
  expect(r.conclusion).toBe("success");
  expect(r.final.missingCount, "missing lines in the final log").toEqual([]);
  expect(r.final.lastStep).toBe(true);
  expect(r.final.liveLinesMissing).toEqual([]);
});
