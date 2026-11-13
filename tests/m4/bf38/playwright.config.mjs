// BF-38 against a hosted edge. `BF38_EDGE` is the edge, `BF38_FACTS` the JSON seed.mjs printed, `BF38_PASSWORD` the
// admin's web password, and `BF38_SHOTS` where screenshots go.
import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  testMatch: "bf38.spec.mjs",
  timeout: 240_000,
  expect: { timeout: 30_000 },
  workers: 1,
  reporter: [["list"], ["json", { outputFile: process.env.BF38_REPORT ?? "/tmp/bf38-report.json" }]],
  use: {
    baseURL: process.env.BF38_EDGE,
    viewport: { width: 1280, height: 900 },
    deviceScaleFactor: 2,
    launchOptions: { executablePath: process.env.CHROMIUM ?? "/usr/sbin/chromium" },
  },
});
