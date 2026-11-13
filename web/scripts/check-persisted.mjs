// Fails when an entry in src/api/persisted.json has a sha256 that is not the hash of its
// query text (UTF-8), or names an operation other than its key. The edge embeds the file
// and looks documents up by hash, so a stale hash would answer PersistedQueryNotFound.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

const file = new URL("../src/api/persisted.json", import.meta.url);
const docs = JSON.parse(readFileSync(file, "utf8"));
let bad = 0;
for (const [name, { sha256, query }] of Object.entries(docs)) {
  const actual = createHash("sha256").update(query, "utf8").digest("hex");
  if (actual !== sha256) {
    console.error(`persisted.json: ${name} has sha256 ${sha256}, but its text hashes to ${actual}`);
    bad++;
  }
  const op = /^\s*(?:query|mutation|subscription)\s+([_A-Za-z][_0-9A-Za-z]*)/.exec(query)?.[1];
  if (op !== name) {
    console.error(`persisted.json: key ${name} holds the operation ${op ?? "(unnamed)"}`);
    bad++;
  }
}
if (bad) process.exit(1);
