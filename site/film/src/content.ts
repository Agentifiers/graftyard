// The product's own strings and the replica table the loop is drawn from. One push, three sites; the order in which
// they hold the entry is the data, so every paused frame agrees with it: the terminal acknowledges only after two
// sites in two regions hold the entry, and the third catches up after the ack.
import { CUE } from './timeline';

export const TERMINAL = {
  title: 'Graftyard',
  prompt: '$',
  command: 'git push',
  ack: 'acknowledged in 214 ms', // inside the measured commit p50 range at one entry in flight, 172 to 370 ms (design/DESIGN.md, "Scale")
};

export type Site = { region: string; code: string; heldAt: number };

/** Left to right as the cards stand. */
export const SITES: Site[] = [
  { region: 'North America', code: 'enam', heldAt: CUE.heldEnam },
  { region: 'Europe', code: 'weur', heldAt: CUE.heldWeur },
  { region: 'Asia-Pacific', code: 'apac', heldAt: CUE.heldApac },
];

/** The sites whose acks the terminal waits for: those that hold the entry before it acknowledges. */
export const QUORUM = SITES.filter((s) => s.heldAt < CUE.ack).sort((a, b) => a.heldAt - b.heldAt);
if (QUORUM.length !== 2) throw new Error(`content: the ack needs two sites holding the entry, got ${QUORUM.length}`);
