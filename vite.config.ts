import solid from "@solidjs/vite-plugin";
import { defineConfig } from "vitest/config";
import { developmentServer } from "./scripts/development-server.js";

export default defineConfig({
  plugins: [solid(), developmentServer()],
  server: {
    host: "localhost",
    port: 1420,
    strictPort: true,
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    globals: true,
    // The app's tests, then the UI library's.
    projects: [{ extends: true, test: { name: "app", include: ["src/**/*.test.{ts,tsx}"] } }, "packages/ui"]
  }
});
