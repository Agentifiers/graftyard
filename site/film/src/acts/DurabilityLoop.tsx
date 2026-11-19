import React from 'react';
import { AbsoluteFill, useCurrentFrame } from 'remotion';
import { C, FONT, SAFE, TYPE, typeStyle, U } from '../brand/tokens';
import { Words } from '../fx/Words';
import { E, mix, prog } from '../lib/anim';
import { mixRgb } from '../lib/color';
import { fitSize, measureTracked } from '../lib/measure';
import { at, cardFace, Check, creepY, lineTail, stageStyle, LH, mono, UI, Window } from '../loops/parts';
import { catchUp, DARK, LAND, LEADER, LOST, push, PUSH_EVERY, SITES, STATUS, WINDOW } from '../durability/content';
import { copy, CUE, f60x, TOTAL, W } from '../durability/timeline';

// One take. A window shows the latest push acknowledged and the two sites that hold it, over a "lost" row that reads 0
// all loop long. Under it the three site cards: North America leads (the accent pill), Europe and Asia-Pacific follow.
// A push leaves the leader on every beat, a dot to each follower; the window's check redraws when the first other site
// holds it, Europe while it is up. Europe's site goes dark: its card dims to "offline", the copy in flight to it is
// dropped, and from the next push on the window reads "held on enam, apac", with no pause. The line lands while the
// product steps back. Europe comes back, catches up from the leader's log (a quick run of dots from the leader) and is
// a follower again; the window reads "enam, weur" once Europe holds a new push, so the last frame flows into frame 0.
// The product creeps up 90 px (loops/parts creepY) until the line is read and settles back down while it is relit.

const CARD = { w: 370 * U, h: 250 * U, gap: 26 * U, y: 700 * U, pad: 36 * U };
const CARD_X0 = (W - (3 * CARD.w + 2 * CARD.gap)) / 2;
const cardX = (i: number) => CARD_X0 + i * (CARD.w + CARD.gap);
/**
 * The leader marker's slot on a card (top left), as the push and chaos loops' pill dock; a follower's holds a push. It
 * is narrower than theirs so the longest status, "catching up", fits inside the card's padding beside it.
 */
const PILL = { w: 72 * U, h: 40 * U };
const LABEL_X = CARD.pad + PILL.w + 16 * U;
const dock = (i: number) => ({ x: cardX(i) + CARD.pad + PILL.w / 2, y: CARD.y + CARD.pad + PILL.h / 2 });

/** The window stands over the card row, its edges on the row's: one acknowledged row, a rule, the "lost" row. */
const WIN = { x: CARD_X0, w: 3 * CARD.w + 2 * CARD.gap, y: 150 * U, bar: 70 * U, pad: 44 * U };
const RULE = 14 * U;
const BODY_TOP = WIN.y + WIN.bar + 20 * U;
const LOST_TOP = BODY_TOP + LH + RULE;
const WIN_H = LOST_TOP + LH + 20 * U - WIN.y;

/** The stage's creep (px at 1080p): up 90 px until the line's read is over, back down while the product is relit. */
const CREEP = { px: 90 * U, end: CUE.lineOut, home: CUE.lineOut + f60x(90) };
const stageY = (F: number) => creepY(F, CREEP.end, CREEP.home, CREEP.px, E.stdInOut);
/** The product steps back while the line is read. */
const back = (F: number) => prog(F, CUE.lineIn - 4, CUE.lineIn + f60x(28), E.softOut) * (1 - prog(F, CUE.lineOut, CUE.lineOut + f60x(30), E.softOut));

/** The loop's frame for an absolute frame, and the loop's first frame (the push stream runs on across the seam). */
const wrap = (F: number) => ((F % TOTAL) + TOTAL) % TOTAL;
const loopStart = (F: number) => F - wrap(F);
/** The newest push index that has left the leader by frame F. */
const pushIndex = (F: number) => Math.floor((F - CUE.push) / PUSH_EVERY);
/** The newest push acknowledged by frame F. */
const lastAcked = (F: number) => {
  for (let k = pushIndex(F); ; k--) if (push(k).ack <= F) return push(k);
};

/** How dark Europe's site is: 0 up, 1 dark (a fast fall as every call fails, a slower relight as it comes back). */
const darkness = (F: number) => prog(F, CUE.dark, CUE.dark + 8, E.glideOut) * (1 - prog(F, CUE.back, CUE.back + f60x(30), E.stdInOut));

const DarkLine: React.FC<{ F: number }> = ({ F }) => {
  const c = copy('dark');
  const size = fitSize(c.text, W - 2 * SAFE.x, TYPE.line.size * U, TYPE.line.weight, TYPE.line.track);
  // centred in the gap as the stage stands mid-read (the stage creeps under it; the line keeps only its own tail)
  const cy = (WIN.y + WIN_H + CARD.y) / 2 + stageY((c.resolved + c.out) / 2);
  return (
    <div style={{ ...at(0, cy - (size * TYPE.line.lead) / 2), width: W, display: 'flex', justifyContent: 'center' }}>
      <div style={lineTail(F, c.resolved, c.out)}>
        <Words text={c.text} start={c.in} out={c.out} size={size} lead={TYPE.line.lead} frame={F} id="dark" style={{ ...typeStyle('line', size), color: C.ink }} />
      </div>
    </div>
  );
};

