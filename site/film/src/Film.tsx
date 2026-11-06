import React from 'react';
import { AbsoluteFill } from 'remotion';
import { PushLoop } from './acts/PushLoop';
import { C } from './brand/tokens';
import { Audit } from './lib/Audit';
import { FontGate } from './lib/FontGate';
import type { SafeSpec } from './lib/safe';

/** audit: draw and log the layout audit; safe: its safe zone (a lib/safe.ts preset or insets; default by aspect). */
export type FilmProps = { audit?: boolean; safe?: SafeSpec };

// The loop: one take on a still camera (src/acts/PushLoop.tsx), muted. The timeline's ACT only names its bars.
export const Film: React.FC<FilmProps> = ({ audit = false, safe }) => (
  <FontGate>
    <AbsoluteFill style={{ background: C.paper }}>
      <PushLoop />
      {audit && <Audit safe={safe} />}
    </AbsoluteFill>
  </FontGate>
);
