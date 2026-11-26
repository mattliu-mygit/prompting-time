import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  server: {
    host: "localhost",
    port: 1420,
    strictPort: true,
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    globals: true,
    // The app's tests, then the UI library's, which build with Solid instead of React.
    projects: [{ extends: true, test: { name: "app", include: ["src/**/*.test.{ts,tsx}"] } }, "packages/ui"]
  }
});
