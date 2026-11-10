import React from 'react';
import { AbsoluteFill, useCurrentFrame } from 'remotion';
import { C, FONT, SAFE, TYPE, typeStyle, U } from '../brand/tokens';
import { TypeOn, type TypeTiming } from '../fx/TypeOn';
import { Words } from '../fx/Words';
import { E, prog } from '../lib/anim';
import { phraseOnsets } from '../lib/cadence';
import { mixRgb } from '../lib/color';
import { fitSize, measureTracked } from '../lib/measure';
import { at, cardFace, Check, creepY, lineTail, stageStyle, LH, mono, Ring, Window } from '../loops/parts';
import { PR, QUEUE, REVIEW, TERMINAL } from '../agents/content';
import { CLEAR_AT, CLEAR_DUR, copy, CUE, f60x, TOTAL, W } from '../agents/timeline';

// One take. An agent's terminal (agent · session 7f3c) types `git push origin HEAD:graftyard/7f3c/fix-session` and
// the host reports the new branch, its tag lit in the accent. The agent types `gh pr merge 412`; the host refuses in
// the refusal colour, and under the terminal PR #412's graftyard/agent-gate count, 0 of 1 approvals from human writers,
// takes the same colour. mara approves (her card ticks); the gate reads 1 of 1 in the accent and ticks; the merge queue
// takes #412 and ticks merged. The line lands in the gap while the product steps back; the terminal scrolls to a fresh
// prompt and the cards return to pending, so the last frame flows into frame 0. Until the scroll the product creeps up
// 90 px (loops/parts creepY) and travels back down with it; the line keeps the tail of its rise (lineTail).

/** The terminal's text: the house mono a step under the label size, so the agent's branch fits a row. */
const CMD = 36 * U;
const cmd: React.CSSProperties = { ...mono, fontSize: CMD };

const TERM = { x: SAFE.x, y: 90 * U, w: W - 2 * SAFE.x, bar: 70 * U, pad: 44 * U, rows: 6 };
const BODY_TOP = TERM.y + TERM.bar + 22 * U;
const TERM_H = TERM.bar + 22 * U + TERM.rows * LH + 18 * U;
/** The terminal's rows: the push, its two-row reply, the merge, the refusal, then the fresh prompt. */
const ROW = { push: 0, reply: 1, merge: 1 + TERMINAL.pushReply.length, refusal: 2 + TERMINAL.pushReply.length, next: 3 + TERMINAL.pushReply.length };

const CARD = { y: 690 * U, h: 315 * U, gap: 25 * U, pad: 30 * U, rowH: 46 * U, mark: 30 * U };
/** PR #412 on the left, wide enough for the gate's count; the reviewer and the merge queue stacked on the right. */
const PR_W = 790 * U;
const SIDE = { x: TERM.x + PR_W + CARD.gap, w: TERM.w - PR_W - CARD.gap, h: (CARD.h - 20 * U) / 2, gap: 20 * U, pad: 26 * U };

export const PUSH_TYPE: TypeTiming = { start: CUE.type, kind: 'human', seed: 'git push' };
export const MERGE_TYPE: TypeTiming = { start: CUE.mergeType, kind: 'human', seed: 'gh pr merge' };
const NEXT_TYPE: TypeTiming = { ...PUSH_TYPE, start: PUSH_TYPE.start + TOTAL };
/** The push's reply prints a row about every 3 frames; the next prompt follows it. */
const REPLY_AT = phraseOnsets(CUE.reply, TERMINAL.pushReply.length, 3).map(Math.round);
const MERGE_PROMPT_AT = REPLY_AT[REPLY_AT.length - 1] + 5;
if (MERGE_PROMPT_AT >= CUE.mergeType) throw new Error('AgentsLoop: the merge prompt would show after its typing starts');
const PROMPT_AT = CUE.lineOut + 4;
const SCROLL = { from: CUE.scroll, to: CUE.scroll + f60x(80) };
/** The fresh prompt scrolls up to the first row, where frame 0 has it. */
const SCROLL_ROWS = ROW.next;
/** The new branch's tag lights once its row is in. */
const LIGHT = { from: REPLY_AT[TERMINAL.landed.row] + 6, to: REPLY_AT[TERMINAL.landed.row] + 6 + f60x(24) };
/**
 * The stage's creep (px at 1080p): up 90 px until the scroll, back down with it. It is home before the PR card clears
 * (CLEAR_AT.pr), so the card's text changes on a still stage: re-drawn while the stage still travelled sub-pixel, the
 * whole stage stepped a pixel in one frame (forensics: a spike at f345).
 */
