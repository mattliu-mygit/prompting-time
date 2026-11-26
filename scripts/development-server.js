import { readFileSync, statSync } from "node:fs";
import { extname, join, resolve, sep } from "node:path";

/**
 * Tauri sends the window's Content Security Policy, from tauri.conf.json, only with the built app.
 * This plugin has the Vite server send it too, so `pnpm tauri dev` and the browser fixture run under
 * it, with what development adds: Vite's reload socket, and the fixture's plugin page, which this
 * server serves.
 *
 * The fixture's plugin page: /plugin-page/<path> answers with the file at <path> in the folder
 * PROMPTING_TIME_FIXTURE_PAGE names, such as a built Whooper page/dist, with the headers the app's
 * pt-plugin:// scheme sends (src-tauri/src/plugin_pages.rs), this server standing in for pt-plugin:.
 *
 * @returns {import("vite").Plugin}
 */
export function developmentServer() {
  const config = JSON.parse(readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"));
  const directives = { ...config.app.security.csp };
  directives["connect-src"] += " ws://localhost:1420";
  directives["frame-src"] += " 'self'";
  const policy = Object.entries(directives).map(([name, sources]) => `${name} ${sources}`).join("; ");
  const pagePolicy = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; connect-src 'none'; form-action 'none'; base-uri 'none'";
  const pageFolder = process.env.PROMPTING_TIME_FIXTURE_PAGE ? resolve(process.env.PROMPTING_TIME_FIXTURE_PAGE) : null;
  const types = {
    ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8",
    ".json": "application/json", ".svg": "image/svg+xml", ".png": "image/png", ".woff2": "font/woff2", ".wasm": "application/wasm",
  };
  /** Answers one request for the fixture's plugin page. */
  function servePage(pathname, response) {
    // Every answer allows the sandboxed frame's origin, "null".
    response.setHeader("Access-Control-Allow-Origin", "null");
    const file = pageFolder && resolve(join(pageFolder, decodeURIComponent(pathname.slice("/plugin-page/".length))));
    if (!file || !file.startsWith(pageFolder + sep) || !statSync(file, { throwIfNoEntry: false })?.isFile()) {
      response.statusCode = 404;
      response.end(pageFolder ? "No such plugin page file." : "Set PROMPTING_TIME_FIXTURE_PAGE to a built plugin page folder.");
      return;
    }
    response.setHeader("Content-Type", types[extname(file)] ?? "application/octet-stream");
    response.setHeader("Content-Security-Policy", pagePolicy);
    response.setHeader("X-Content-Type-Options", "nosniff");
    response.end(readFileSync(file));
  }
  return {
    name: "prompting-time-development-server",
    apply: "serve",
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        const { pathname } = new URL(request.url ?? "/", "http://localhost");
        if (pathname.startsWith("/plugin-page/")) {
          servePage(pathname, response);
          return;
        }
        response.setHeader("Content-Security-Policy", policy);
        next();
      });
    },
  };
}
