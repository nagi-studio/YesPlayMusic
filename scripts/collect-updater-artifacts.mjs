import { access, copyFile, mkdir, readdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { verifyUpdaterArtifactSignature } from './verify-updater-signature.mjs';

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..'
);

export const UPDATER_ARTIFACT_SPECS = Object.freeze({
  'darwin-aarch64': {
    targetTriple: 'aarch64-apple-darwin',
    bundleDir: 'macos',
    suffix: '.app.tar.gz',
  },
  'windows-x86_64': {
    targetTriple: 'x86_64-pc-windows-msvc',
    bundleDir: 'nsis',
    suffix: '.exe',
  },
  'linux-x86_64-appimage': {
    targetTriple: 'x86_64-unknown-linux-gnu',
    bundleDir: 'appimage',
    suffix: '.AppImage',
  },
  'linux-x86_64-deb': {
    targetTriple: 'x86_64-unknown-linux-gnu',
    bundleDir: 'deb',
    suffix: '.deb',
  },
});

export async function collectUpdaterArtifacts(
  target,
  outputDir,
  root = projectRoot,
  publicKey = process.env.TAURI_UPDATER_PUBKEY
) {
  if (!publicKey) throw new Error('Missing updater public key');
  const spec = UPDATER_ARTIFACT_SPECS[target];
  if (!spec) throw new Error(`Unsupported updater target: ${target}`);
  const sourceDir = path.join(
    root,
    'src-tauri',
    'target',
    spec.targetTriple,
    'release',
    'bundle',
    spec.bundleDir
  );
  const files = await readdir(sourceDir);
  const matches = files.filter(file => file.endsWith(spec.suffix));
  if (matches.length !== 1) {
    throw new Error(
      `Expected one ${target} updater artifact, found ${matches.length}`
    );
  }

  const artifactName = matches[0];
  const artifactPath = path.join(sourceDir, artifactName);
  const signaturePath = `${artifactPath}.sig`;
  await access(signaturePath);
  await verifyUpdaterArtifactSignature(artifactPath, signaturePath, publicKey);
  await mkdir(outputDir, { recursive: true });
  await copyFile(artifactPath, path.join(outputDir, artifactName));
  await copyFile(signaturePath, path.join(outputDir, `${artifactName}.sig`));
  return { artifactName, artifactPath, signaturePath };
}

if (import.meta.main) {
  const [target, outputDir] = process.argv.slice(2);
  if (!target || !outputDir) {
    console.error('Usage: collect-updater-artifacts.mjs <target> <output-dir>');
    process.exit(1);
  }
  try {
    const result = await collectUpdaterArtifacts(target, outputDir);
    console.log(`[tauri-updater] collected ${result.artifactName}`);
  } catch (error) {
    console.error(`[tauri-updater] ${error.message}`);
    process.exit(1);
  }
}
