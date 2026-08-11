import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import {
  validateUpdaterPublicKey,
  verifyUpdaterArtifactSignature,
} from './verify-updater-signature.mjs';
import { readUpdaterArtifactVersion } from './read-updater-artifact-version.mjs';

export const UPDATER_MANIFEST_SUFFIXES = Object.freeze({
  'darwin-aarch64': '.app.tar.gz',
  'windows-x86_64': '.exe',
  'linux-x86_64-appimage': '.AppImage',
  'linux-x86_64-deb': '.deb',
});

async function walk(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...(await walk(entryPath)));
    else files.push(entryPath);
  }
  return files;
}

export async function createUpdaterManifest({
  artifactsDir,
  version,
  repository = 'nagi-studio/YesPlayMusic',
  tag = `v${version}`,
  publishedAt = new Date().toISOString(),
  publicKey = process.env.TAURI_UPDATER_PUBKEY,
  artifactVersionReader = readUpdaterArtifactVersion,
}) {
  if (!publicKey) throw new Error('Missing updater public key');
  if (tag !== `v${version}`) {
    throw new Error(
      `Updater manifest tag/version mismatch: ${tag} != v${version}`
    );
  }
  validateUpdaterPublicKey(publicKey);
  const files = await walk(artifactsDir);
  const platforms = {};
  for (const [target, suffix] of Object.entries(UPDATER_MANIFEST_SUFFIXES)) {
    const matches = files.filter(
      file => file.endsWith(suffix) && !file.endsWith('.sig')
    );
    if (matches.length !== 1) {
      throw new Error(
        `Expected one ${target} updater artifact, found ${matches.length}`
      );
    }
    const artifact = matches[0];
    const signaturePath = `${artifact}.sig`;
    const signature = (await readFile(signaturePath, 'utf8')).trim();
    if (!signature) throw new Error(`Empty updater signature: ${artifact}.sig`);
    await verifyUpdaterArtifactSignature(artifact, signaturePath, publicKey);
    const artifactVersion = await artifactVersionReader(target, artifact);
    if (artifactVersion !== version) {
      throw new Error(
        `${target} artifact version does not match updater manifest: ${artifactVersion} != ${version}`
      );
    }
    const name = path.basename(artifact);
    platforms[target] = {
      signature,
      url: `https://github.com/${repository}/releases/download/${tag}/${encodeURIComponent(
        name
      )}`,
    };
  }
  return {
    version,
    notes: `YesPlayMusic ${version}`,
    pub_date: publishedAt,
    platforms,
  };
}

export async function writeUpdaterManifest(options, outputPath) {
  const manifest = await createUpdaterManifest(options);
  await mkdir(path.dirname(outputPath), { recursive: true });
  await writeFile(outputPath, `${JSON.stringify(manifest, null, 2)}\n`, 'utf8');
  return manifest;
}

if (import.meta.main) {
  const [artifactsDir, outputPath, version, tag, publishedAt] =
    process.argv.slice(2);
  if (!artifactsDir || !outputPath || !version) {
    console.error(
      'Usage: generate-updater-manifest.mjs <artifacts-dir> <output> <version> [tag] [published-at]'
    );
    process.exit(1);
  }
  try {
    await writeUpdaterManifest(
      {
        artifactsDir,
        version,
        tag: tag || `v${version}`,
        publishedAt: publishedAt || new Date().toISOString(),
      },
      outputPath
    );
    console.log(`[tauri-updater] manifest: ${outputPath}`);
  } catch (error) {
    console.error(`[tauri-updater] ${error.message}`);
    process.exit(1);
  }
}
