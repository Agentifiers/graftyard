// BF-54's scratch Worker in the runner account (tests/m4/wp4/bf54.mjs): one `standard-4` Sandbox per name on the
// runner image, driven over HTTP by the orb driver. Deleted after the run. Every call needs header x-t: <TOKEN>.
//   POST /write?box=<name>&path=<file>   body: the file's contents
//   POST /start?box=<name>               body: a shell command, started with keepAlive (the box stays up until /destroy)
//   POST /exec?box=<name>                body: a shell command; answers {exitCode, stdout, stderr}
//   POST /destroy?box=<name>
import { Sandbox, getSandbox } from "@cloudflare/sandbox";
export { ContainerProxy } from "@cloudflare/sandbox";
export class Bf54Box extends Sandbox {}

export default {
  async fetch(req, env) {
    if (req.headers.get("x-t") !== env.TOKEN) return new Response("no", { status: 403 });
    const u = new URL(req.url);
    const box = getSandbox(env.BOX, u.searchParams.get("box"));
    const body = await req.text();
    try {
      switch (u.pathname) {
        case "/write": await box.writeFile(u.searchParams.get("path"), body); return Response.json({ ok: true });
        case "/start": {
          await box.setKeepAlive(true);
          const p = await box.startProcess(body, { processId: u.searchParams.get("id") ?? "main" });
          return Response.json({ ok: true, id: p.id });
        }
        case "/exec": {
          const r = await box.exec(body);
          return Response.json({ exitCode: r.exitCode, stdout: r.stdout, stderr: r.stderr });
        }
        case "/destroy": await box.destroy(); return Response.json({ ok: true });
      }
      return new Response("not found", { status: 404 });
    } catch (e) {
      return Response.json({ error: String(e?.message ?? e) }, { status: 500 });
    }
  },
};
