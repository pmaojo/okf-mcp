#!/usr/bin/env node
// Copies the single-file build into the Rust crate that embeds it via
// `include_str!`. Run after `pnpm build` (or via `pnpm run build:sync`).
// CI re-runs this and fails the build if the checked-in copy drifts —
// see .github/workflows/mcp-app.yml.
import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const appRoot = join(here, "..");
const src = join(appRoot, "dist", "mcp-app.html");
const destDir = join(appRoot, "..", "crates", "memory-tools", "assets");
const dest = join(destDir, "mcp-app.html");

if (!existsSync(src)) {
  console.error(`Missing ${src} — run "pnpm build" first.`);
  process.exit(1);
}

mkdirSync(destDir, { recursive: true });
copyFileSync(src, dest);
console.log(`Synced ${src} -> ${dest}`);