const CREEP = { px: 90 * U, home: CUE.scroll + f60x(64) };
if (CREEP.home > CLEAR_AT.pr) throw new Error('AgentsLoop: the stage must be home before the PR card clears');
const stageY = (F: number) => creepY(F, CUE.scroll, CREEP.home, CREEP.px, E.stdInOut);
/** The product steps back while the line is read. */
const back = (F: number) => prog(F, CUE.lineIn - 4, CUE.lineIn + f60x(28), E.softOut) * (1 - prog(F, CUE.lineOut, CUE.lineOut + f60x(30), E.softOut));
type Card = keyof typeof CLEAR_AT;
/** A card's return to pending (0 = as the take left it, 1 = pending again). */
const clearOf = (k: Card, F: number) => prog(F, CLEAR_AT[k], CLEAR_AT[k] + CLEAR_DUR, E.stdInOut);
/** A state arriving at `cue`, undone by its card's clear. */
const on = (F: number, cue: number, k: Card, dur = f60x(18)) => prog(F, cue, cue + dur, E.glideOut) * (1 - clearOf(k, F));

const DecideLine: React.FC<{ F: number }> = ({ F }) => {
  const c = copy('decide');
  const size = fitSize(c.text, W - 2 * SAFE.x, TYPE.line.size * U, TYPE.line.weight, TYPE.line.track);
  // centred in the gap as the stage stands mid-read (the stage creeps under it; the line keeps only its own tail)
  const cy = (TERM.y + TERM_H + CARD.y) / 2 + stageY((c.resolved + c.out) / 2);
  return (
    <div style={{ ...at(0, cy - (size * TYPE.line.lead) / 2), width: W, display: 'flex', justifyContent: 'center' }}>
      <div style={lineTail(F, c.resolved, c.out)}>
        <Words text={c.text} start={c.in} out={c.out} size={size} lead={TYPE.line.lead} frame={F} id="decide" style={{ ...typeStyle('line', size), color: C.ink }} />
      </div>
    </div>
  );
};

/** The reply row that reports the push, with the new branch's tag in the accent. */
const landedSpan = () => {
  const row = TERMINAL.pushReply[TERMINAL.landed.row];
  const i = row.indexOf(TERMINAL.landed.text);
  return { before: row.slice(0, i), text: TERMINAL.landed.text, after: row.slice(i + TERMINAL.landed.text.length) };
};

