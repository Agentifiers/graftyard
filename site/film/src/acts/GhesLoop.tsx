import React from 'react';
import { AbsoluteFill, useCurrentFrame } from 'remotion';
import { C, FONT, SAFE, TYPE, typeStyle, U } from '../brand/tokens';
import { countTo } from '../fx/Roll';
import { TypeOn, type TypeTiming } from '../fx/TypeOn';
import { Words } from '../fx/Words';
import { E, mix, prog } from '../lib/anim';
import { phraseOnsets } from '../lib/cadence';
import { mixRgb } from '../lib/color';
import { fitSize, measureTracked } from '../lib/measure';
import { at, cardFace, Check, creepY, lineTail, stageStyle, LH, mono, Ring, UI, Window } from '../loops/parts';
import { CLIENTS, GIT_ROWS, TERMINAL } from '../ghes/content';
import { CLEAR_AT, CLEAR_DUR, copy, CUE, f60x, GIT_COUNT, TICKS, TOTAL, W } from '../ghes/timeline';

// One take. A person types `gh api meta --hostname git.example`; the host answers as GitHub Enterprise Server does,
// with installed_version 3.22.0, and the version lights in the accent. Three accent dots leave the version, one for each
// client card (gh, git-lfs, git): each runs right along the version's row, down the clear lane past the end of the
// reply, across the gap and into its card's status light, crossing no text, a short trail behind it that retracts into
// the card, and lands as that card's checks tick in, git's three versions
// counting clients.sh's 29 rows each. The line lands in the gap while the product steps back; the terminal scrolls to a
// fresh prompt and the cards return to pending, so the last frame flows into frame 0. Until the scroll the product creeps
// up 90 px (loops/parts creepY) and travels back down with it; the line keeps the tail of its rise (lineTail).

const TERM = { x: SAFE.x, y: 90 * U, w: W - 2 * SAFE.x, bar: 70 * U, pad: 44 * U, rows: 6 };
const BODY_TOP = TERM.y + TERM.bar + 22 * U;
const TERM_H = TERM.bar + 22 * U + TERM.rows * LH + 18 * U;
const lineTop = (i: number) => BODY_TOP + i * LH;

const CARD = { y: 690 * U, h: 315 * U, gap: 25 * U, pad: 30 * U, rowH: 46 * U, mark: 30 * U };
const CARD_W = (TERM.w - 2 * CARD.gap) / 3;
const cardX = (i: number) => TERM.x + i * (CARD_W + CARD.gap);
const rowTop = (r: number) => CARD.pad + LH + 12 * U + r * CARD.rowH;
/** Each card's status light, top right: where the version's dot lands. */
const DOT_R = 9 * U;
const statusAt = (i: number) => ({ x: cardX(i) + CARD_W - CARD.pad - DOT_R, y: CARD.y + CARD.pad + LH / 2 });

export const CMD_TYPE: TypeTiming = { start: CUE.type, kind: 'human', seed: 'gh api meta' };
const NEXT_TYPE: TypeTiming = { ...CMD_TYPE, start: CMD_TYPE.start + TOTAL };
/** The reply prints a row about every 3 frames, the way gh streams a short body. */
const REPLY_AT = phraseOnsets(CUE.reply, TERMINAL.reply.length, 3).map(Math.round);
const PROMPT_AT = CUE.lineOut + 4;
const SCROLL = { from: CUE.scroll, to: CUE.scroll + f60x(80) };
const SCROLL_ROWS = TERMINAL.reply.length + 1;
/** The version lights once its row is in. */
const LIGHT = { from: REPLY_AT[TERMINAL.version.row] + 6, to: REPLY_AT[TERMINAL.version.row] + 6 + f60x(24) };
/**
 * The dots: each leaves the version just in time to land on its card's status light 3 f before the card's first tick,
 * so none waits anywhere. The flights follow each route's length (about 1,370, 970 and 760 px at 1080p) at about 30 px
 * a frame on average, the push loop's ack dots' speed.
 */
const FLY_DUR = [44, 32, 25];
const FLY = [CUE.gh, CUE.lfs, CUE.git].map((cue, i) => ({ from: cue - 3 - FLY_DUR[i], to: cue - 3 }));
if (FLY[0].from < LIGHT.from) throw new Error('GhesLoop: the first dot would leave the version before it lights');
/** The stage's creep (px at 1080p): up 90 px until the scroll, back down with it. */
const CREEP = { px: 90 * U, home: CUE.scroll + f60x(96) };
const stageY = (F: number) => creepY(F, CUE.scroll, CREEP.home, CREEP.px, E.stdInOut);
/** The product steps back while the line is read. */
const back = (F: number) => prog(F, CUE.lineIn - 4, CUE.lineIn + f60x(28), E.softOut) * (1 - prog(F, CUE.lineOut, CUE.lineOut + f60x(30), E.softOut));

