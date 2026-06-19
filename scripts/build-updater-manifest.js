#!/usr/bin/env node

const fs = require("fs");

const [version, repo, assetsJsonPath] = process.argv.slice(2);

if (!version || !repo || !assetsJsonPath) {
  console.error("usage: build-updater-manifest.js <version> <owner/repo> <assets.json>");
  process.exit(2);
}

const release = JSON.parse(fs.readFileSync(assetsJsonPath, "utf8"));
const assets = Array.isArray(release) ? release : release.assets || [];

function asset(name) {
  return assets.find((item) => item.name === name);
}

function sigFor(name) {
  const sigPath = `${name}.sig`;
  if (!fs.existsSync(sigPath)) {
    throw new Error(`missing signature file: ${sigPath}`);
  }
  return fs.readFileSync(sigPath, "utf8").trim();
}

function releaseUrl(name) {
  if (!asset(name)) return null;
  return `https://github.com/${repo}/releases/download/v${version}/${encodeURIComponent(name)}`;
}

const platforms = {
  "darwin-aarch64": `ThatIsOK_aarch64.app.tar.gz`,
  "darwin-x86_64": `ThatIsOK_x64.app.tar.gz`,
  "windows-x86_64": `ThatIsOK_${version}_x64-setup.exe`,
  "windows-i686": `ThatIsOK_${version}_x86-setup.exe`,
  "linux-x86_64": `ThatIsOK_${version}_amd64.AppImage.tar.gz`,
};

const requiredPlatforms = new Set(["darwin-aarch64", "windows-x86_64"]);
const resolvedPlatforms = {};
for (const [key, name] of Object.entries(platforms)) {
  const url = releaseUrl(name);
  if (!url) {
    if (requiredPlatforms.has(key)) {
      throw new Error(`missing required updater asset for ${key}: ${name}`);
    }
    continue;
  }
  const signature = sigFor(name);
  resolvedPlatforms[key] = { url, signature };
}

const manifest = {
  version,
  notes: "Bug fixes and performance improvements.",
  pub_date: new Date().toISOString(),
  platforms: resolvedPlatforms,
};

fs.writeFileSync("latest.json", `${JSON.stringify(manifest, null, 2)}\n`);
