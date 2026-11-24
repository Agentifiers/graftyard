// The Workflows script (`build/BUILD-PLAN.md`, "Workflows script"; M6-WP1): `ReplaceMember`'s and
// `RestorePlacement`'s coordinator, group and expiry Workflows, and the handler `graft-ops` reaches over its
// `WORKFLOWS` service binding (all in `move.mjs`). No public route: `deploy.mjs` turns `workers_dev` off.
import { WorkflowEntrypoint } from "cloudflare:workers";
import { coordinate, expire, group, handle } from "./move.mjs";

const withSites = (env) => ({ ...env, sites: JSON.parse(env.GRAFTYARD_SITES) });

export class MoveCoordinator extends WorkflowEntrypoint {
  run(event, step) {
    return coordinate(withSites(this.env), event, step);
  }
}

export class MoveGroup extends WorkflowEntrypoint {
  run(event, step) {
    return group(withSites(this.env), event, step);
  }
}

export class MoveExpire extends WorkflowEntrypoint {
  run(event, step) {
    return expire(withSites(this.env), event, step);
  }
}

export default { fetch: handle };
