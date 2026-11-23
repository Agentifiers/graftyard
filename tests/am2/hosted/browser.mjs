// The user's browser for an SDK client's authorization URL (`tests/am2/hosted/mcp.mjs`, `consent`): prints the
// client's callback URL with `code`, `state`, and `iss`. For the Go and Python clients, which run it as a subprocess.
//   node tests/am2/hosted/browser.mjs <authorization-url>
import { consent } from "./mcp.mjs";

const { callback } = await consent(process.argv[2]);
process.stdout.write(`${callback}\n`);