const Terminal: React.FC<{ F: number }> = ({ F }) => {
  const promptW = measureTracked(`${TERMINAL.prompt} `, CMD, 450, 0, FONT.mono);
  const p = prog(F, SCROLL.from, SCROLL.to, E.stdInOut);
  const scrollY = -SCROLL_ROWS * LH * p;
  // the finished session fades out as the scroll gets under way (and each row as it nears the window's top edge), so
  // no row is cut by the edge while it can still be read; the fresh prompt rises into the first row
  const oldO = Math.min(1, Math.max(0, (0.45 - p) / 0.35));
  const light = prog(F, LIGHT.from, LIGHT.to, E.glideOut);
  const v = landedSpan();
  const row = (i: number, o: number, children: React.ReactNode, key: string) => (
    <div key={key} style={{ ...at(TERM.pad, i * LH + (1 - o) * 8 * U), height: LH, display: 'flex', alignItems: 'center', color: C.ink, opacity: o * (i < ROW.next ? oldO : 1) * Math.min(1, Math.max(0, 1 + (i * LH + scrollY) / (0.6 * LH))), ...cmd }}>{children}</div>
  );
  const fadeIn = (from: number) => prog(F, from, from + f60x(16), E.glideOut);
  const prompt = <span style={{ width: promptW, color: C.mute }}>{TERMINAL.prompt}</span>;
  return (
    <Window x={TERM.x} y={TERM.y} w={TERM.w} h={TERM_H} bar={TERM.bar} pad={TERM.pad} title={TERMINAL.title} ui="terminal">
      <div style={{ ...at(0, BODY_TOP - TERM.y), width: TERM.w, height: TERM_H - (BODY_TOP - TERM.y), overflow: 'hidden' }}>
        <div style={{ position: 'absolute', inset: 0, transform: `translateY(${scrollY.toFixed(2)}px)` }}>
          {row(ROW.push, 1, <>
            {prompt}
            <TypeOn text={TERMINAL.push} frame={F} {...PUSH_TYPE} role="ui" size={CMD} family={FONT.mono} caretOut={CUE.enter} id="push" style={cmd} />
          </>, 'push')}
          {TERMINAL.pushReply.map((text, i) => {
            if (F < REPLY_AT[i]) return null;
            const body = i === TERMINAL.landed.row
              ? <span style={{ color: C.mute }}>{v.before}<span style={{ color: mixRgb(C.mute, C.accent, light) }}>{v.text}</span>{v.after}</span>
              : <span style={{ color: C.mute }}>{text}</span>;
            return row(ROW.reply + i, fadeIn(REPLY_AT[i]), <span data-text={`push-reply-${i}`} style={{ whiteSpace: 'pre' }}>{body}</span>, `r${i}`);
          })}
          {F >= MERGE_PROMPT_AT && row(ROW.merge, fadeIn(MERGE_PROMPT_AT), <>
            {prompt}
            <TypeOn text={TERMINAL.merge} frame={F} {...MERGE_TYPE} role="ui" size={CMD} family={FONT.mono} caretOut={CUE.mergeEnter} id="merge" style={cmd} />
          </>, 'merge')}
          {F >= CUE.refuse && row(ROW.refusal, fadeIn(CUE.refuse), <span data-text="refusal" style={{ whiteSpace: 'pre', color: C.refuse }}>{TERMINAL.refusal}</span>, 'refusal')}
          {F >= PROMPT_AT && row(ROW.next, fadeIn(PROMPT_AT), <>
            {prompt}
            <TypeOn text={TERMINAL.push} frame={F} {...NEXT_TYPE} role="ui" size={CMD} family={FONT.mono} id="prompt" style={cmd} />
          </>, 'next')}
        </div>
      </div>
    </Window>
  );
};

/** A row's mark: the pending ring, replaced by the drawn check as `p` goes 0 -> 1. */
const Mark: React.FC<{ p: number }> = ({ p }) => (
  <div style={{ position: 'relative', width: CARD.mark, height: CARD.mark, flex: 'none' }}>
    <Ring size={CARD.mark} o={1 - p} />
    <Check size={CARD.mark} p={p} />
  </div>
);

