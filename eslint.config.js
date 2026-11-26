import js from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";

export default [
  {
    ignores: ["dist", "packages/*/dist", "node_modules", "target", "src-tauri/target"]
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    languageOptions: {
      globals: globals.browser
    }
  },
  {
    files: ["tests/browser/plugin-page-v2/*.js"],
    languageOptions: {
      sourceType: "script",
      globals: globals.browser
    }
  },
  {
    files: ["scripts/**/*.js"],
    languageOptions: {
      globals: globals.node
    }
  }
];
