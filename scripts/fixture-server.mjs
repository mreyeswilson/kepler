// Tiny local HTTP server used by scripts/acceptance.ps1 (and manual Vision checks).
// Serves a page with: a console error, a 404 sub-resource, a broken same-origin link and an
// image without alt text. Prints `listening <port>` on stdout once ready.
// Usage: node scripts/fixture-server.mjs [port]   (0 or omitted = pick a free port)
import http from "node:http";

const PAGE = `<!doctype html>
<html><head><meta charset="utf-8"><title>Kepler fixture</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<link rel="stylesheet" href="/missing.css">
</head><body>
<h1>Fixture</h1>
<a href="/ok">fine link</a> <a href="/broken-link">broken link</a>
<img src="/pixel.png">
<script>console.error("fixture console error");</script>
</body></html>`;

const server = http.createServer((req, res) => {
  const url = (req.url ?? "/").split("?")[0];
  if (url === "/" || url === "/index.html") {
    res.writeHead(200, { "content-type": "text/html; charset=utf-8" }).end(PAGE);
  } else if (url === "/ok") {
    res.writeHead(200, { "content-type": "text/plain" }).end("ok");
  } else if (url === "/pixel.png") {
    const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==", "base64");
    res.writeHead(200, { "content-type": "image/png" }).end(png);
  } else if (url === "/slow") {
    // Never answers within any sane timeout: used to prove the time limit kills the worker.
    setTimeout(() => res.writeHead(200).end("late"), 120_000).unref();
  } else {
    res.writeHead(404, { "content-type": "text/plain" }).end("not found");
  }
});

server.listen(Number(process.argv[2] ?? 0), "127.0.0.1", () => {
  const addr = server.address();
  console.log(`listening ${typeof addr === "object" && addr ? addr.port : 0}`);
});