/** PR #412: its branch, and graftyard/agent-gate with its count of approvals from human writers. */
const PrCard: React.FC<{ F: number }> = ({ F }) => {
  const out = clearOf('pr', F);
  // the refusal tints the count; just before the gate passes the tint settles to ink, the count steps to 1 on a whole
  // frame (as the host's status does) and turns to the accent with the check, so "1 of 1" never shows in the refusal
  // colour; halfway through the clear it steps back to 0 and returns to muted
  const seen = prog(F, CUE.refuse, CUE.refuse + f60x(24), E.glideOut) * (1 - out);
  const settle = prog(F, CUE.gate - 8, CUE.gate, E.stdInOut);
  const pass = on(F, CUE.gate, 'pr');
  const have = F >= CUE.gate && out < 0.5 ? PR.need : 0;
  const merged = on(F, CUE.merged, 'pr', f60x(24));
  const countColor = mixRgb(mixRgb(mixRgb(C.mute, C.refuse, seen), C.ink, settle * (1 - out)), C.accent, pass);
  return (
    <div data-ui="pr" style={{ ...at(TERM.x, CARD.y), ...cardFace, width: PR_W, height: CARD.h, borderColor: mixRgb(C.line, C.accent, 0.35 * merged) }}>
      <div style={{ ...at(CARD.pad, CARD.pad), height: LH, display: 'flex', alignItems: 'center' }}>
        <span style={{ ...typeStyle('ui'), color: C.ink }}>{PR.title}</span>
      </div>
      <div style={{ ...at(CARD.pad, CARD.pad + LH + 4 * U), height: LH, display: 'flex', alignItems: 'center' }}>
        <span style={{ ...cmd, color: C.mute }}>{PR.branch}</span>
      </div>
      <div style={{ ...at(CARD.pad, CARD.pad + 2 * LH + 22 * U), height: CARD.rowH, display: 'flex', alignItems: 'center' }}>
        <Mark p={pass} />
        <span style={{ ...cmd, marginLeft: 16 * U, color: mixRgb(C.mute, C.ink, seen) }}>{PR.check}</span>
      </div>
      <div style={{ ...at(CARD.pad + CARD.mark + 16 * U, CARD.pad + 2 * LH + 22 * U + CARD.rowH + 2 * U), height: LH, display: 'flex', alignItems: 'center' }}>
        <span data-text="gate-count" style={{ ...typeStyle('ui'), color: countColor }}>{`${have} of ${PR.need} ${PR.approvals}`}</span>
      </div>
    </div>
  );
};

/**
 * One of the two stacked cards on the right: a title, then one row with its mark, a name and a state on the right. The
 * name stands from frame 0, or with `nameAt` arrives then (the queue takes #412 once the gate passes) and leaves with
 * the clear.
 */
const SideCard: React.FC<{ i: number; k: Card; F: number; title: string; name: string; nameAt?: number; state: string; at: number }> = ({ i, k, F, title, name, nameAt, state, at: cue }) => {
  const done = on(F, cue, k);
  const shown = nameAt === undefined ? 1 : on(F, nameAt, k, f60x(20));
  const y = CARD.y + (i - 1) * (SIDE.h + SIDE.gap);
  return (
    <div data-ui={`side-${i}`} style={{ ...at(SIDE.x, y), ...cardFace, width: SIDE.w, height: SIDE.h, borderColor: mixRgb(C.line, C.accent, 0.35 * done) }}>
      <div style={{ ...at(SIDE.pad, SIDE.pad - 6 * U), height: LH, display: 'flex', alignItems: 'center' }}>
        <span style={{ ...typeStyle('ui'), color: C.mute }}>{title}</span>
      </div>
      <div style={{ ...at(SIDE.pad, SIDE.h - SIDE.pad - CARD.rowH), width: SIDE.w - 2 * SIDE.pad, height: CARD.rowH, display: 'flex', alignItems: 'center' }}>
        <Mark p={done} />
        <span style={{ ...typeStyle('ui'), marginLeft: 16 * U, color: mixRgb(C.mute, C.ink, done), opacity: shown }}>{name}</span>
        <span data-text={`state-${i}`} style={{ ...typeStyle('ui'), position: 'absolute', right: 0, color: C.accent, opacity: done }}>{state}</span>
      </div>
    </div>
  );
};

export const AgentsLoop: React.FC = () => {
  const F = useCurrentFrame();
  return (
    <AbsoluteFill style={{ background: C.paper, overflow: 'hidden' }}>
      <div style={stageStyle(stageY(F))}>
        <div style={{ position: 'absolute', inset: 0, opacity: 1 - 0.55 * back(F) }}>
          <Terminal F={F} />
          <PrCard F={F} />
          <SideCard i={1} k="review" F={F} title={REVIEW.title} name={REVIEW.who} state={REVIEW.state} at={CUE.approve} />
          <SideCard i={2} k="queue" F={F} title={QUEUE.title} name={QUEUE.pr} nameAt={CUE.gate + 6} state={QUEUE.state} at={CUE.merged} />
        </div>
      </div>
      <DecideLine F={F} />
    </AbsoluteFill>
  );
};
