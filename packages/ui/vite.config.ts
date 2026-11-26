import solid from "@solidjs/vite-plugin";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [solid()],
  // `pnpm --filter @prompting-time/ui gallery` serves gallery/index.html here.
  server: { host: "localhost", port: 5174, strictPort: true },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
  },
});
