import React from 'react';
import { AbsoluteFill, useCurrentFrame } from 'remotion';
import { C, FONT, SAFE, TYPE, typeStyle, U } from '../brand/tokens';
import { countTo } from '../fx/Roll';
import { Words } from '../fx/Words';
import { E, mix, prog } from '../lib/anim';
import { mixRgb } from '../lib/color';
import { fitSize, measureTracked } from '../lib/measure';
import { at, cardFace, creepY, lineTail, stageStyle, LH, mono, Window } from '../loops/parts';
import { ACKED, down, leaderAt, LOST, RESETS, SITES, TRANSFERS, WINDOW, WRITE_DUR, WRITE_EVERY, WRITE_PHASE } from '../chaos/content';
import { copy, CUE, f60x, RESET_DOWN, TOTAL, TRANSFER_DUR, W } from '../chaos/timeline';

// One take. A window names the run (lin24b, 24 h) and counts its acknowledged writes while the three site cards take
// faults: a write leaves the leader's card for the two others every third of a second, never stopping. Asia-Pacific's
// member is reset (its card flashes, its member goes down and comes back); leadership moves from North America to
// Europe (the leader marker travels); North America's member is reset; the counter lands on the run's 904,294 with
// "lost" still 0, and the line lands while the product steps back. Leadership moves back, Europe's member is reset, and
// the window scrolls to a fresh counter, so the last frame flows into frame 0. Until the scroll the product creeps up 90 px
// (loops/parts creepY) and travels back down with it; the line keeps the tail of its rise (lineTail).

const WIN = { y: 90 * U, bar: 70 * U, pad: 44 * U };
const ROWS = 2;
const WIN_H = WIN.bar + 22 * U + ROWS * LH + 22 * U;
const BODY_TOP = WIN.y + WIN.bar + 22 * U;

const CARD = { w: 370 * U, h: 250 * U, gap: 26 * U, y: 700 * U, pad: 36 * U };
const CARD_X0 = (W - (3 * CARD.w + 2 * CARD.gap)) / 2;
/** The window stands over the card row, its edges on the row's. */
const WIN_X = CARD_X0;
const WIN_W = 3 * CARD.w + 2 * CARD.gap;
const cardX = (i: number) => CARD_X0 + i * (CARD.w + CARD.gap);
/** The leader marker's slot on a card (top left), as the push loop's pill dock. */
const PILL = { w: 104 * U, h: 44 * U };
const dock = (i: number) => ({ x: cardX(i) + CARD.pad + PILL.w / 2, y: CARD.y + CARD.pad + PILL.h / 2 });

const COUNT = { from: CUE.count, to: CUE.counted };
const SCROLL = { from: CUE.scroll, to: CUE.scroll + f60x(80) };
/** The stage's creep (px at 1080p): up 90 px until the scroll, back down with it. */
const CREEP = { px: 90 * U, home: CUE.scroll + f60x(96) };
const stageY = (F: number) => creepY(F, CUE.scroll, CREEP.home, CREEP.px, E.stdInOut);
const back = (F: number) => prog(F, CUE.lineIn - 4, CUE.lineIn + f60x(28), E.softOut) * (1 - prog(F, CUE.lineOut, CUE.lineOut + f60x(30), E.softOut));

const LostLine: React.FC<{ F: number }> = ({ F }) => {
  const c = copy('lost');
  const size = fitSize(c.text, W - 2 * SAFE.x, TYPE.line.size * U, TYPE.line.weight, TYPE.line.track);
  // centred in the gap as the stage stands mid-read (the stage creeps under it; the line keeps only its own tail)
  const cy = (WIN.y + WIN_H + CARD.y) / 2 + stageY((c.resolved + c.out) / 2);
  return (
    <div style={{ ...at(0, cy - (size * TYPE.line.lead) / 2), width: W, display: 'flex', justifyContent: 'center' }}>
      <div style={lineTail(F, c.resolved, c.out)}>
        <Words text={c.text} start={c.in} out={c.out} size={size} lead={TYPE.line.lead} frame={F} id="lost" style={{ ...typeStyle('line', size), color: C.ink }} />
      </div>
    </div>
  );
};

const fmt = (n: number) => n.toLocaleString('en-US');

