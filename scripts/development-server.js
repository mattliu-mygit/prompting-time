import { readFileSync } from "node:fs";

/**
 * Tauri sends the window's Content Security Policy, from tauri.conf.json, only with the built app.
 * This plugin has the Vite server send it too, so `pnpm tauri dev` and the browser fixture run under
 * it, with what development adds: Vite's reload socket, and the fixture's plugin page, which this
 * server serves.
 *
 * @returns {import("vite").Plugin}
 */
export function developmentServer() {
  const config = JSON.parse(readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"));
  const directives = { ...config.app.security.csp };
  directives["connect-src"] += " ws://localhost:1420";
  directives["frame-src"] += " 'self'";
  const policy = Object.entries(directives).map(([name, sources]) => `${name} ${sources}`).join("; ");
  return {
    name: "prompting-time-development-server",
    apply: "serve",
    configureServer(server) {
      server.middlewares.use((_request, response, next) => {
        response.setHeader("Content-Security-Policy", policy);
        next();
      });
    },
  };
}
