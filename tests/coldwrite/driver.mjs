// coldwrite's front script (`run.sh`): wrangler dev serves the first config, so this one sends `/mk` to site 0's
// script and every other request to edge script `x-cw-edge` (0 is the seeding edge), unchanged. Each edge script is
// the same build, so an edge script that has served nothing yet is a cold isolate.
export default {
  async fetch(req, env) {
    const url = new URL(req.url);
    if (url.pathname === "/mk") return env.SITE0.fetch(req);
    const n = req.headers.get("x-cw-edge") ?? "0";
    return env[`EDGE${n}`].fetch(req);
  },
};
