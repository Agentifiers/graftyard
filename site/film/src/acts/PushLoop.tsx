import React from 'react';
import { AbsoluteFill, useCurrentFrame } from 'remotion';
import { Mark } from '../brand/Mark';
import { C, FONT, SAFE, TYPE, typeStyle, U } from '../brand/tokens';
import { QUORUM, SITES, TERMINAL } from '../content';
import { typeOn, TypeOn, type TypeTiming } from '../fx/TypeOn';
import { Words } from '../fx/Words';
import { E, mix, prog } from '../lib/anim';
import { fitSize, measureTracked } from '../lib/measure';
import { copy, CUE, f60x, TOTAL, W } from '../timeline';

// The whole loop is one take on a still camera, every state a function of the absolute frame. The entry is the one
// element carried throughout: `git push` is typed, the caret swells into the entry (a leaf-green pill), the pill
// travels down and becomes three copies, one per site. Europe and North America hold theirs in turn and the line
// lands centred between the terminal and the sites. Their acks converge back into the terminal's check, which prints
// the acknowledged line; Asia-Pacific catches up; the terminal scrolls to a fresh prompt and the sites clear, so the
// last frame flows into frame 0.

const ui = TYPE.ui;
const UI = ui.size * U;
const LH = UI * ui.lead;
const mono: React.CSSProperties = { ...typeStyle('ui'), fontFamily: FONT.mono, fontWeight: 450, letterSpacing: 0 };

const TERM = { x: 175 * U, y: 70 * U, w: 1000 * U, h: 290 * U, bar: 76 * U, pad: 44 * U };
const BODY_TOP = TERM.y + TERM.bar + 26 * U;
const lineTop = (i: number) => BODY_TOP + i * LH;
const lineMid = (i: number) => lineTop(i) + LH / 2;

const CARD = { w: 370 * U, h: 250 * U, gap: 26 * U, y: 700 * U, pad: 36 * U };
const CARD_X0 = (W - (3 * CARD.w + 2 * CARD.gap)) / 2;
const cardX = (i: number) => CARD_X0 + i * (CARD.w + CARD.gap);
const PILL = { w: 104 * U, h: 44 * U };
const dock = (i: number) => ({ x: cardX(i) + CARD.pad + PILL.w / 2, y: CARD.y + CARD.pad + PILL.h / 2 });

/** Where the entry waits before it splits: above the middle card. */
const FORK = { x: W / 2, y: CARD.y - 70 * U };
/** The check on the acknowledged line, where the acks converge. */
const CHECK = { size: 34 * U, x: TERM.x + TERM.pad, y: lineMid(1) };

/** The typed command (fx/TypeOn): a person at the keys, the first key just after frame 0. */
export const CMD_TYPE: TypeTiming = { start: CUE.type, kind: 'human', seed: 'git push' };
/** The fresh prompt waits for the next cycle's push: the same typing one loop later, so its caret is frame 0's. */
const NEXT_TYPE: TypeTiming = { ...CMD_TYPE, start: CMD_TYPE.start + TOTAL };
/** The acknowledgement the shell prints once the acks are in. */
export const ACK_TYPE: TypeTiming = { start: CUE.ack + 2, kind: 'stream' };

const SWELL = { from: CUE.enter, to: CUE.enter + f60x(20) };
const DESCEND = { from: SWELL.to + 2, to: CUE.split - 6 };
/** Each copy's flight from the fork to its site's dock: the two quorum sites land as they hold the entry. */
const FLIGHT: Record<string, { from: number; to: number }> = {
  weur: { from: CUE.split, to: CUE.heldWeur },
  enam: { from: CUE.split, to: CUE.heldEnam },
  apac: { from: CUE.split, to: CUE.lineIn }, // it arrives, but holds only after the ack
};
const ACK_FLIGHT = { from: CUE.ackGo, to: CUE.ack };
const SCROLL = { from: CUE.scroll, to: CUE.scroll + f60x(44) };
/** The sites clear left to right for the next entry. */
const CLEAR_AT = [CUE.clear, CUE.clear + 4, CUE.clear + 9];
const CLEAR_DUR = f60x(24);
const LAG = 0.42; // the copy that has not been held yet, dimmed

/** An absolutely placed box at (x, y), by transform (layout offsets snap to whole pixels). */
const at = (x: number, y: number): React.CSSProperties => ({ position: 'absolute', left: 0, top: 0, transform: `translate(${x.toFixed(2)}px, ${y.toFixed(2)}px)` });

const Pill: React.FC<{ x: number; y: number; w?: number; h?: number; o?: number }> = ({ x, y, w = PILL.w, h = PILL.h, o = 1 }) => (
  <div style={{ position: 'absolute', left: 0, top: 0, width: w, height: h, borderRadius: h / 2, background: C.accent, opacity: o, transform: `translate(${(x - w / 2).toFixed(2)}px, ${(y - h / 2).toFixed(2)}px)` }} />
);