/** The window: the run's name in the bar, its acknowledged writes counting to the audited figure, and "lost" at 0. */
const RunWindow: React.FC<{ F: number }> = ({ F }) => {
  const n = countTo(F, { from: 0, to: ACKED, start: COUNT.from, dur: COUNT.to - COUNT.from, ease: E.settleOut });
  const landed = prog(F, CUE.counted, CUE.counted + f60x(20), E.glideOut);
  const p = prog(F, SCROLL.from, SCROLL.to, E.stdInOut);
  const scrollY = -ROWS * LH * p;
  // one run is readable at a time: the finished run fades out as the scroll gets under way and the fresh one fades in
  // only once it is gone, so the old "lost" row never stands above the new "acknowledged writes" row
  const runO = (i: number) => Math.min(1, Math.max(0, i < ROWS ? (0.55 - p) / 0.3 : (p - 0.55) / 0.4));
  const numW = measureTracked(fmt(ACKED), TYPE.ui.size * U, 450, 0, FONT.mono);
  const row = (i: number, label: string, value: string, color: string, key: string) => (
    <div key={key} style={{ ...at(WIN.pad, i * LH), width: WIN_W - 2 * WIN.pad, height: LH, display: 'flex', alignItems: 'center', justifyContent: 'space-between', opacity: runO(i) * Math.min(1, Math.max(0, 1 + (i * LH + scrollY) / (0.6 * LH))) }}>
      <span style={{ ...typeStyle('ui'), color: C.mute }}>{label}</span>
      <span data-text={key} style={{ ...mono, width: numW, textAlign: 'right', color }}>{value}</span>
    </div>
  );
  return (
    <Window x={WIN_X} y={WIN.y} w={WIN_W} h={WIN_H} bar={WIN.bar} pad={WIN.pad} title={`${WINDOW.title}  ${WINDOW.run}`} ui="run">
      <div style={{ ...at(0, BODY_TOP - WIN.y), width: WIN_W, height: ROWS * LH, overflow: 'hidden' }}>
        <div style={{ position: 'absolute', inset: 0, transform: `translateY(${scrollY.toFixed(2)}px)` }}>
          {row(0, WINDOW.acked, fmt(n), mixRgb(C.ink, C.accent, landed), 'acked')}
          {row(1, WINDOW.lost, String(LOST), C.accent, 'lost-n')}
          {/* the next run's counter, waiting below: what frame 0 shows */}
          {row(2, WINDOW.acked, '0', C.ink, 'acked-next')}
          {row(3, WINDOW.lost, String(LOST), C.accent, 'lost-next')}
        </div>
      </div>
    </Window>
  );
};

/** A member reset: the flash on the hit, the member down, back at RESET_DOWN. 0 = up, 1 = fully down. */
const downAt = (i: number, F: number) => {
  let v = 0;
  for (const r of RESETS) {
    if (r.site !== i) continue;
    const goes = prog(F, r.at, r.at + 8, E.glideOut);
    const back = prog(F, r.at + RESET_DOWN - 6, r.at + RESET_DOWN + f60x(28), E.stdInOut);
    v = Math.max(v, goes * (1 - back));
  }
  return v;
};
const flashAt = (i: number, F: number) => {
  let v = 0;
  for (const r of RESETS) if (r.site === i) v = Math.max(v, F >= r.at ? 1 - prog(F, r.at, r.at + f60x(40), E.glideOut) : 0);
  return v;
};

const SiteCard: React.FC<{ i: number; F: number }> = ({ i, F }) => {
  const s = SITES[i];
  const d = downAt(i, F);
  const flash = flashAt(i, F);
  // the label follows the marker: during a transfer no card says leader, the new one does once the marker lands
  const w = leadW(i, F);
  // the status is the member's own label: reset while it is down, else leader or follower (crossfaded over a few frames)
  // reset never shares the slot: the standing label is gone by d = 0.35 and reset only starts there (no ghost tail of
  // "follower" beside the shorter word)
  const dIn = Math.min(1, Math.max(0, (d - 0.35) / 0.65));
  const dOut = Math.min(1, d / 0.35);
  const states: [string, string, number][] = [['reset', C.ink, dIn], ['leader', C.accent, (1 - dOut) * w], ['follower', C.mute, (1 - dOut) * (1 - w)]];
  const slot = dock(i);
  return (
    <div data-ui={`site-${s.code}`} style={{ ...at(cardX(i), CARD.y), ...cardFace, width: CARD.w, height: CARD.h, background: mixRgb(C.mist, C.ink2, d), borderColor: mixRgb(C.line, C.ink, 0.6 * flash) }}>
      <div style={{ position: 'absolute', inset: 0, borderRadius: 22 * U, background: C.ink, opacity: 0.07 * flash }} />
      <div style={{ ...at(slot.x - cardX(i) - PILL.w / 2, slot.y - CARD.y - PILL.h / 2), width: PILL.w, height: PILL.h, borderRadius: PILL.h / 2, border: `${2 * U}px solid ${C.line}`, boxSizing: 'border-box' }} />
      <div style={{ ...at(CARD.pad + PILL.w + 20 * U, slot.y - CARD.y - LH / 2), height: LH, display: 'flex', alignItems: 'center' }}>
        {states.map(([label, color, o]) => (o > 0.001 ? <span key={label} data-text={o > 0.5 ? `status-${s.code}` : undefined} style={{ ...typeStyle('ui'), color, opacity: o, position: 'absolute', left: 0 }}>{label}</span> : null))}
      </div>
      <div style={{ position: 'absolute', left: CARD.pad, bottom: CARD.pad - 6 * U, display: 'flex', flexDirection: 'column', gap: 6 * U, opacity: 1 - 0.45 * d }}>
        <span style={{ ...typeStyle('ui'), color: C.ink }}>{s.region}</span>
        <span style={{ ...mono, color: C.mute }}>{s.code}</span>
      </div>
    </div>
  );
};

