// The parser's differential oracle: `@actions/workflow-parser` 0.3.61 (MIT, GitHub).
//
//   mkdir -p /tmp/wfp && cd /tmp/wfp && npm i @actions/workflow-parser@0.3.61
//   cp <repo>/tests/m4/wp3/oracle.mjs /tmp/wfp/ && cd /tmp/wfp && bun oracle.mjs <repo>/tests/m4/wp3/workflows
//
// Writes `<name>.oracle.json` beside each `<name>.yml`: the `convertWorkflowTemplate` result, which
// `crates/actions/tests/workflows.rs` compares with the Rust port. Run it with bun; node refuses the package's JSON
// import. One change to the oracle: a multi-line string's expressions are read from its value, not its raw YAML
// source, because the runner's C# parser does that and the npm parser keeps the source only for editor ranges
// (`crates/actions/src/workflow/template.rs`).
import { parseWorkflow, convertWorkflowTemplate, NoOperationTraceWriter } from "@actions/workflow-parser";
import { YamlObjectReader } from "./node_modules/@actions/workflow-parser/dist/workflows/yaml-object-reader.js";
import { readFileSync, readdirSync, writeFileSync } from "fs";

const literal = YamlObjectReader.getLiteralToken;
YamlObjectReader.getLiteralToken = function (...args) {
  const t = literal.apply(this, args);
  if (t && "source" in t) t.source = undefined;
  return t;
};
const dir = process.argv[2];
for (const f of readdirSync(dir).filter(f => f.endsWith(".yml")).sort()) {
  const r = parseWorkflow({ name: f, content: readFileSync(`${dir}/${f}`, "utf8") }, new NoOperationTraceWriter());
  const t = await convertWorkflowTemplate(r.context, r.value);
  writeFileSync(`${dir}/${f.replace(/\.yml$/, ".oracle.json")}`, JSON.stringify(t) + "\n");
}
