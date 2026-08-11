#!/usr/bin/env bun
import { readdir } from 'node:fs/promises';
import path from 'node:path';
import {
  assertAppComplianceMatchesDirectory,
  defaultAppComplianceOutput,
} from './build-app-compliance.mjs';
import { withExtractedUpdaterArtifact } from './read-updater-artifact-version.mjs';

const supportedTargets = new Set([
  'windows-x86_64',
  'linux-x86_64-appimage',
  'linux-x86_64-deb',
]);

async function findComplianceDirectories(directory) {
  const matches = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isSymbolicLink()) continue;
    if (!entry.isDirectory()) continue;
    const children = await readdir(entryPath);
    if (children.includes('APP-COMPLIANCE-MANIFEST.json')) {
      matches.push(entryPath);
    }
    matches.push(...(await findComplianceDirectories(entryPath)));
  }
  return matches;
}

export async function verifyPackagedAppCompliance(
  target,
  artifactPath,
  expectedDirectory = defaultAppComplianceOutput,
  execute
) {
  if (!supportedTargets.has(target)) {
    throw new Error(`Unsupported packaged compliance target: ${target}`);
  }
  return withExtractedUpdaterArtifact(
    target,
    artifactPath,
    async extractionDirectory => {
      const matches = await findComplianceDirectories(extractionDirectory);
      if (matches.length !== 1) {
        throw new Error(
          `Expected one packaged app-compliance directory, found ${matches.length}`
        );
      }
      await assertAppComplianceMatchesDirectory(matches[0], expectedDirectory);
      return matches[0];
    },
    execute,
    { expandWindowsNested: true }
  );
}

async function main() {
  const [target, artifactPath] = process.argv.slice(2);
  if (!target || !artifactPath) {
    throw new Error(
      'Usage: verify-packaged-app-compliance.mjs <target> <artifact-path>'
    );
  }
  await verifyPackagedAppCompliance(target, artifactPath);
  console.log(`[app-compliance] verified ${target}: ${artifactPath}`);
}

if (import.meta.main) {
  main().catch(error => {
    console.error(`[app-compliance] ${error.message}`);
    process.exit(1);
  });
}
