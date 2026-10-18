import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("./src", import.meta.url));
const port = Number(process.env.PORT || 1420);

const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
};

createServer(async (req, res) => {
  try {
    let path = decodeURIComponent(new URL(req.url, "http://localhost").pathname);
    if (path === "/") {
      path = "/index.html";
    }
    const file = normalize(join(root, path));
    if (!file.startsWith(root)) {
      res.writeHead(403).end("forbidden");
      return;
    }
    const data = await readFile(file);
    res.writeHead(200, {
      "content-type": types[extname(file).toLowerCase()] || "application/octet-stream",
    });
    res.end(data);
  } catch {
    res.writeHead(404).end("not found");
  }
}).listen(port, () => console.log(`Melon Client launcher UI at http://localhost:${port}`));
