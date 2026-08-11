import { mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';

export const UPDATER_ARTIFACT_VERSION_RESOURCE =
  'updater-artifact-version.json';

function run(command, args) {
  const result = Bun.spawnSync([command, ...args], {
    stdout: 'pipe',
    stderr: 'pipe',
  });
  if (result.exitCode !== 0) {
    const stderr = new TextDecoder().decode(result.stderr || '').trim();
    throw new Error(
      `${command} failed with exit code ${result.exitCode}${
        stderr ? `: ${stderr}` : ''
      }`
    );
  }
}

async function walk(directory) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...(await walk(entryPath)));
    else if (entry.isFile()) files.push(entryPath);
  }
  return files;
}

async function readExtractedVersion(directory) {
  const matches = (await walk(directory)).filter(
    file => path.basename(file) === UPDATER_ARTIFACT_VERSION_RESOURCE
  );
  if (matches.length !== 1) {
    throw new Error(
      `Expected one embedded updater artifact version resource, found ${matches.length}`
    );
  }
  let metadata;
  try {
    metadata = JSON.parse(await readFile(matches[0], 'utf8'));
  } catch (error) {
    throw new Error(
      `Invalid embedded updater artifact version resource: ${error.message}`
    );
  }
  if (typeof metadata?.version !== 'string' || !metadata.version.trim()) {
    throw new Error('Missing embedded updater artifact version');
  }
  return metadata.version;
}

function isSquashfsSuperblock(buffer, offset) {
  if (offset + 32 > buffer.length) return false;
  const blockSize = buffer.readUInt32LE(offset + 12);
  const blockLog = buffer.readUInt16LE(offset + 22);
  return (
    blockSize >= 4096 &&
    blockSize <= 1024 * 1024 &&
    (blockSize & (blockSize - 1)) === 0 &&
    2 ** blockLog === blockSize &&
    buffer.readUInt16LE(offset + 28) === 4 &&
    buffer.readUInt16LE(offset + 30) === 0
  );
}

export function findSquashfsOffset(buffer) {
  const magic = Buffer.from('hsqs');
  let offset = buffer.indexOf(magic);
  while (offset !== -1) {
    if (isSquashfsSuperblock(buffer, offset)) return offset;
    offset = buffer.indexOf(magic, offset + 1);
  }
  throw new Error('AppImage does not contain a SquashFS v4 filesystem');
}

async function extractWindowsArtifact(
  artifactPath,
  outputDirectory,
  execute,
  expandNested
) {
  execute('7z', ['x', '-y', `-o${outputDirectory}`, artifactPath]);
  const extracted = await walk(outputDirectory);
  if (
    !expandNested &&
    extracted.some(
      file => path.basename(file) === UPDATER_ARTIFACT_VERSION_RESOURCE
    )
  ) {
    return;
  }
  const nestedArchives = extracted.filter(file => file.endsWith('.7z'));
  for (const [index, archive] of nestedArchives.entries()) {
    execute('7z', [
      'x',
      '-y',
      `-o${path.join(outputDirectory, `nested-${index}`)}`,
      archive,
    ]);
  }
}

export async function readUpdaterArtifactVersion(
  target,
  artifactPath,
  execute = run
) {
  return withExtractedUpdaterArtifact(
    target,
    artifactPath,
    directory => readExtractedVersion(directory),
    execute
  );
}

export async function withExtractedUpdaterArtifact(
  target,
  artifactPath,
  inspect,
  execute = run,
  { expandWindowsNested = false } = {}
) {
  const outputDirectory = await mkdtemp(
    path.join(tmpdir(), 'yesplaymusic-updater-version-')
  );
  try {
    switch (target) {
      case 'darwin-aarch64':
        execute('tar', ['-xzf', artifactPath, '-C', outputDirectory]);
        break;
      case 'windows-x86_64':
        await extractWindowsArtifact(
          artifactPath,
          outputDirectory,
          execute,
          expandWindowsNested
        );
        break;
      case 'linux-x86_64-appimage': {
        const offset = findSquashfsOffset(await readFile(artifactPath));
        execute('unsquashfs', [
          '-f',
          '-o',
          String(offset),
          '-d',
          outputDirectory,
          artifactPath,
        ]);
        break;
      }
      case 'linux-x86_64-deb':
        execute('dpkg-deb', ['-x', artifactPath, outputDirectory]);
        break;
      default:
        throw new Error(`Unsupported updater target: ${target}`);
    }
    return await inspect(outputDirectory);
  } catch (error) {
    throw new Error(
      `Failed to read ${target} artifact version from ${artifactPath}: ${error.message}`
    );
  } finally {
    await rm(outputDirectory, { recursive: true, force: true });
  }
}
