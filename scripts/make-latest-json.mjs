#!/usr/bin/env node
/**
 * Generates `latest.json` (the Tauri updater manifest) from the release bundle.
 *
 * Usage:
 *   node scripts/make-latest-json.mjs [--notes "What's new"]
 *
 * It reads the version from src-tauri/tauri.conf.json, grabs the signature(s)
 * from the built `*.app.tar.gz.sig` file(s), and writes:
 *   src-tauri/target/release/bundle/latest.json
 *
 * The `url` points at UPDATE_BASE_URL (env) or the default insrt.fr location.
 * Upload BOTH `latest.json` and the matching `*.app.tar.gz` to that URL.
 */
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const BASE = (process.env.UPDATE_BASE_URL || "https://updates.insrt.fr/pierscrm").replace(/\/$/, "");

const notesIdx = process.argv.indexOf("--notes");
const notes = notesIdx > -1 ? process.argv[notesIdx + 1] : "Nouvelle version de PiersCRM.";

const conf = JSON.parse(readFileSync(join(root, "src-tauri/tauri.conf.json"), "utf8"));
const version = conf.version;
const bundleMac = join(root, "src-tauri/target/release/bundle/macos");

// Only publish the architecture that was actually built. `pnpm tauri build`
// on an Intel Mac produces an x86_64 bundle; build on Apple Silicon (or with
// `--target aarch64-apple-darwin`) to also serve `darwin-aarch64`.
// Override with UPDATE_ARCH=aarch64 when building for Apple Silicon.
const arch = process.env.UPDATE_ARCH || (process.arch === "arm64" ? "aarch64" : "x86_64");
const file = "PiersCRM.app.tar.gz";
const sig = join(bundleMac, `${file}.sig`);

const platforms = {};
if (existsSync(sig)) {
  platforms[`darwin-${arch}`] = {
    signature: readFileSync(sig, "utf8").trim(),
    url: `${BASE}/${file}`,
  };
}

if (Object.keys(platforms).length === 0) {
  console.error("No *.app.tar.gz.sig found — run `pnpm tauri build` first (with signing env vars).");
  process.exit(1);
}

const manifest = {
  version,
  notes,
  pub_date: new Date().toISOString(),
  platforms,
};

const out = join(root, "src-tauri/target/release/bundle/latest.json");
writeFileSync(out, JSON.stringify(manifest, null, 2));
console.log(`Wrote ${out}\n`);
console.log(JSON.stringify(manifest, null, 2));
console.log(`\nUpload to ${BASE}/ :`);
console.log(`  - latest.json`);
console.log(`  - PiersCRM.app.tar.gz  (from src-tauri/target/release/bundle/macos/)`);