const SeenLine: React.FC<{ F: number }> = ({ F }) => {
  const c = copy('seen');
  const size = fitSize(c.text, W - 2 * SAFE.x, TYPE.line.size * U, TYPE.line.weight, TYPE.line.track);
  // centred in the gap as the stage stands mid-read (the stage creeps under it; the line keeps only its own tail)
  const cy = (TERM.y + TERM_H + CARD.y) / 2 + stageY((c.resolved + c.out) / 2);
  return (
    <div style={{ ...at(0, cy - (size * TYPE.line.lead) / 2), width: W, display: 'flex', justifyContent: 'center' }}>
      <div style={lineTail(F, c.resolved, c.out)}>
        <Words text={c.text} start={c.in} out={c.out} size={size} lead={TYPE.line.lead} frame={F} id="seen" style={{ ...typeStyle('line', size), color: C.ink }} />
      </div>
    </div>
  );
};

/** The version's characters on the reply row, for the light and the dots' start. */
const versionSpan = () => {
  const row = TERMINAL.reply[TERMINAL.version.row];
  const i = row.indexOf(TERMINAL.version.text);
  return { before: row.slice(0, i), text: TERMINAL.version.text, after: row.slice(i + TERMINAL.version.text.length) };
};

const Terminal: React.FC<{ F: number }> = ({ F }) => {
  const promptW = measureTracked(`${TERMINAL.prompt} `, UI, 450, 0, FONT.mono);
  const scrollY = -SCROLL_ROWS * LH * prog(F, SCROLL.from, SCROLL.to, E.stdInOut);
  const light = prog(F, LIGHT.from, LIGHT.to, E.glideOut);
  const v = versionSpan();
  const row = (i: number, o: number, children: React.ReactNode, key: string) => (
    <div key={key} style={{ ...at(TERM.pad, lineTop(i) - BODY_TOP + (1 - o) * 8 * U), height: LH, display: 'flex', alignItems: 'center', color: C.ink, opacity: o * Math.min(1, Math.max(0, 1 + (i * LH + scrollY) / (0.6 * LH))), ...mono }}>{children}</div>
  );
  return (
    <Window x={TERM.x} y={TERM.y} w={TERM.w} h={TERM_H} bar={TERM.bar} pad={TERM.pad} title={TERMINAL.title} ui="terminal">
      <div style={{ ...at(0, BODY_TOP - TERM.y), width: TERM.w, height: TERM_H - (BODY_TOP - TERM.y), overflow: 'hidden' }}>
        <div style={{ position: 'absolute', inset: 0, transform: `translateY(${scrollY.toFixed(2)}px)` }}>
          {row(0, 1, <>
            <span style={{ width: promptW, color: C.mute }}>{TERMINAL.prompt}</span>
            <TypeOn text={TERMINAL.command} frame={F} {...CMD_TYPE} role="ui" family={FONT.mono} caretOut={CUE.enter} id="cmd" style={mono} />
          </>, 'cmd')}
          {TERMINAL.reply.map((text, i) => {
            if (F < REPLY_AT[i]) return null;
            const o = prog(F, REPLY_AT[i], REPLY_AT[i] + f60x(16), E.glideOut);
            const body = i === TERMINAL.version.row
              ? <span style={{ color: C.mute }}>{v.before}<span style={{ color: mixRgb(C.ink, C.accent, light) }}>{v.text}</span>{v.after}</span>
              : <span style={{ color: C.mute }}>{text}</span>;
            return row(1 + i, o, <span data-text={`reply-${i}`} style={{ whiteSpace: 'pre' }}>{body}</span>, `r${i}`);
          })}
          {F >= PROMPT_AT && row(1 + TERMINAL.reply.length, prog(F, PROMPT_AT, PROMPT_AT + f60x(16), E.glideOut), <>
            <span style={{ width: promptW, color: C.mute }}>{TERMINAL.prompt}</span>
            <TypeOn text={TERMINAL.command} frame={F} {...NEXT_TYPE} role="ui" family={FONT.mono} id="prompt" style={mono} />
          </>, 'prompt')}
        </div>
      </div>
    </Window>
  );
};

