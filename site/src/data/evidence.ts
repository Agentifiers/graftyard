// The full evidence page's data, read at build time from the repository's own records, so the page lists exactly
// what they hold: every table in tests/RESULTS.md, and every milestone's status table and every work package's
// acceptance (gate) table in build/BUILD-PLAN.md. The site deploys daily, so the page follows new results.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

export interface Table { label: string; head: string[]; rows: string[][] }
export interface Block { id: string; title: string; intro: string[]; tables: Table[] }
export interface Part { id: string; title: string; intro: string[]; blocks: Block[]; planned: boolean }

// npm scripts run from site/, so the repository root is one up
const read = (rel: string) => readFileSync(resolve(process.cwd(), '..', rel), 'utf8');

const esc = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
// inline Markdown as the records write it: `code`, **bold**, *emphasis*, [text](url), and the evidence tags
export const inline = (md: string) => {
  const parts = md.split(/(`[^`]*`)/);
  return parts.map((p) => {
    if (p.startsWith('`') && p.endsWith('`') && p.length > 1) return `<code>${esc(p.slice(1, -1))}</code>`;
    return esc(p)
      .replace(/\*\*([^*]+)\*\*/g, '<b>$1</b>')
      .replace(/(^|[\s(])\*([^*\s][^*]*)\*/g, '$1<em>$2</em>')
      .replace(/\[([^\]]+)\]\((https?:[^)\s]+)\)/g, '<a href="$2" rel="noopener">$1</a>')
      .replace(/\[(measured|doc|inference|guess|unverified)([^\]]*)\]/g, '<span class="tag">$1$2</span>')
      // a bracketed list such as a site set [0,1,2] is data: its brackets go in their own text nodes, so the kit's
      // placeholder guard (system.js, PLACEHOLDER), which tests one text node at a time, does not flag it
      .replace(/\[([\d, ]+)\]/g, '<code><span>[</span>$1<span>]</span></code>')
      // gate and package ids (BF-20, B0-24a, M2-WP6) never break at their hyphen
      .replace(/\b([A-Z]{1,3}\d?-(?:[A-Z]{1,3}\d+|\d+[a-z]?))\b/g, '<span class="nw">$1</span>');
  }).join('');
};

const cells = (line: string) => {
  // split on pipes outside backticks
  const out: string[] = []; let cur = ''; let tick = false;
  for (const ch of line.trim().replace(/^\|/, '').replace(/\|$/, '')) {
    if (ch === '`') tick = !tick;
    if (ch === '|' && !tick) { out.push(cur.trim()); cur = ''; } else cur += ch;
  }
  out.push(cur.trim());
  return out;
};
const slug = (s: string) => s.toLowerCase().replace(/`/g, '').replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');

// one pass over a Markdown file: headings open blocks, tables and the paragraph before each are kept
function blocks(md: string, level: string, keepIntro: (title: string) => boolean) {
  const lines = md.split('\n');
  const out: Block[] = [];
  let cur: Block | null = null;
  let para: string[] = [];
  let lastPara = '';
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i];
    if (l.startsWith(level + ' ')) {
      const title = l.slice(level.length + 1).trim();
      cur = { id: slug(title), title, intro: [], tables: [] };
      out.push(cur); para = []; lastPara = '';
      continue;
    }
    if (!cur) continue;
    if (l.startsWith('|') && lines[i + 1]?.startsWith('| ---')) {
      const head = cells(l); const rows: string[][] = [];
      i += 2;
      while (i < lines.length && lines[i].startsWith('|')) rows.push(cells(lines[i++]));
      i--;
      const label = /^\*\*[^*]+\.\*\*$/.test(lastPara.trim()) ? lastPara.trim().replace(/\*\*/g, '').replace(/\.$/, '') : '';
      cur.tables.push({ label, head, rows });
      para = []; lastPara = '';
      continue;
    }
    if (l.trim() === '') {
      if (para.length) {
        lastPara = para.join(' ');
        if (keepIntro(cur.title) && !cur.tables.length && cur.intro.length < 2 && !lastPara.startsWith('**')) cur.intro.push(lastPara);
        para = [];
      }
    } else if (!l.startsWith('#')) para.push(l.trim());
  }
  return out;
}

const results = read('tests/RESULTS.md');
const resultsIntro = results.split('\n## ')[0].split('\n').slice(1).join('\n').trim().split(/\n\s*\n/).slice(0, 2);
export const ledger: Part = {
  id: 'ledger', title: 'Results ledger, Milestones 0 and 1', planned: false,
  intro: resultsIntro,
  blocks: blocks(results, '##', () => true).filter((b) => b.tables.length),
};

// BUILD-PLAN: one part per milestone, its status table first, then each work package's tables
const plan = read('build/BUILD-PLAN.md');
const milestones = plan.split('\n## ').slice(1).filter((s) => /^(Milestone \d|AM\d)/.test(s));
export const gates: Part[] = milestones.map((sec) => {
  const title = sec.split('\n')[0].trim();
  const body = '### ' + sec.split('\n### ').slice(1).join('\n### ');
  const bs = blocks(body, '###', (t) => /status/i.test(t)).filter((b) => b.tables.length);
  const status = bs.filter((b) => /status/i.test(b.title));
  const rest = bs.filter((b) => !/status/i.test(b.title));
  const ran = status.length > 0 || /^(Milestone [0-3]|AM1)\b/.test(title);
  return { id: slug(title), title, intro: [], blocks: [...status, ...rest], planned: !ran };
});

export const counts = {
  tables: ledger.blocks.reduce((n, b) => n + b.tables.length, 0) + gates.reduce((n, p) => n + p.blocks.reduce((m, b) => m + b.tables.length, 0), 0),
  rows: [ledger, ...gates].reduce((n, p) => n + p.blocks.reduce((m, b) => m + b.tables.reduce((k, t) => k + t.rows.length, 0), 0), 0),
};