/**
 * The window: the latest push acknowledged, with its check redrawn on each acknowledgement, the two sites that hold it,
 * and "lost" at 0. When the second site changes, the old name leaves before the new one comes in (both four letters
 * of mono in one place: no longer word ever shows past a shorter one).
 */
const AckWindow: React.FC<{ F: number }> = ({ F }) => {
  const p = lastAcked(F);
  const prev = push(pushIndex(p.t0) - 1);
  const next = push(pushIndex(p.t0) + 1);
  // the check: drawn as the push is acknowledged, eased back to a faint mark just before the next one lands
  const draw = prog(F, p.ack, p.ack + f60x(18), E.glideOut);
  const fade = 1 - 0.75 * prog(F, next.ack - 5, next.ack, E.stdInOut);
  const swap = prev.quorum !== p.quorum ? prog(F, p.ack, p.ack + 10, E.stdInOut) : 1;
  const names: [string, number][] = prev.quorum !== p.quorum
    ? [[SITES[prev.quorum].code, Math.max(0, 1 - 2 * swap)], [SITES[p.quorum].code, Math.max(0, 2 * swap - 1)]]
    : [[SITES[p.quorum].code, 1]];
  const siteW = measureTracked('weur', UI, 450, 0, FONT.mono);
  const numW = measureTracked(String(LOST), UI, 450, 0, FONT.mono);
  return (
    <Window x={WIN.x} y={WIN.y} w={WIN.w} h={WIN_H} bar={WIN.bar} pad={WIN.pad} title={WINDOW.title} ui="acks">
      <div style={{ ...at(WIN.pad, BODY_TOP - WIN.y), width: WIN.w - 2 * WIN.pad, height: LH, display: 'flex', alignItems: 'center' }}>
        <div style={{ opacity: fade }}><Check size={30 * U} p={draw} /></div>
        <span style={{ ...typeStyle('ui'), marginLeft: 16 * U, color: C.ink }}>{WINDOW.acked}</span>
        <span data-text="ack-sites" style={{ ...mono, position: 'absolute', right: siteW, color: C.mute }}>{`${WINDOW.heldOn} `}<span style={{ color: C.ink }}>{`${SITES[LEADER].code}, `}</span></span>
        {names.map(([code, o]) => (o > 0.001 ? <span key={code} style={{ ...mono, position: 'absolute', right: 0, width: siteW, color: C.ink, opacity: o }}>{code}</span> : null))}
      </div>
      <div style={{ ...at(WIN.pad, LOST_TOP - WIN.y - RULE / 2), width: WIN.w - 2 * WIN.pad, height: 1.5 * U, background: C.line }} />
      <div style={{ ...at(WIN.pad, LOST_TOP - WIN.y), width: WIN.w - 2 * WIN.pad, height: LH, display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <span style={{ ...typeStyle('ui'), color: C.mute }}>{WINDOW.lost}</span>
        <span data-text="lost" style={{ ...mono, width: numW, textAlign: 'right', color: C.accent }}>{String(LOST)}</span>
      </div>
    </Window>
  );
};

/**
 * A follower's status: follower, offline while its site is dark, catching up once it is back, follower again. One
 * label at a time: each leaves before the next comes in, so no longer word shows past a shorter one.
 */
const statusOf = (i: number, F: number): [string, string, number][] => {
  if (i === LEADER) return [[STATUS.leader, C.accent, 1]];
  if (i !== DARK) return [[STATUS.follower, C.mute, 1]];
  const out = (from: number) => prog(F, from, from + 6, E.stdInOut);
  const inn = (from: number) => prog(F, from + 6, from + 12, E.stdInOut);
  return [
    [STATUS.follower, C.mute, Math.max(1 - out(CUE.dark), inn(CUE.caughtUp))],
    [STATUS.dark, C.mute, inn(CUE.dark) * (1 - out(CUE.back))],
    [STATUS.catchingUp, C.ink, inn(CUE.back) * (1 - out(CUE.caughtUp))],
  ];
};

/** When a follower's slot last took an entry (absolute frame): a push it held, or for Europe a catch-up entry. */
const lastHeld = (i: number, F: number) => {
  let last = -Infinity;
  for (let k = pushIndex(F) - 3; k <= pushIndex(F); k++) {
    const p = push(k);
    const t = p.t0 + LAND[i];
    if ((i !== DARK || p.heldWeur) && t <= F) last = Math.max(last, t);
  }
  if (i === DARK) for (const c of catchUp()) if (loopStart(F) + c.land <= F) last = Math.max(last, loopStart(F) + c.land);
  return last;
};

const SiteCard: React.FC<{ i: number; F: number }> = ({ i, F }) => {
  const s = SITES[i];
  const d = i === DARK ? darkness(F) : 0;
  const slot = dock(i);
  const t = lastHeld(i, F);
  const held = i === LEADER || !Number.isFinite(t) ? 0 : 1 - prog(F, t, t + 12, E.glideOut);
  return (
    <div data-ui={`site-${s.code}`} style={{ ...at(cardX(i), CARD.y), ...cardFace, width: CARD.w, height: CARD.h, background: mixRgb(C.mist, C.paper, d), borderColor: mixRgb(C.line, C.ink2, d) }}>
      <div style={{ ...at(slot.x - cardX(i) - PILL.w / 2, slot.y - CARD.y - PILL.h / 2), width: PILL.w, height: PILL.h, borderRadius: PILL.h / 2, border: `${2 * U}px solid ${mixRgb(C.line, C.accent, 0.8 * held)}`, background: mixRgb(C.mist, C.accent, 0.22 * held), boxSizing: 'border-box', opacity: 1 - 0.6 * d }} />
      <div style={{ ...at(LABEL_X, slot.y - CARD.y - LH / 2), height: LH, display: 'flex', alignItems: 'center' }}>
        {statusOf(i, F).map(([label, color, o]) => (o > 0.001 ? <span key={label} data-text={o > 0.5 ? `status-${s.code}` : undefined} style={{ ...typeStyle('ui'), color, opacity: o, position: 'absolute', left: 0 }}>{label}</span> : null))}
      </div>
      <div style={{ position: 'absolute', left: CARD.pad, bottom: CARD.pad - 6 * U, display: 'flex', flexDirection: 'column', gap: 6 * U, opacity: 1 - 0.6 * d }}>
        <span style={{ ...typeStyle('ui'), color: C.ink }}>{s.region}</span>
        <span style={{ ...mono, color: C.mute }}>{s.code}</span>
      </div>
    </div>
  );
};

/** The leader marker: the accent pill in North America's slot, all loop long. */
const Leader: React.FC = () => {
  const p = dock(LEADER);
  return <div style={{ position: 'absolute', left: 0, top: 0, width: PILL.w, height: PILL.h, borderRadius: PILL.h / 2, background: C.accent, transform: `translate(${(p.x - PILL.w / 2).toFixed(2)}px, ${(p.y - PILL.h / 2).toFixed(2)}px)` }} />;
};

/**
 * One dot from the leader's slot toward a follower's, arcing over the labels and the cards between them: it lifts
 * steeply off the leader's pill and drops steeply into the follower's. A dropped copy (to a site that went dark while
 * it flew) fades out on its way, short of the dark card.
 */
const dot = (key: string, F: number, from: number, land: number, to: number, dropAt?: number) => {
  const r = 8 * U;
  const p = prog(F, from, land, E.stdInOut);
  const lost = dropAt === undefined ? 0 : prog(F, dropAt, dropAt + 6, E.softIn);
  const o = prog(F, from, from + 3, E.snapOut) * (1 - prog(F, land - 2, land + 2, E.softIn)) * (1 - lost);
  if (o <= 0.001) return null;
  const a = dock(LEADER);
  const b = dock(to);
  const x = mix(a.x, b.x, p);
  const y = a.y - Math.sin(Math.PI * p) ** 0.35 * (to === 2 ? 90 : 72) * U;
  return <div key={key} style={{ position: 'absolute', left: 0, top: 0, width: 2 * r, height: 2 * r, borderRadius: r, background: C.accent, opacity: 0.9 * o, transform: `translate(${(x - r).toFixed(2)}px, ${(y - r).toFixed(2)}px)` }} />;
};

/** Each push's dots: one to each follower it is sent to (Europe only while it is current), and the catch-up run. */
const Pushes: React.FC<{ F: number }> = ({ F }) => {
  const out: React.ReactNode[] = [];
  for (let k = pushIndex(F) - 2; k <= pushIndex(F); k++) {
    const p = push(k);
    if (p.sentWeur) out.push(dot(`w${k}`, F, p.t0, p.t0 + LAND[1], 1, p.heldWeur ? undefined : loopStart(p.t0) + CUE.dark));
    out.push(dot(`a${k}`, F, p.t0, p.t0 + LAND[2], 2));
  }
  for (const [n, c] of catchUp().entries()) out.push(dot(`c${n}`, F, loopStart(F) + c.from, loopStart(F) + c.land, 1));
  return <>{out}</>;
};

export const DurabilityLoop: React.FC = () => {
  const F = useCurrentFrame();
  return (
    <AbsoluteFill style={{ background: C.paper, overflow: 'hidden' }}>
      <div style={stageStyle(stageY(F))}>
        <div style={{ position: 'absolute', inset: 0, opacity: 1 - 0.55 * back(F) }}>
          <AckWindow F={F} />
          {SITES.map((s, i) => <SiteCard key={s.code} i={i} F={F} />)}
          <Pushes F={F} />
          <Leader />
        </div>
      </div>
      <DarkLine F={F} />
    </AbsoluteFill>
  );
};