const ClientCard: React.FC<{ i: number; F: number }> = ({ i, F }) => {
  const c = CLIENTS[i];
  const ticks = [TICKS.gh, TICKS.lfs, TICKS.git][i];
  const out = prog(F, CLEAR_AT[i], CLEAR_AT[i] + CLEAR_DUR, E.stdInOut);
  const lit = prog(F, FLY[i].to, FLY[i].to + f60x(16), E.glideOut) * (1 - out);
  const s = statusAt(i);
  const countW = measureTracked(`${GIT_ROWS}/${GIT_ROWS}`, UI, 450, 0, FONT.mono);
  return (
    <div data-ui={`client-${i}`} style={{ ...at(cardX(i), CARD.y), ...cardFace, width: CARD_W, height: CARD.h, borderColor: mixRgb(C.line, C.accent, 0.35 * lit) }}>
      <div style={{ ...at(CARD.pad, CARD.pad), height: LH, display: 'flex', alignItems: 'center' }}>
        <span style={{ ...typeStyle('ui'), color: mixRgb(C.mute, C.ink, lit) }}>{c.name}</span>
      </div>
      <div style={{ ...at(s.x - cardX(i) - DOT_R, s.y - CARD.y - DOT_R), width: 2 * DOT_R, height: 2 * DOT_R, borderRadius: DOT_R, border: `${2 * U}px solid ${C.line}`, boxSizing: 'border-box' }} />
      {c.rows.map((label, r) => {
        const t = ticks[r];
        const tick = prog(F, t, t + f60x(18), E.glideOut) * (1 - out);
        const counting = i === 2 && F >= t - GIT_COUNT && out < 1;
        const n = i === 2 ? countTo(F, { from: 0, to: GIT_ROWS, start: t - GIT_COUNT, dur: GIT_COUNT }) : 0;
        return (
          <div key={label} style={{ ...at(CARD.pad, rowTop(r)), width: CARD_W - 2 * CARD.pad, height: CARD.rowH, display: 'flex', alignItems: 'center' }}>
            <div style={{ position: 'relative', width: CARD.mark, height: CARD.mark, flex: 'none' }}>
              <Ring size={CARD.mark} o={1 - tick} />
              <Check size={CARD.mark} p={tick} />
            </div>
            <span style={{ ...(i === 0 || i === 1 ? mono : typeStyle('ui')), marginLeft: 16 * U, color: mixRgb(C.mute, C.ink, tick) }}>{label}</span>
            {counting && (
              <span data-text={`count-${r}`} style={{ ...mono, position: 'absolute', right: 0, width: countW, textAlign: 'right', color: mixRgb(C.mute, C.accent, tick), opacity: 1 - out }}>{`${n}/${GIT_ROWS}`}</span>
            )}
          </div>
        );
      })}
    </div>
  );
};

type Pt = { x: number; y: number };
/** A polyline with each inner corner rounded (radius r), sampled, with the cumulative length at each sample. */
const roundedPath = (pts: Pt[], r: number) => {
  const out: Pt[] = [pts[0]];
  for (let k = 1; k < pts.length - 1; k++) {
    const [a, c, b] = [pts[k - 1], pts[k], pts[k + 1]];
    const la = Math.hypot(c.x - a.x, c.y - a.y);
    const lb = Math.hypot(b.x - c.x, b.y - c.y);
    const ra = Math.min(r, la / 2);
    const rb = Math.min(r, lb / 2);
    const p0 = { x: c.x - ((c.x - a.x) / la) * ra, y: c.y - ((c.y - a.y) / la) * ra };
    const p2 = { x: c.x + ((b.x - c.x) / lb) * rb, y: c.y + ((b.y - c.y) / lb) * rb };
    for (let j = 0; j <= 12; j++) {
      const t = j / 12;
      out.push({ x: (1 - t) ** 2 * p0.x + 2 * (1 - t) * t * c.x + t * t * p2.x, y: (1 - t) ** 2 * p0.y + 2 * (1 - t) * t * c.y + t * t * p2.y });
    }
  }
  out.push(pts[pts.length - 1]);
  const len = [0];
  for (let k = 1; k < out.length; k++) len.push(len[k - 1] + Math.hypot(out[k].x - out[k - 1].x, out[k].y - out[k - 1].y));
  return { pts: out, len };
};
/** The point a share `p` (0..1) of the way along a sampled path. */
const along = (path: ReturnType<typeof roundedPath>, p: number): Pt => {
  const s = p * path.len[path.len.length - 1];
  let k = 1;
  while (k < path.len.length - 1 && path.len[k] < s) k++;
  const seg = path.len[k] - path.len[k - 1];
  const t = seg > 0 ? (s - path.len[k - 1]) / seg : 0;
  return { x: mix(path.pts[k - 1].x, path.pts[k].x, t), y: mix(path.pts[k - 1].y, path.pts[k].y, t) };
};

