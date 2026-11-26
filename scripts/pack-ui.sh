#!/usr/bin/env bash
# Builds @prompting-time/ui and packs it as packages/ui/prompting-time-ui-<version>.tgz, the
# tarball a Prompting Time release attaches for plugin pages. Prints the tarball's path and its
# sha512 integrity, the value a plugin's lockfile records for it (packages/ui/README.md).
set -euo pipefail

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
cd "$script_directory/../packages/ui"

pnpm run build
tarball="$(pnpm pack --json | node -e 'let s = ""; process.stdin.on("data", (d) => (s += d)).on("end", () => console.log(JSON.parse(s).filename))')"
integrity="$(node -e 'const { createHash } = require("node:crypto"); const { readFileSync } = require("node:fs"); console.log("sha512-" + createHash("sha512").update(readFileSync(process.argv[1])).digest("base64"))' "$tarball")"

echo "tarball:   $PWD/$tarball"
echo "integrity: $integrity"