/** The leader marker: the accent pill in its site's slot, travelling to the next leader on each transfer. */
const Leader: React.FC<{ F: number }> = ({ F }) => {
  let x = dock(leaderAt(0)).x;
  let lift = 0;
  for (const t of TRANSFERS) {
    if (F < t.at) break;
    // it lifts clear of the cards' top edge first, travels, then settles into the new slot: it never crosses a label
    const p = prog(F, t.at + 6, t.at + TRANSFER_DUR - 6, E.stdInOut);
    x = mix(dock(t.from).x, dock(t.to).x, p);
    lift = 100 * U * (prog(F, t.at, t.at + 9, E.glideOut) - prog(F, t.at + TRANSFER_DUR - 9, t.at + TRANSFER_DUR, E.stdInOut));
  }
  const y = dock(0).y - lift;
  return <div style={{ position: 'absolute', left: 0, top: 0, width: PILL.w, height: PILL.h, borderRadius: PILL.h / 2, background: C.accent, transform: `translate(${(x - PILL.w / 2).toFixed(2)}px, ${(y - PILL.h / 2).toFixed(2)}px)` }} />;
};

/** The write stream: a dot leaves the leader for each other site every WRITE_EVERY frames; a dot to a member that is
 * down waits at its card's edge and is taken when the member comes back. It repeats exactly over the loop. */
const Writes: React.FC<{ F: number }> = ({ F }) => {
  const dots: React.ReactNode[] = [];
  const r = 8 * U;
  for (let k = -3; k * WRITE_EVERY + WRITE_PHASE <= F; k++) {
    const t0 = k * WRITE_EVERY + WRITE_PHASE;
    if (F - t0 > WRITE_DUR + 2 || transferringAt(t0)) continue;
    const from = leaderAt(t0);
    for (let i = 0; i < 3; i++) {
      if (i === from || down(from, t0)) continue;
      const p = prog(F, t0, t0 + WRITE_DUR, E.stdInOut);
      const a = dock(from);
      const b = dock(i);
      const held = down(i, t0 + WRITE_DUR);
      const end = held ? mix(a.x, b.x, 0.82) : b.x;
      const x = mix(a.x, end, p);
      const y = a.y - Math.sin(Math.PI * p) * 70 * U;
      const o = prog(F, t0, t0 + 3, E.snapOut) * (1 - prog(F, t0 + WRITE_DUR - 3, t0 + WRITE_DUR + 2, E.softIn));
      if (o <= 0.001) continue;
      dots.push(<div key={`${k}-${i}`} style={{ position: 'absolute', left: 0, top: 0, width: 2 * r, height: 2 * r, borderRadius: r, background: C.accent, opacity: 0.85 * o, transform: `translate(${(x - r).toFixed(2)}px, ${(y - r).toFixed(2)}px)` }} />);
    }
  }
  return <>{dots}</>;
};
/** How much site i is labelled leader: the old leader lets go as the marker lifts, the new one takes it as it lands. */
const leadW = (i: number, F: number) => {
  let w = leaderAt(0) === i ? 1 : 0;
  for (const t of TRANSFERS) {
    if (F < t.at) break;
    w = i === t.from ? 1 - prog(F, t.at + 2, t.at + 8, E.stdInOut) : i === t.to ? prog(F, t.at + TRANSFER_DUR - 6, t.at + TRANSFER_DUR, E.stdInOut) : 0;
  }
  return w;
};
const transferringAt = (f: number) => TRANSFERS.some((t) => f >= t.at - WRITE_DUR && f < t.at + TRANSFER_DUR);

export const ChaosLoop: React.FC = () => {
  const F = useCurrentFrame();
  return (
    <AbsoluteFill style={{ background: C.paper, overflow: 'hidden' }}>
      <div style={stageStyle(stageY(F))}>
        <div style={{ position: 'absolute', inset: 0, opacity: 1 - 0.55 * back(F) }}>
          <RunWindow F={F} />
          {SITES.map((s, i) => <SiteCard key={s.code} i={i} F={F} />)}
          <Writes F={F} />
          <Leader F={F} />
        </div>
      </div>
      <LostLine F={F} />
    </AbsoluteFill>
  );
};
void TOTAL;