/**
 * Each dot's route, in clear space only: from just right of the version's row (after its comma) right to a lane past
 * the reply's longest row, down that lane below the last row with text on its right, then across the window's empty
 * lower rows and the gap to a point above its card's status light, and down into it.
 */
const routes = () => {
  const x0 = TERM.x + TERM.pad;
  const vRow = TERMINAL.reply[TERMINAL.version.row];
  const y0 = lineTop(1 + TERMINAL.version.row) + LH / 2;
  const longest = Math.max(...TERMINAL.reply.map((r) => measureTracked(r, UI, 450, 0, FONT.mono)));
  const lane = x0 + longest + 38 * U;
  const start = { x: x0 + measureTracked(vRow, UI, 450, 0, FONT.mono) + 12 * U, y: y0 };
  const clear = lineTop(1 + TERMINAL.reply.length - 2) + LH + 2 * U; // below the last row that reaches the lane
  const gapY = CARD.y - 40 * U;
  return [0, 1, 2].map((i) => {
    const s = statusAt(i);
    const pts = i === 2 ? [start, { x: lane, y: y0 }, s] : [start, { x: lane, y: y0 }, { x: lane, y: clear }, { x: s.x, y: gapY }, s];
    return roundedPath(pts, 40 * U);
  });
};

/** Frames the trail's tail runs behind the dot. */
const TRAIL_LAG = 9;
/** The route from share a to share b as SVG polyline points. */
const trailPoints = (path: ReturnType<typeof roundedPath>, a: number, b: number) => {
  const n = 24;
  const pts: string[] = [];
  for (let k = 0; k <= n; k++) {
    const p = along(path, mix(a, b, k / n));
    pts.push(`${p.x.toFixed(2)},${p.y.toFixed(2)}`);
  }
  return pts.join(' ');
};

/** The version's three dots: from "3.22.0" to each card's status light, where they stay lit until the cards clear. */
const Dots: React.FC<{ F: number }> = ({ F }) => {
  const paths = routes();
  return (
    <>
      {/* lint-ok: three dots carrying the version, not lines of text */}
      {FLY.map((fl, i) => {
        if (F < fl.from) return null;
        const out = prog(F, CLEAR_AT[i], CLEAR_AT[i] + CLEAR_DUR, E.stdInOut);
        if (out >= 1) return null;
        const t = prog(F, fl.from, fl.to, E.stdInOut);
        const { x, y } = along(paths[i], t);
        const r = mix(6 * U, DOT_R, t);
        const born = prog(F, fl.from, fl.from + 4, E.snapOut);
        // the trail: the route behind the dot, its tail following TRAIL_LAG frames later, so it retracts into the card
        const tail = prog(F, fl.from + TRAIL_LAG, fl.to + TRAIL_LAG, E.stdInOut);
        const trail = tail < t - 0.002 ? trailPoints(paths[i], tail, t) : '';
        return (
          <React.Fragment key={i}>
            {trail && (
              <svg width={W} height={1080 * U} style={{ position: 'absolute', left: 0, top: 0, overflow: 'visible' }}>
                <polyline points={trail} fill="none" stroke={C.accent} strokeWidth={3 * U} strokeLinecap="round" strokeLinejoin="round" opacity={0.45 * born} />
              </svg>
            )}
            <div style={{ position: 'absolute', left: 0, top: 0, width: 2 * r, height: 2 * r, borderRadius: r, background: C.accent, opacity: born * (1 - out), transform: `translate(${(x - r).toFixed(2)}px, ${(y - r).toFixed(2)}px)` }} />
          </React.Fragment>
        );
      })}
    </>
  );
};

export const GhesLoop: React.FC = () => {
  const F = useCurrentFrame();
  return (
    <AbsoluteFill style={{ background: C.paper, overflow: 'hidden' }}>
      <div style={stageStyle(stageY(F))}>
        <div style={{ position: 'absolute', inset: 0, opacity: 1 - 0.55 * back(F) }}>
          <Terminal F={F} />
          {CLIENTS.map((_, i) => <ClientCard key={i} i={i} F={F} />)}
          <Dots F={F} />
        </div>
      </div>
      <SeenLine F={F} />
    </AbsoluteFill>
  );
};
