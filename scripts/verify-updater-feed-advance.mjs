import { readFile } from 'node:fs/promises';
import { isDeepStrictEqual } from 'node:util';

function parseCanaryVersion(version) {
  const match = version.match(
    /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)-canary\.(0|[1-9]\d*)(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/
  );
  if (!match) throw new Error(`Invalid canary updater version: ${version}`);
  return match.slice(1, 5).map(BigInt);
}

export function compareCanaryUpdaterVersions(left, right) {
  const leftParts = parseCanaryVersion(left);
  const rightParts = parseCanaryVersion(right);
  for (let index = 0; index < leftParts.length; index += 1) {
    if (leftParts[index] < rightParts[index]) return -1;
    if (leftParts[index] > rightParts[index]) return 1;
  }
  return 0;
}

export function verifyCanaryUpdaterFeedAdvance(current, next) {
  const comparison = compareCanaryUpdaterVersions(
    next.version,
    current.version
  );
  if (comparison < 0) {
    throw new Error(
      `Refusing to move canary updater feed backwards: ${current.version} -> ${next.version}`
    );
  }
  if (comparison === 0 && !isDeepStrictEqual(current, next)) {
    throw new Error(
      `Refusing to change canary updater feed content for the same version: ${next.version}`
    );
  }
  return true;
}

if (import.meta.main) {
  const [currentPath, nextPath] = process.argv.slice(2);
  if (!currentPath || !nextPath) {
    console.error(
      'Usage: verify-updater-feed-advance.mjs <current-manifest> <next-manifest>'
    );
    process.exit(1);
  }
  try {
    const [current, next] = await Promise.all(
      [currentPath, nextPath].map(async file =>
        JSON.parse(await readFile(file, 'utf8'))
      )
    );
    verifyCanaryUpdaterFeedAdvance(current, next);
    console.log(
      `[tauri-updater] canary feed: ${current.version} -> ${next.version}`
    );
  } catch (error) {
    console.error(`[tauri-updater] ${error.message}`);
    process.exit(1);
  }
}
