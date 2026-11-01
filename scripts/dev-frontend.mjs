import { createServer } from "node:http";
import { createReadStream, existsSync, statSync } from "node:fs";
import { extname, resolve, sep } from "node:path";

const root = resolve("frontend");
const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
  ".json": "application/json",
  ".png": "image/png",
  ".ico": "image/x-icon",
};

const server = createServer((request, response) => {
  const raw = decodeURIComponent((request.url || "/").split("?")[0]);
  const relative = raw === "/" ? "/index.html" : raw;
  const file = resolve(root, `.${relative}`);
  const inside = file === root || file.startsWith(root + sep);
  if (!inside || !existsSync(file) || !statSync(file).isFile()) {
    response.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" });
    response.end("Not Found");
    return;
  }
  response.writeHead(200, {
    "Content-Type": types[extname(file)] || "application/octet-stream",
  });
  if (request.method === "HEAD") {
    response.end();
    return;
  }
  createReadStream(file).pipe(response);
});

server.on("error", (error) => {
  if (error.code !== "EADDRINUSE") {
    console.error(error);
    process.exit(1);
  }
  // 上次开发服务还在听 1430。保持本进程不退出，Tauri 才能继续编译并打开窗口。
  console.log("127.0.0.1:1430 已在提供页面，继续使用现有服务。");
  setInterval(() => {}, 60 * 60 * 1000);
});

server.listen(1430, "127.0.0.1");
