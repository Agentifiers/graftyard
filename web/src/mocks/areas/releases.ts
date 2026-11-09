import { HttpResponse } from "msw";
import { createOpenApiHttp } from "openapi-msw";
import type { UploadPaths } from "@/areas/releases/api";
import { gate, http, notFound, page, unprocessable } from "../http";
import { branches } from "../world";
import { db } from "./core-db";
import { releasesDb } from "./releases-db";

// Releases, their assets, reactions, generated notes, uploads on the uploads host, and the
// source archives a published release links to.

const uploadsHttp = createOpenApiHttp<UploadPaths>({ baseUrl: "/api/uploads" });

const DOCS = "https://docs.github.com/enterprise-server@3.22/rest/releases";

function binary(data: Uint8Array<ArrayBuffer>, name: string, type: string) {
  return new HttpResponse(data, {
    status: 200,
    headers: {
      "Content-Type": type,
      "Content-Length": String(data.byteLength),
      "Content-Disposition": `attachment; filename="${name}"`,
    },
  });
}

// ---- Source archives: a tarball and a zipball with one README, built in memory ----

const enc = new TextEncoder();

function tar(path: string, text: string): Uint8Array<ArrayBuffer> {
  const content = enc.encode(text);
  const header = new Uint8Array(512);
  const put = (s: string, at: number) => header.set(enc.encode(s), at);
  const octal = (n: number, width: number) => n.toString(8).padStart(width - 1, "0") + "\0";
  put(path, 0);
  put(octal(0o644, 8), 100);
  put(octal(0, 8), 108);
  put(octal(0, 8), 116);
  put(octal(content.length, 12), 124);
  put(octal(Math.floor(Date.now() / 1000), 12), 136);
  put("        ", 148);
  put("0", 156);
  put("ustar\x0000", 257);
  const sum = header.reduce((a, b) => a + b, 0);
  put(octal(sum, 7) + " ", 148);
  const padded = Math.ceil(content.length / 512) * 512;
  const out = new Uint8Array(512 + padded + 1024);
  out.set(header, 0);
  out.set(content, 512);
  return out;
}

