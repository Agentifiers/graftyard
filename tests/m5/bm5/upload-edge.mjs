// BM-5 clause 3 (`mint.sh edge`): uploads `<dir>`'s edge build as a version of `<script>`, keeping every binding and
// the runtime settings of version `<live>`, without deploying it. Prints `{ok, id, errors}`.
import { readFileSync } from "node:fs";
const [dir, name, live] = process.argv.slice(2);
const B = `https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}`;
const H = { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}` };
const one = await (await fetch(`${B}/workers/scripts/${name}/versions/${live}`, { headers: H })).json();
const rt = one.result.resources.script_runtime;
// A version upload keeps the bindings of the script's newest version, not the deployed one's, so the plain-text vars
// are copied from `<live>` itself: a var a later upload added (clause 1's FORCE_FORMAT) is not carried.
const vars = one.result.resources.bindings.filter((b) => b.type === "plain_text").map(({ name, text }) => ({ type: "plain_text", name, text }));
const meta = {
  main_module: "index.js",
  keep_bindings: ["secret_text", "json", "durable_object_namespace", "service", "r2_bucket", "queue",
    "secrets_store_secret", "pipelines", "version_metadata", "assets", "workflow"],
  bindings: vars,
  keep_assets: true,
  annotations: { "workers/tag": "bm5m-edge", "workers/message": "BM-5 clause 3: the edge's MintWatch" },
  compatibility_date: rt.compatibility_date,
  ...(rt.compatibility_flags ? { compatibility_flags: rt.compatibility_flags } : {}),
  ...(rt.limits ? { limits: rt.limits } : {}),
};
const form = new FormData();
form.append("metadata", JSON.stringify(meta));
form.append("index.js", new Blob([readFileSync(`${dir}/index.js`)], { type: "application/javascript+module" }), "index.js");
form.append("index_bg.wasm", new Blob([readFileSync(`${dir}/index_bg.wasm`)], { type: "application/wasm" }), "index_bg.wasm");
const r = await (await fetch(`${B}/workers/scripts/${name}/versions`, { method: "POST", headers: H, body: form })).json();
console.log(JSON.stringify({ ok: r.success, id: r.result?.id, errors: r.errors }));
