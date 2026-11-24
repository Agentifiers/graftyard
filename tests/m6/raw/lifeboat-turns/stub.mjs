// The stale bed's install stand-in: the feed answers 503 (stopped), and the edge answers every `entries` probe 200
// with no rows (caught up) after a delay drawn from BF-53's hosted edge wall times; the decoder POST gets 201.
// Logs one JSON line per second: requests served, in flight, and the most in flight.
import http from "node:http";
import { readFileSync } from "node:fs";
const walls = JSON.parse(readFileSync(process.argv[3]));
const port = Number(process.argv[2]);
const rtt = Number(process.argv[4] ?? 30);
let served = 0, inflight = 0, peak = 0, s503 = 0;
http.createServer((req, res) => {
  if (req.url.startsWith("/feed")) { s503++; res.writeHead(503); return res.end("<Error><Code>ServiceUnavailable</Code></Error>"); }
  if (req.method === "POST") { res.writeHead(201, { "content-type": "application/json" }); return res.end("{}"); }
  inflight++; peak = Math.max(peak, inflight);
  const ms = walls[Math.floor(Math.random() * walls.length)] + rtt;
  setTimeout(() => { inflight--; served++; res.writeHead(200, { "content-type": "application/octet-stream" }); res.end(); }, ms);
}).listen(port, "127.0.0.1");
setInterval(() => { console.log(JSON.stringify({ t: Date.now(), served, inflight, peak, s503 })); peak = inflight; }, 1000);
