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
  if (!asset(name)) {
    throw new Error(`missing release asset: ${name}`);
  }
  return `https://github.com/${repo}/releases/download/v${version}/${encodeURIComponent(name)}`;
}

const macUpdater = `ThatIsOK_aarch64.app.tar.gz`;
const windowsUpdater = `ThatIsOK_${version}_x64-setup.exe`;

const manifest = {
  version,
  notes: "Bug fixes and improvements.",
  pub_date: new Date().toISOString(),
  platforms: {
    "darwin-aarch64": {
      url: releaseUrl(macUpdater),
      signature: sigFor(macUpdater),
    },
    "windows-x86_64": {
      url: releaseUrl(windowsUpdater),
      signature: sigFor(windowsUpdater),
    },
  },
};

fs.writeFileSync("latest.json", `${JSON.stringify(manifest, null, 2)}\n`);