/** A drawn check (0..1 of its stroke), in the accent. */
const Check: React.FC<{ size: number; p?: number }> = ({ size, p = 1 }) => (
  <svg width={size} height={size} viewBox="0 0 32 32" style={{ display: 'block', overflow: 'visible' }}>
    <path d="M5 17 L13 25 L28 8" fill="none" stroke={C.accent} strokeWidth={4.2} strokeLinecap="round" strokeLinejoin="round" pathLength={1} strokeDasharray={1} strokeDashoffset={1 - p} />
  </svg>
);

/** The line of the script over the product, centred in the gap between the terminal and the sites. */
const HeldLine: React.FC<{ F: number }> = ({ F }) => {
  const c = copy('held');
  const size = fitSize(c.text, W - 2 * SAFE.x, TYPE.line.size * U, TYPE.line.weight, TYPE.line.track);
  const cy = (TERM.y + TERM.h + CARD.y) / 2;
  return (
    <div style={{ ...at(0, cy - (size * TYPE.line.lead) / 2), width: W, display: 'flex', justifyContent: 'center' }}>
      <Words text={c.text} start={c.in} out={c.out} size={size} lead={TYPE.line.lead} frame={F} id="held" style={{ ...typeStyle('line', size), color: C.ink }} />
    </div>
  );
};

const Terminal: React.FC<{ F: number }> = ({ F }) => {
  const prefix = `${TERMINAL.prompt} `;
  const promptW = measureTracked(prefix, UI, 450, 0, FONT.mono);
  const scrollY = -2 * LH * prog(F, SCROLL.from, SCROLL.to, E.stdInOut);
  const checkP = prog(F, CUE.ack, CUE.ack + f60x(16), E.glideOut);
  const row = (i: number, children: React.ReactNode) => (
    <div style={{ ...at(TERM.pad, lineTop(i) - BODY_TOP), height: LH, display: 'flex', alignItems: 'center', color: C.ink, opacity: Math.min(1, Math.max(0, 1 + (i * LH + scrollY) / (0.6 * LH))), ...mono }}>{children}</div>
  );
  return (
    <div data-ui="terminal" style={{ ...at(TERM.x, TERM.y), width: TERM.w, height: TERM.h, borderRadius: 22 * U, background: C.ink2, border: `${1.5 * U}px solid ${C.line}`, boxSizing: 'border-box', overflow: 'hidden' }}>
      <div style={{ height: TERM.bar, display: 'flex', alignItems: 'center', gap: 16 * U, padding: `0 ${TERM.pad}px`, borderBottom: `${1.5 * U}px solid ${C.line}` }}>
        <Mark size={34 * U} />
        <span style={{ ...typeStyle('ui'), color: C.mute }}>{TERMINAL.title}</span>
      </div>
      <div style={{ ...at(0, BODY_TOP - TERM.y), width: TERM.w, height: TERM.y + TERM.h - BODY_TOP, overflow: 'hidden' }}>
        <div style={{ position: 'absolute', inset: 0, transform: `translateY(${scrollY.toFixed(2)}px)` }}>
          {row(0, <>
            <span style={{ width: promptW, color: C.mute }}>{TERMINAL.prompt}</span>
            <TypeOn text={TERMINAL.command} frame={F} {...CMD_TYPE} role="ui" family={FONT.mono} caretOut={CUE.enter} id="cmd" style={mono} />
          </>)}
          {F >= CUE.ack && row(1, <>
            <span style={{ width: promptW, display: 'flex' }}><Check size={CHECK.size} p={checkP} /></span>
            <TypeOn text={TERMINAL.ack} frame={F} {...ACK_TYPE} role="ui" family={FONT.mono} caret={false} id="ack" style={mono} />
          </>)}
          {F >= CUE.prompt && row(2, <>
            <span style={{ width: promptW, color: C.mute }}>{TERMINAL.prompt}</span>
            <TypeOn text={TERMINAL.command} frame={F} {...NEXT_TYPE} role="ui" family={FONT.mono} id="prompt" style={mono} />
          </>)}
        </div>
      </div>
    </div>
  );
};

