import solid from "@solidjs/vite-plugin";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [solid()],
  // `pnpm --filter @prompting-time/ui gallery` serves gallery/index.html here.
  server: { host: "localhost", port: 5174, strictPort: true },
  // `pnpm --filter @prompting-time/ui build` compiles the components and bundles every dependency
  // except Solid into dist/index.js, the module the packed tarball exports to plugin pages (README),
  // with the bundled packages' licenses in dist/THIRD_PARTY_LICENSES.md. Shiki's grammars stay
  // separate chunks, which load the first time a language is highlighted.
  build: {
    license: { fileName: "THIRD_PARTY_LICENSES.md" },
    lib: { entry: "src/index.ts", formats: ["es"], fileName: "index" },
    rolldownOptions: {
      external: [/^solid-js(\/|$)/, /^@solidjs\/web(\/|$)/],
      output: { chunkFileNames: "chunks/[name]-[hash].js" },
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
  },
});
