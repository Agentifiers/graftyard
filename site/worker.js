// Serves the static site from dist/ and takes one page-view beacon per page load on POST /_v.
// The beacon carries the path, the referring host, the viewport width and the theme; the Worker adds
// the time and the visitor's country and sends the row to the Basin stream. No cookie, no IP, no id.
export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);
    if (url.pathname !== '/_v') return env.ASSETS.fetch(request);
    if (request.method !== 'POST') return new Response(null, { status: 405, headers: { allow: 'POST' } });

    let body = {};
    try { body = JSON.parse((await request.text()).slice(0, 2048)); } catch {}
    const path = typeof body.p === 'string' && body.p.startsWith('/') ? body.p.slice(0, 256) : null;
    if (!path) return new Response(null, { status: 400 });
    const width = Number.isInteger(body.w) && body.w > 0 && body.w < 100000 ? body.w : null;
    const theme = body.t === 'light' || body.t === 'dark' ? body.t : null;
    let referrer = null;
    try { if (body.r) referrer = new URL(body.r).hostname.slice(0, 253) || null; } catch {}

    ctx.waitUntil(env.PAGEVIEWS.send([{
      ts: new Date().toISOString(),
      path,
      referrer,
      country: request.cf?.country ?? null,
      viewport_w: width,
      theme,
    }]).catch((e) => console.error('pageview send failed', e)));
    return new Response(null, { status: 204 });
  },
};
