// Snapshot of GitHub's last 90 days from The Missing GitHub Status Page (https://mrshu.github.io/github-statuses/,
// MIT, by Marek Šuppa), written to src/data/github-status.json for the home page's outage section.
// It repeats that page's own arithmetic (app.js): days run in UTC, a day takes its worst incident's impact,
// maintenance shows on the day but is not downtime, and uptime is 1 minus the merged downtime minutes over 90 days.
//   node scripts/github-status.mjs            # snapshot as of now
//   node scripts/github-status.mjs 2026-11-06 # as of that UTC day
import { writeFileSync, mkdirSync } from 'node:fs';

const ROOT = 'https://mrshu.github.io/github-statuses/parsed';
const IMPACTS = ['none', 'maintenance', 'minor', 'major', 'critical'];
const SERVICES = ['Git Operations', 'Webhooks', 'API Requests', 'Issues', 'Pull Requests', 'Actions'];
const DAY = 86400000;

const parseCSV = (text) => {
  const [head, ...lines] = text.trim().split(/\r?\n/);
  const keys = head.split(',');
  // titles may hold commas; only the first five and the last field are read, so split from both ends
  return lines.map((line) => {
    const parts = line.split(',');
    const row = Object.fromEntries(keys.slice(0, 5).map((k, i) => [k, parts[i]]));
    row.impact = parts[parts.length - 1];
    return row;
  });
};
const merge = (spans) => {
  const out = [];
  for (const s of spans.slice().sort((a, b) => a[0] - b[0])) {
    const last = out[out.length - 1];
    if (last && s[0] <= last[1]) last[1] = Math.max(last[1], s[1]);
    else out.push([...s]);
  }
  return out;
};
const minutes = ([a, b]) => Math.max(0, Math.ceil(b / 60000) - Math.floor(a / 60000));
const clip = (a, b, lo, hi) => (Math.min(b, hi) > Math.max(a, lo) ? [Math.max(a, lo), Math.min(b, hi)] : null);

const asOf = process.argv[2] ? new Date(`${process.argv[2]}T12:00:00Z`) : new Date();
const today = Date.UTC(asOf.getUTCFullYear(), asOf.getUTCMonth(), asOf.getUTCDate());
const lo = today - 89 * DAY, hi = today + DAY;

const [incText, winText] = await Promise.all([`${ROOT}/incidents.jsonl`, `${ROOT}/downtime_windows.csv`].map((u) => fetch(u).then((r) => {
  if (!r.ok) throw new Error(`${u}: ${r.status}`);
  return r.text();
})));
const incidents = incText.trim().split('\n').map((l) => JSON.parse(l));
const windows = parseCSV(winText);

// the platform row: every downtime window
const days = new Array(90).fill(0);
const spans = [];
const ids = new Set();
let longest = null;
for (const w of windows) {
  const c = clip(Date.parse(w.downtime_start), Date.parse(w.downtime_end), lo, hi);
  if (!c) continue;
  ids.add(w.incident_id);
  if (w.impact !== 'maintenance') spans.push(c);
  for (let d = Math.floor(c[0] / DAY) * DAY; d <= Math.floor(c[1] / DAY) * DAY; d += DAY) {
    const i = (d - lo) / DAY;
    if (i >= 0 && i < 90) days[i] = Math.max(days[i], IMPACTS.indexOf(w.impact));
  }
}
const downtime = merge(spans).reduce((s, x) => s + minutes(x), 0);

// per service: incidents tagged with the component
const services = SERVICES.map((name) => {
  const own = [];
  for (const inc of incidents) {
    if (!inc.components?.includes(name) || !inc.downtime_start || !inc.downtime_end || inc.impact === 'maintenance') continue;
    const c = clip(Date.parse(inc.downtime_start), Date.parse(inc.downtime_end), lo, hi);
    if (c) own.push(c);
  }
  const m = merge(own).reduce((s, x) => s + minutes(x), 0);
  return { name, uptime: (100 * (1 - m / (90 * 1440))).toFixed(2), downtimeMinutes: m };
});

for (const inc of incidents) {
  if (!ids.has(String(inc.id)) || inc.impact === 'maintenance') continue;
  if (!longest || (inc.duration_minutes || 0) > longest.minutes) longest = { title: inc.title, minutes: inc.duration_minutes, day: inc.started_at.slice(0, 10), impact: inc.impact };
}

const out = {
  source: 'https://mrshu.github.io/github-statuses/',
  sourceAuthor: 'Marek Šuppa',
  asOf: new Date(today).toISOString().slice(0, 10),
  from: new Date(lo).toISOString().slice(0, 10),
  // counted as that page counts them: every incident that started inside the window, maintenance included
  incidents: incidents.filter((i) => Date.parse(i.started_at || i.published_at) >= lo).length,
  uptime: (100 * (1 - downtime / (90 * 1440))).toFixed(2),
  downtimeMinutes: downtime,
  daysWithIncident: days.filter((d) => d >= 2).length,
  days: days.map((d) => IMPACTS[d]),
  services,
  longest,
};
mkdirSync(new URL('../src/data/', import.meta.url), { recursive: true });
writeFileSync(new URL('../src/data/github-status.json', import.meta.url), JSON.stringify(out, null, 2) + '\n');
console.log(`${out.asOf}: ${out.uptime}% uptime, ${out.incidents} incidents, ${out.daysWithIncident} of 90 days with an incident; ` +
  services.map((s) => `${s.name} ${s.uptime}%`).join(', '));
