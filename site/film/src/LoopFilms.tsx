import React from 'react';
import { AbsoluteFill } from 'remotion';
import { AgentsLoop } from './acts/AgentsLoop';
import { ChaosLoop } from './acts/ChaosLoop';
import { DurabilityLoop } from './acts/DurabilityLoop';
import { GhesLoop } from './acts/GhesLoop';
import { COPY as AGENTS_COPY } from './agents/timeline';
import { C } from './brand/tokens';
import { COPY as CHAOS_COPY } from './chaos/timeline';
import { COPY as DURABILITY_COPY } from './durability/timeline';
import { COPY as GHES_COPY } from './ghes/timeline';
import type { FilmProps } from './Film';
import { Audit } from './lib/Audit';
import { FilmProvider } from './lib/film';
import { FontGate } from './lib/FontGate';

// The newer loops, beside the push loop's Film (src/Film.tsx, unchanged). Each has its own timeline and copy, given
// to the primitives through the film context (lib/film.ts), so the push loop's COPY stays its own.

export const GhesFilm: React.FC<FilmProps> = ({ audit = false, safe }) => (
  <FontGate>
    <FilmProvider value={{ copy: GHES_COPY }}>
      <AbsoluteFill style={{ background: C.paper }}>
        <GhesLoop />
        {audit && <Audit safe={safe} />}
      </AbsoluteFill>
    </FilmProvider>
  </FontGate>
);

export const ChaosFilm: React.FC<FilmProps> = ({ audit = false, safe }) => (
  <FontGate>
    <FilmProvider value={{ copy: CHAOS_COPY }}>
      <AbsoluteFill style={{ background: C.paper }}>
        <ChaosLoop />
        {audit && <Audit safe={safe} />}
      </AbsoluteFill>
    </FilmProvider>
  </FontGate>
);

export const AgentsFilm: React.FC<FilmProps> = ({ audit = false, safe }) => (
  <FontGate>
    <FilmProvider value={{ copy: AGENTS_COPY }}>
      <AbsoluteFill style={{ background: C.paper }}>
        <AgentsLoop />
        {audit && <Audit safe={safe} />}
      </AbsoluteFill>
    </FilmProvider>
  </FontGate>
);

export const DurabilityFilm: React.FC<FilmProps> = ({ audit = false, safe }) => (
  <FontGate>
    <FilmProvider value={{ copy: DURABILITY_COPY }}>
      <AbsoluteFill style={{ background: C.paper }}>
        <DurabilityLoop />
        {audit && <Audit safe={safe} />}
      </AbsoluteFill>
    </FilmProvider>
  </FontGate>
);