const SiteCard: React.FC<{ i: number; F: number }> = ({ i, F }) => {
  const s = SITES[i];
  const out = prog(F, CLEAR_AT[i], CLEAR_AT[i] + CLEAR_DUR, E.softIn);
  const tick = prog(F, s.heldAt, s.heldAt + f60x(18), E.glideOut);
  const d = dock(i);
  return (
    <div data-ui={`site-${s.code}`} style={{ ...at(cardX(i), CARD.y), width: CARD.w, height: CARD.h, borderRadius: 22 * U, background: C.mist, border: `${1.5 * U}px solid ${C.line}`, boxSizing: 'border-box' }}>
      <div style={{ position: 'absolute', left: CARD.pad, bottom: CARD.pad - 6 * U, display: 'flex', flexDirection: 'column', gap: 6 * U }}>
        <span style={{ ...typeStyle('ui'), color: C.ink }}>{s.region}</span>
        <span style={{ ...mono, color: C.mute }}>{s.code}</span>
      </div>
      <div style={{ ...at(d.x - cardX(i) - PILL.w / 2, d.y - CARD.y - PILL.h / 2), width: PILL.w, height: PILL.h, borderRadius: PILL.h / 2, border: `${2 * U}px solid ${C.line}`, boxSizing: 'border-box' }} />
      <div style={{ ...at(CARD.pad + PILL.w + 20 * U, d.y - CARD.y - LH / 2 + (1 - tick) * 10 * U), height: LH, display: 'flex', alignItems: 'center', gap: 10 * U, opacity: tick * (1 - out) }}>
        <Check size={30 * U} />
        <span style={{ ...typeStyle('ui'), color: C.accent }}>held</span>
      </div>
    </div>
  );
};

/** The entry: born from the caret, down to the fork, then one copy per site. */
const Entry: React.FC<{ F: number }> = ({ F }) => {
  if (F < CUE.enter) return null;
  if (F < CUE.split) {
    const c = typeOn(TERMINAL.command, F, { ...CMD_TYPE, role: 'ui', family: FONT.mono });
    const x0 = TERM.x + TERM.pad + measureTracked(`${TERMINAL.prompt} `, UI, 450, 0, FONT.mono) + c.caretX + 0.03 * UI;
    const sw = prog(F, SWELL.from, SWELL.to, E.glideOut);
    const w = mix(0.055 * UI, PILL.w, sw);
    const h = mix(LH * 0.9, PILL.h, sw);
    const start = { x: x0 + w / 2 + 6 * U * sw, y: lineMid(0) };
    const t = prog(F, DESCEND.from, DESCEND.to, E.stdInOut);
    return <Pill x={mix(start.x, FORK.x, t)} y={mix(start.y, FORK.y, t)} w={w} h={h} />;
  }
  return (
    <>
      {SITES.map((s, i) => {
        const fl = FLIGHT[s.code];
        const t = prog(F, fl.from, fl.to, E.stdInOut);
        const d = dock(i);
        const lag = s.heldAt > CUE.ack ? mix(1, LAG, t) + (1 - LAG) * prog(F, s.heldAt, s.heldAt + f60x(18), E.glideOut) : 1;
        const out = prog(F, CLEAR_AT[i], CLEAR_AT[i] + CLEAR_DUR, E.softIn);
        if (out >= 1) return null;
        return <Pill key={s.code} x={mix(FORK.x, d.x, t)} y={mix(FORK.y, d.y, t)} o={lag * (1 - out)} />;
      })}
    </>
  );
};

/** The quorum's acks: from the two sites that hold the entry to the terminal's check, where they become it. */
const Acks: React.FC<{ F: number }> = ({ F }) => {
  if (F < ACK_FLIGHT.from || F >= CUE.ack + f60x(10)) return null;
  const t = prog(F, ACK_FLIGHT.from, ACK_FLIGHT.to, E.stdInOut);
  const gone = prog(F, CUE.ack, CUE.ack + f60x(10), E.glideOut);
  const r = 14 * U * (1 - 0.7 * gone);
  return (
    <>
      {/* lint-ok: the acks are two dots in flight, not lines of text */}
      {QUORUM.map((s) => {
        const d = dock(SITES.indexOf(s));
        const x = mix(d.x, CHECK.x + CHECK.size / 2, t);
        const y = mix(d.y, CHECK.y, t);
        return <div key={s.code} style={{ position: 'absolute', left: 0, top: 0, width: 2 * r, height: 2 * r, borderRadius: r, background: C.accent, opacity: 1 - gone, transform: `translate(${(x - r).toFixed(2)}px, ${(y - r).toFixed(2)}px)` }} />;
      })}
    </>
  );
};

/** The product steps back while the line is read, and comes forward for the acks. */
const backAt = (F: number) => prog(F, CUE.lineIn, CUE.lineIn + f60x(24), E.softOut) * (1 - prog(F, CUE.lineOut, CUE.ackGo + 2, E.softOut));

export const PushLoop: React.FC = () => {
  const F = useCurrentFrame();
  const dim = 1 - 0.4 * backAt(F);
  return (
    <AbsoluteFill style={{ background: C.paper, overflow: 'hidden' }}>
      <div style={{ position: 'absolute', inset: 0, opacity: dim }}>
        <Terminal F={F} />
        {SITES.map((s, i) => <SiteCard key={s.code} i={i} F={F} />)}
      </div>
      <HeldLine F={F} />
      <Entry F={F} />
      <Acks F={F} />
    </AbsoluteFill>
  );
};
