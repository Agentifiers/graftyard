// Scratch front for the colo probe: forwards every request to the probe over a service binding, as graft-front
// forwards to the edge, so the probe's Cache API calls run in a Worker invoked by a binding.
export default {
  async fetch(req, env) {
    return env.EDGE.fetch(req);
  },
};
