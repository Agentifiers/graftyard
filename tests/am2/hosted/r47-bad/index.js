// R47's bad graft-mcp (`tests/am2/hosted/r47-day.sh`): every request answers 500, and a request from graft-front
// (`x-graftyard-front`) gets this version's id as `x-graftyard-mcp-version`, as the real graft-mcp reports it, so
// graft-front counts the 500s under this version. graft-ops uploads it with the running version's bindings.
export default {
  async fetch(req, env) {
    const h = new Headers({ "content-type": "text/plain" });
    if (req.headers.has("x-graftyard-front") && env.VERSION) h.set("x-graftyard-mcp-version", env.VERSION.id);
    return new Response("r47: a bad graft-mcp\n", { status: 500, headers: h });
  },
};
