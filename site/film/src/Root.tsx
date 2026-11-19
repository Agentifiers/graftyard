import React from 'react';
import { Composition, Folder, Still } from 'remotion';
import { U } from './brand/tokens';
import { Film, FilmProps } from './Film';
import { FilmEvery, everyMetadata, FilmSub, subMetadata } from './FilmSub';
import { TOTAL as AGENTS_TOTAL } from './agents/timeline';
import { TOTAL as CHAOS_TOTAL } from './chaos/timeline';
import { TOTAL as DURABILITY_TOTAL } from './durability/timeline';
import { TOTAL as GHES_TOTAL } from './ghes/timeline';
import { AgentsFilm, ChaosFilm, DurabilityFilm, GhesFilm } from './LoopFilms';
import { BrandAvatar, BrandHeader, BrandLockup, BrandMark, LogoSheet, LogoWallStill, MarkSizes, MarkStill } from './Stills';
import { FPS, H, TOTAL, W } from './timeline';

export const RemotionRoot: React.FC = () => (
  <>
    {/* The film. Render with scripts/render.sh (muted picture + ffmpeg mux + sync check). */}
    <Composition id="Film" component={Film} durationInFrames={TOTAL} fps={FPS} width={W} height={H} defaultProps={{ audit: false } as FilmProps} />
    {/* The film as sub-frames for the motion-blurred master (render.sh --blur passes out/samples.json). */}
    <Composition
      id="FilmSub"
      component={FilmSub}
      durationInFrames={TOTAL}
      fps={FPS}
      width={W}
      height={H}
      defaultProps={{ groups: [] as number[] }}
      calculateMetadata={subMetadata}
    />
    {/* The film sampled for a dev round: every 4th frame (--props='{"step":4,"offset":2}' for the others). */}
    <Composition id="FilmEvery" component={FilmEvery} durationInFrames={TOTAL} fps={FPS} width={W} height={H} defaultProps={{ step: 4, offset: 0 }} calculateMetadata={everyMetadata} />
    {/* The newer loops (src/LoopFilms.tsx): npm run loop:ghes, npm run loop:chaos, npm run loop:agents, npm run loop:durability. */}
    <Composition id="Ghes" component={GhesFilm} durationInFrames={GHES_TOTAL} fps={FPS} width={W} height={H} defaultProps={{ audit: false } as FilmProps} />
    <Composition id="Chaos" component={ChaosFilm} durationInFrames={CHAOS_TOTAL} fps={FPS} width={W} height={H} defaultProps={{ audit: false } as FilmProps} />
    <Composition id="Agents" component={AgentsFilm} durationInFrames={AGENTS_TOTAL} fps={FPS} width={W} height={H} defaultProps={{ audit: false } as FilmProps} />
    <Composition id="Durability" component={DurabilityFilm} durationInFrames={DURABILITY_TOTAL} fps={FPS} width={W} height={H} defaultProps={{ audit: false } as FilmProps} />

    <Folder name="Stills">
      <Still id="Mark16" component={MarkStill} width={16} height={16} defaultProps={{ size: 16, surface: 'paper' as const }} />
      <Still id="MarkLarge" component={MarkStill} width={W} height={H} defaultProps={{ size: 640 * U, surface: 'paper' as const }} />
      <Still id="MarkSizes" component={MarkSizes} width={W} height={H} />
      {/* the real marks the film fetched (scripts/logos.mjs) in the kit's logo parts */}
      <Still id="KitLogos" component={LogoSheet} width={W} height={H} defaultProps={{ logos: [] as string[] }} />
      <Still id="KitLogoWall" component={LogoWallStill} width={W} height={H} defaultProps={{ logos: [] as string[], frame: 120, ground: 'light' as const, ring: false }} />
    </Folder>

    {/* The brand as files, for a brand the film invented: bash scripts/brand-kit.sh renders these. */}
    <Folder name="Brand">
      <Still id="BrandMark" component={BrandMark} defaultProps={{ size: 1024, ground: 'light' as const }} calculateMetadata={({ props }) => ({ width: props.size, height: props.size })} />
      <Still id="BrandLockup" component={BrandLockup} width={2400} height={800} defaultProps={{ ground: 'light' as const }} />
      <Still id="BrandAvatar" component={BrandAvatar} width={400} height={400} defaultProps={{ ground: 'light' as const }} />
      <Still id="BrandHeader" component={BrandHeader} width={1500} height={500} defaultProps={{ ground: 'light' as const }} />
    </Folder>
  </>
);
