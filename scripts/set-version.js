#!/usr/bin/env node
// Usage: node scripts/set-version.js <semver>
// Bumps the version across Cargo.toml and every npm package (meta + 5 platform
// sub-packages) so a single `git tag vX.Y.Z` drives a consistent release.
const fs = require("fs");
const path = require("path");

const VERSION = process.argv[2];
if (!VERSION || !/^\d+\.\d+\.\d+/.test(VERSION)) {
  console.error("Usage: node scripts/set-version.js <semver>   (e.g. 2.0.1)");
  process.exit(1);
}

const root = path.resolve(__dirname, "..");

function bumpPkg(rel) {
  const p = path.join(root, rel);
  const json = JSON.parse(fs.readFileSync(p, "utf8"));
  json.version = VERSION;
  if (json.optionalDependencies) {
    for (const k of Object.keys(json.optionalDependencies)) {
      if (k.startsWith("@dhicoc/codex-web-search-mcp-")) {
        json.optionalDependencies[k] = VERSION;
      }
    }
  }
  fs.writeFileSync(p, JSON.stringify(json, null, 2) + "\n");
  console.log("bumped", rel, "->", VERSION);
}

const packages = [
  "npm/codex-web-search-mcp/package.json",
  "npm/platforms/darwin-universal/package.json",
  "npm/platforms/linux-x64/package.json",
  "npm/platforms/linux-arm64/package.json",
  "npm/platforms/win32-x64/package.json",
  "npm/platforms/win32-arm64/package.json",
];
packages.forEach(bumpPkg);

const cargoPath = path.join(root, "Cargo.toml");
let cargo = fs.readFileSync(cargoPath, "utf8");
cargo = cargo.replace(/^version = ".*"/m, `version = "${VERSION}"`);
fs.writeFileSync(cargoPath, cargo);
console.log("bumped Cargo.toml ->", VERSION);
