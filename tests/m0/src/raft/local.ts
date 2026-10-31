// Local harness only (wrangler dev): three member classes in one script stand in for the three site scripts. For
// M0-WP4, `RELEASE_A`, `READS_B`, `BREAK_KIND_C` and the like give one site its own release variables.
import { RaftMember, type MemberEnv } from "./member.ts";
import { RaftEdge } from "./edge.ts";
export { FaultControl } from "../faults.ts";
export { SiteHealth } from "./health.ts";
const at = (site: "a" | "b" | "c") => class extends RaftMember {
  constructor(ctx: DurableObjectState, env: MemberEnv) {
    const e = env as MemberEnv & Record<string, string | undefined>, up = site.toUpperCase();
    super(ctx, { ...env, SITE: site, RELEASE: e[`RELEASE_${up}`] ?? env.RELEASE, READS: e[`READS_${up}`] ?? env.READS,
      WRITES: e[`WRITES_${up}`] ?? env.WRITES, BREAK_KIND: e[`BREAK_KIND_${up}`] ?? env.BREAK_KIND });
  }
};
export const MemberA = at("a"), MemberB = at("b"), MemberC = at("c");
export default RaftEdge;