async function gzip(data: Uint8Array<ArrayBuffer>) {
  const stream = new Blob([data]).stream().pipeThrough(new CompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

const CRC = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc32 = (d: Uint8Array) => {
  let c = 0xffffffff;
  for (const b of d) c = CRC[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};

// A stored (uncompressed) zip: one local header, the data, one central entry, the end record.
function zip(path: string, text: string): Uint8Array<ArrayBuffer> {
  const name = enc.encode(path);
  const data = enc.encode(text);
  const crc = crc32(data);
  const local = new DataView(new ArrayBuffer(30));
  local.setUint32(0, 0x04034b50, true);
  local.setUint16(4, 10, true);
  local.setUint32(14, crc, true);
  local.setUint32(18, data.length, true);
  local.setUint32(22, data.length, true);
  local.setUint16(26, name.length, true);
  const central = new DataView(new ArrayBuffer(46));
  central.setUint32(0, 0x02014b50, true);
  central.setUint16(4, 20, true);
  central.setUint16(6, 10, true);
  central.setUint32(16, crc, true);
  central.setUint32(20, data.length, true);
  central.setUint32(24, data.length, true);
  central.setUint16(28, name.length, true);
  const localSize = 30 + name.length + data.length;
  const centralSize = 46 + name.length;
  const end = new DataView(new ArrayBuffer(22));
  end.setUint32(0, 0x06054b50, true);
  end.setUint16(8, 1, true);
  end.setUint16(10, 1, true);
  end.setUint32(12, centralSize, true);
  end.setUint32(16, localSize, true);
  const out = new Uint8Array(localSize + centralSize + 22);
  let at = 0;
  for (const part of [
    new Uint8Array(local.buffer),
    name,
    data,
    new Uint8Array(central.buffer),
    name,
    new Uint8Array(end.buffer),
  ]) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}

function archiveReadme(full: string, ref: string) {
  return `# ${full}\n\nSource archive of ${full} at ${ref}, served by the Graftyard mock.\n`;
}

export const handlers = [
  http.get("/repos/{owner}/{repo}/releases", async ({ params, query, response }) => {
    const fault = await gate("releases-list");
    if (fault) return response.untyped(fault);
    const s = releasesDb.state(params.owner, params.repo);
    if (!s) return response.untyped(notFound());
    const all = releasesDb.list(s);
    const items = page(all, query);
    const perPage = Math.min(100, Number(query.get("per_page")) || 30);
    const last = Math.max(1, Math.ceil(all.length / perPage));
    const current = Math.max(1, Number(query.get("page")) || 1);
    const base = `${s.repo.url}/releases?per_page=${perPage}`;
    const rels = [
      current < last && `<${base}&page=${current + 1}>; rel="next"`,
      current < last && `<${base}&page=${last}>; rel="last"`,
      current > 1 && `<${base}&page=${current - 1}>; rel="prev"`,
      current > 1 && `<${base}&page=1>; rel="first"`,
    ].filter(Boolean);
    return response(200).json(items, { headers: { Link: rels.join(", ") } });
  }),

  http.get("/repos/{owner}/{repo}/releases/latest", async ({ params, response }) => {
    const fault = await gate("releases-latest");
    if (fault) return response.untyped(fault);
    const s = releasesDb.state(params.owner, params.repo);
    const r = s && releasesDb.latest(s);
    return r ? response(200).json(r) : response.untyped(notFound());
  }),

  http.get("/repos/{owner}/{repo}/releases/tags/{tag}", async ({ params, response }) => {
    const fault = await gate("releases-get");
    if (fault) return response.untyped(fault);
    const s = releasesDb.state(params.owner, params.repo);
    const r = s && releasesDb.byTag(s, params.tag);
    return r ? response(200).json(r) : response.untyped(notFound());
  }),

  http.get("/repos/{owner}/{repo}/releases/{release_id}", async ({ params, response }) => {
    const fault = await gate("releases-get");
    if (fault) return response.untyped(fault);
    const s = releasesDb.state(params.owner, params.repo);
    const r = s && releasesDb.get(s, Number(params.release_id));
    return r ? response(200).json(r) : response.untyped(notFound());
  }),

  http.post("/repos/{owner}/{repo}/releases", async ({ params, request, response }) => {
    const fault = await gate("releases-create");
    if (fault) return response.untyped(fault);
    const s = releasesDb.state(params.owner, params.repo);
    if (!s) return response.untyped(notFound());
    const body = await request.json();
    const tag = body.tag_name.trim();
    if (!tag || /\s|\.\.|[~^:?*[\\]|\.lock$|^-|\/$/.test(tag)) {
      return response.untyped(unprocessable("Release", "tag_name", "invalid"));
    }
    if (s.releases.some((r) => r.tag_name === tag)) {
      return response.untyped(unprocessable("Release", "tag_name", "already_exists"));
    }
    const target = body.target_commitish ?? s.repo.default_branch;
    const tagExists = releasesDb.tags(s).includes(tag);
    if (!tagExists && !knownTarget(params.owner, params.repo, target)) {
      return response.untyped(unprocessable("Release", "target_commitish", "invalid"));
    }
    let notes = body.body;
    if (body.generate_release_notes) {
      notes = releasesDb.generateNotes(s, mergedPulls(params.owner, params.repo), {
        tag_name: tag,
      }).body;
    }
    const created = releasesDb.create(s, {
      ...body,
      tag_name: tag,
      target_commitish: target,
      body: notes,
    });
    return response(201).json(created, { headers: { Location: created.url } });
  }),

  http.patch(
    "/repos/{owner}/{repo}/releases/{release_id}",
    async ({ params, request, response }) => {
      const fault = await gate("releases-update");
      if (fault) return response.untyped(fault);
      const s = releasesDb.state(params.owner, params.repo);
      const r = s && releasesDb.record(s, Number(params.release_id));
      if (!s || !r) return response.untyped(notFound());
      const body = (await request.json()) ?? {};
      if (body.tag_name !== undefined) {
        const tag = body.tag_name.trim();
        if (!tag || /\s|\.\.|[~^:?*[\\]/.test(tag)) {
          return response.untyped(unprocessable("Release", "tag_name", "invalid"));
        }
        if (s.releases.some((x) => x.tag_name === tag && x.id !== r.id)) {
          return response.untyped(unprocessable("Release", "tag_name", "already_exists"));
        }
        body.tag_name = tag;
      }
      return response(200).json(releasesDb.update(s, r, body));
    },
  ),

  http.delete("/repos/{owner}/{repo}/releases/{release_id}", async ({ params, response }) => {
    const fault = await gate("releases-delete");
    if (fault) return response.untyped(fault);
    const s = releasesDb.state(params.owner, params.repo);
    if (!s || !releasesDb.record(s, Number(params.release_id))) {
      return response.untyped(notFound());
    }
    releasesDb.remove(s, Number(params.release_id));
    return response(204).empty();
  }),

  http.post(
    "/repos/{owner}/{repo}/releases/generate-notes",
    async ({ params, request, response }) => {
      const fault = await gate("releases-notes");
      if (fault) return response.untyped(fault);
      const s = releasesDb.state(params.owner, params.repo);
      if (!s) return response.untyped(notFound());
      const body = await request.json();
      if (body.previous_tag_name && !releasesDb.tags(s).includes(body.previous_tag_name)) {
        return response.untyped(notFound());
      }
      return response(200).json(
        releasesDb.generateNotes(s, mergedPulls(params.owner, params.repo), body),
      );
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/releases/{release_id}/assets",
    async ({ params, query, response }) => {
      const fault = await gate("releases-assets");
      if (fault) return response.untyped(fault);
      const s = releasesDb.state(params.owner, params.repo);
      const r = s && releasesDb.record(s, Number(params.release_id));
      return r ? response(200).json(page(r.assets, query)) : response.untyped(notFound());
    },
  ),

  // Accept: application/octet-stream downloads the asset and counts it, as GHES does by
  // redirecting to the binary. Any other Accept gets the asset's JSON.
  http.get(
    "/repos/{owner}/{repo}/releases/assets/{asset_id}",
    async ({ params, request, response }) => {
      const fault = await gate("releases-asset-download");
      if (fault) return response.untyped(fault);
      const s = releasesDb.state(params.owner, params.repo);
      const found = s && releasesDb.findAsset(s, Number(params.asset_id));
      if (!found) return response.untyped(notFound());
      if (!request.headers.get("accept")?.includes("application/octet-stream")) {
        return response(200).json(found.asset);
      }
      found.asset.download_count += 1;
      return response.untyped(
        binary(releasesDb.assetBytes(found.asset), found.asset.name, found.asset.content_type),
      );
    },
  ),

  http.patch(
    "/repos/{owner}/{repo}/releases/assets/{asset_id}",
    async ({ params, request, response }) => {
      const fault = await gate("releases-asset-update");
      if (fault) return response.untyped(fault);
      const s = releasesDb.state(params.owner, params.repo);
      const found = s && releasesDb.findAsset(s, Number(params.asset_id));
      if (!found) return response.untyped(notFound());
      const body = (await request.json()) ?? {};
      if (body.name !== undefined) {
        const name = body.name.trim();
        if (!name) return response.untyped(unprocessable("ReleaseAsset", "name", "missing_field"));
        if (found.release.assets.some((a) => a.name === name && a.id !== found.asset.id)) {
          return response.untyped(unprocessable("ReleaseAsset", "name", "already_exists"));
        }
        found.asset.name = name;
        found.asset.browser_download_url = `${found.release.html_url.replace("/tag/", "/download/")}/${encodeURIComponent(name)}`;
      }
      if (body.label !== undefined) found.asset.label = body.label || null;
      found.asset.updated_at = new Date().toISOString();
      return response(200).json(found.asset);
    },
  ),

  http.delete("/repos/{owner}/{repo}/releases/assets/{asset_id}", async ({ params, response }) => {
    const fault = await gate("releases-asset-delete");
    if (fault) return response.untyped(fault);
    const s = releasesDb.state(params.owner, params.repo);
    if (!s || !releasesDb.removeAsset(s, Number(params.asset_id))) {
      return response.untyped(notFound());
    }
    return response(204).empty();
  }),

  http.get(
    "/repos/{owner}/{repo}/releases/{release_id}/reactions",
    async ({ params, query, response }) => {
      const fault = await gate("releases-reactions");
      if (fault) return response.untyped(fault);
      const s = releasesDb.state(params.owner, params.repo);
      if (!s || !releasesDb.record(s, Number(params.release_id))) {
        return response.untyped(notFound());
      }
      const content = query.get("content");
      const list = releasesDb
        .reactions(s, Number(params.release_id))
        .filter((r) => !content || r.content === content);
      return response(200).json(page(list, query));
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/releases/{release_id}/reactions",
    async ({ params, request, response }) => {
      const fault = await gate("releases-react");
      if (fault) return response.untyped(fault);
      const s = releasesDb.state(params.owner, params.repo);
      if (!s || !releasesDb.record(s, Number(params.release_id))) {
        return response.untyped(notFound());
      }
      const { content } = await request.json();
      const { reaction, created } = releasesDb.react(s, Number(params.release_id), content);
      return created ? response(201).json(reaction) : response(200).json(reaction);
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/releases/{release_id}/reactions/{reaction_id}",
    async ({ params, response }) => {
      const fault = await gate("releases-react");
      if (fault) return response.untyped(fault);
      const s = releasesDb.state(params.owner, params.repo);
      if (!s) return response.untyped(notFound());
      releasesDb.unreact(s, Number(params.release_id), Number(params.reaction_id));
      return response(204).empty();
    },
  ),

  // Source archives. GHES answers 302 to codeload; the mock serves the archive directly.
  http.get("/repos/{owner}/{repo}/tarball/{ref}", async ({ params, response }) => {
    const fault = await gate("releases-archive");
    if (fault) return response.untyped(fault);
    const s = releasesDb.state(params.owner, params.repo);
    if (!s) return response.untyped(notFound());
    const prefix = `${s.repo.name}-${params.ref.replace(/^v/, "")}`;
    const data = await gzip(
      tar(`${prefix}/README.md`, archiveReadme(s.repo.full_name, params.ref)),
    );
    return response.untyped(binary(data, `${prefix}.tar.gz`, "application/x-gzip"));
  }),

  http.get("/repos/{owner}/{repo}/zipball/{ref}", async ({ params, response }) => {
    const fault = await gate("releases-archive");
    if (fault) return response.untyped(fault);
    const s = releasesDb.state(params.owner, params.repo);
    if (!s) return response.untyped(notFound());
    const prefix = `${s.repo.name}-${params.ref.replace(/^v/, "")}`;
    const data = zip(`${prefix}/README.md`, archiveReadme(s.repo.full_name, params.ref));
    return response.untyped(binary(data, `${prefix}.zip`, "application/zip"));
  }),

  uploadsHttp.post(
    "/repos/{owner}/{repo}/releases/{release_id}/assets",
    async ({ params, query, request, response }) => {
      const fault = await gate("releases-upload");
      if (fault) return response.untyped(fault);
      const s = releasesDb.state(params.owner, params.repo);
      const r = s && releasesDb.record(s, Number(params.release_id));
      if (!s || !r) return response.untyped(notFound());
      const name = query.get("name")?.trim();
      if (!name) {
        return response(422).json({
          message: "Validation Failed",
          documentation_url: `${DOCS}/assets#upload-a-release-asset`,
          errors: [{ resource: "ReleaseAsset", field: "name", code: "missing_field" }],
        });
      }
      if (r.assets.some((a) => a.name === name)) {
        return response(422).json({
          message: "Validation Failed",
          documentation_url: `${DOCS}/assets#upload-a-release-asset`,
          errors: [{ resource: "ReleaseAsset", field: "name", code: "already_exists" }],
        });
      }
      const data = new Uint8Array(await request.arrayBuffer());
      const created = releasesDb.addAsset(s, r, {
        name,
        label: query.get("label") ?? undefined,
        type: request.headers.get("content-type") || "application/octet-stream",
        data,
      });
      return response(201).json(created);
    },
  ),
];

function mergedPulls(owner: string, repo: string) {
  return db.state(owner, repo)?.pulls.filter((p) => p.merged_at) ?? [];
}

function knownTarget(owner: string, repo: string, target: string) {
  return (
    (branches[`${owner}/${repo}`.toLowerCase()] ?? []).includes(target) ||
    /^[0-9a-f]{7,40}$/.test(target)
  );
}
