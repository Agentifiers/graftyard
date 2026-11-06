import React from 'react';
import { Mark } from '../brand/Mark';
import { C, FONT, TYPE, typeStyle, U } from '../brand/tokens';
import { holdTail } from '../lib/moves';

// What the two newer loops (GHES clients, chaos) share with the push loop's look (src/acts/PushLoop.tsx): the same
// window with the mark in its title bar, the same check, the same mono and card faces. The push loop keeps its own
// copies, so it renders exactly as before.

export const UI = TYPE.ui.size * U;
export const LH = UI * TYPE.ui.lead;
export const mono: React.CSSProperties = { ...typeStyle('ui'), fontFamily: FONT.mono, fontWeight: 450, letterSpacing: 0 };

/** An absolutely placed box at (x, y), by transform (layout offsets snap to whole pixels). */
export const at = (x: number, y: number): React.CSSProperties => ({ position: 'absolute', left: 0, top: 0, transform: `translate(${x.toFixed(2)}px, ${y.toFixed(2)}px)` });

/** A drawn check (0..1 of its stroke), in the accent. */
export const Check: React.FC<{ size: number; p?: number; color?: string }> = ({ size, p = 1, color = C.accent }) => (
  <svg width={size} height={size} viewBox="0 0 32 32" style={{ display: 'block', overflow: 'visible' }}>
    <path d="M5 17 L13 25 L28 8" fill="none" stroke={color} strokeWidth={4.2} strokeLinecap="round" strokeLinejoin="round" pathLength={1} strokeDasharray={1} strokeDashoffset={1 - p} />
  </svg>
);

/** A pending row's mark: a hairline ring that the check replaces. */
export const Ring: React.FC<{ size: number; o?: number }> = ({ size, o = 1 }) => (
  <svg width={size} height={size} viewBox="0 0 32 32" style={{ display: 'block', position: 'absolute', left: 0, top: 0, opacity: o }}>
    <circle cx={16} cy={16} r={11} fill="none" stroke={C.line} strokeWidth={3} />
  </svg>
);

/** The window the push loop's terminal stands in: the mark and the title in its bar, the body below. */
export const Window: React.FC<{ x: number; y: number; w: number; h: number; bar: number; pad: number; title: string; ui: string; children: React.ReactNode }> = ({ x, y, w, h, bar, pad, title, ui, children }) => (
  <div data-ui={ui} style={{ ...at(x, y), width: w, height: h, borderRadius: 22 * U, background: C.ink2, border: `${1.5 * U}px solid ${C.line}`, boxSizing: 'border-box', overflow: 'hidden' }}>
    <div style={{ height: bar, display: 'flex', alignItems: 'center', gap: 16 * U, padding: `0 ${pad}px`, borderBottom: `${1.5 * U}px solid ${C.line}` }}>
      <Mark size={34 * U} />
      <span style={{ ...typeStyle('ui'), color: C.mute }}>{title}</span>
    </div>
    {children}
  </div>
);

/**
 * A line's hold tail (lib/moves holdTail: the decaying tail of its rise, 0.8 -> 0.2 px a frame at 1080p, upward; about 30 px over a read), from
 * the frame it resolves to the frame it leaves: the read never stands still. Set it on a layer whose content does not
 * change while it is read (translate3d + willChange), so the glyphs move sub-pixel instead of snapping a pixel at a time.
 */
export const lineTail = (F: number, resolved: number, out: number): React.CSSProperties => ({
  transform: `translate3d(0, ${(-holdTail(Math.min(F, out), resolved, undefined, { v0: 0.8, vEnd: 0.2 })).toFixed(4)}px, 0)`,
  willChange: 'transform',
});

/**
 * The stage's creep: the product rises `px` at a constant rate from frame 0 to `end` (90 px over 10 s at 1080p is 0.3 px
 * a frame, 9 px/s: under the drift band of forensics WANDER busy_px, 15 px/s), so every hold keeps moving without a
 * word being moved while it is read; from `end` to `home` it travels back down with the scroll that resets the loop.
 * Offset at frame F: +px/2 at frame 0, -px/2 at `end`, +px/2 again from `home` on. Draw it with stageStyle().
 */
export const creepY = (F: number, end: number, home: number, px: number, ease: (t: number) => number) => {
  if (F < end) return (px / 2) * (1 - (2 * F) / end); // lint-ok: a constant ambient creep under the drift band
  const t = ease(Math.min(1, Math.max(0, (F - end) / (home - end))));
  return -px / 2 + px * t;
};
/** The stage as its own composited layer, so a sub-pixel creep moves sub-pixel instead of in whole-pixel stairs. */
export const stageStyle = (y: number): React.CSSProperties => ({ position: 'absolute', inset: 0, transform: `translate3d(0, ${y.toFixed(4)}px, 0)`, willChange: 'transform' });

/** A card face, as the push loop's site cards. */
export const cardFace: React.CSSProperties = { borderRadius: 22 * U, background: C.mist, border: `${1.5 * U}px solid ${C.line}`, boxSizing: 'border-box' };
