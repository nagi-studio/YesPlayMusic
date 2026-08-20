#!/usr/bin/env bun
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { verifyUpdaterReleaseEnvironment } from './verify-updater-release-env.mjs';

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..'
);

export const UPDATER_BUILD_PLANS = Object.freeze({
  'darwin-aarch64': {
    targetTriple: 'aarch64-apple-darwin',
    args: [],
    afterBuild: [
      ['run', 'sign:tauri:local'],
      ['scripts/refresh-macos-updater.mjs'],
    ],
  },
  'windows-x86_64': {
    targetTriple: 'x86_64-pc-windows-msvc',
    args: ['--bundles', 'nsis', '--ci'],
    afterBuild: [],
  },
  'linux-x86_64': {
    targetTriple: 'x86_64-unknown-linux-gnu',
    args: ['--verbose', '--bundles', 'deb,appimage', '--ci'],
    afterBuild: [],
  },
});

export const CANARY_UPDATER_ENDPOINT =
  'https://raw.githubusercontent.com/nagi-studio/YesPlayMusic/updater-feed/channels/canary.json';

function updaterChannel(version) {
  const match = version.match(
    /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/
  );
  if (!match) throw new Error(`Invalid updater version: ${version}`);
  const prerelease = match[1];
  if (!prerelease) return 'stable';
  if (/^canary\.(?:0|[1-9]\d*)$/.test(prerelease)) return 'canary';
  throw new Error(`Unsupported updater prerelease channel: ${version}`);
}

export async function createUpdaterBuildConfig(publicKey, version) {
  const [base, tauriConfig, packageJson] = await Promise.all(
    [
      'src-tauri/tauri.updater.conf.json',
      'src-tauri/tauri.conf.json',
      'package.json',
    ].map(async file =>
      JSON.parse(await readFile(path.join(projectRoot, file), 'utf8'))
    )
  );
  const channel = updaterChannel(version ?? packageJson.version);
  const endpoints =
    channel === 'canary'
      ? [CANARY_UPDATER_ENDPOINT]
      : tauriConfig.plugins.updater.endpoints;
  return {
    ...base,
    plugins: {
      ...base.plugins,
      updater: {
        ...base.plugins?.updater,
        pubkey: publicKey,
        endpoints,
      },
    },
  };
}

/// Bundle targets for a build. The Developer ID path is the only one that
/// overrides its plan, because it has to produce the DMG that ships.
///
/// `app` must stay in that list even though only the DMG is published. Drop
/// it and Tauri deletes bundle/macos/YesPlayMusic.app the moment the DMG is
/// built, which breaks three consumers at once: the signature/staple
/// validation gate, collect:tauri:release-dmg (it re-verifies the .app and
/// reads its provenance), and the updater artifacts — those come only from an
/// updater-enabled target, so a dmg-only build silently ships no macOS
/// auto-update at all. v0.9.2's first signed build failed on exactly this.
export function updaterBuildArgs(target, { developerId = false } = {}) {
  const plan = UPDATER_BUILD_PLANS[target];
  if (!plan) throw new Error(`Unsupported updater target: ${target}`);
  return target === 'darwin-aarch64' && developerId
    ? ['--bundles', 'app,dmg', '--ci']
    : plan.args;
}

function run(args) {
  const result = Bun.spawnSync([process.execPath, ...args], {
    cwd: projectRoot,
    env: process.env,
    stdout: 'inherit',
    stderr: 'inherit',
  });
  if (result.exitCode !== 0) {
    throw new Error(
      `bun ${args.join(' ')} failed with exit code ${result.exitCode}`
    );
  }
}

export async function buildTauriUpdater(target, { developerId = false } = {}) {
  verifyUpdaterReleaseEnvironment();
  const plan = UPDATER_BUILD_PLANS[target];
  if (!plan) throw new Error(`Unsupported updater target: ${target}`);
  const temporaryDirectory = await mkdtemp(
    path.join(tmpdir(), 'yesplaymusic-updater-config-')
  );
  const configPath = path.join(temporaryDirectory, 'updater.json');
  try {
    const config = await createUpdaterBuildConfig(
      process.env.TAURI_UPDATER_PUBKEY
    );
    await writeFile(configPath, JSON.stringify(config), 'utf8');
    const buildArgs = updaterBuildArgs(target, { developerId });
    run([
      'tauri',
      'build',
      '--target',
      plan.targetTriple,
      ...buildArgs,
      '--config',
      configPath,
    ]);
    if (!developerId) {
      for (const command of plan.afterBuild) run(command);
    }
    return plan;
  } finally {
    await rm(temporaryDirectory, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  try {
    await buildTauriUpdater(process.argv[2], {
      developerId: process.argv.includes('--developer-id'),
    });
  } catch (error) {
    console.error(`[tauri-updater] ${error.message}`);
    process.exit(1);
  }
}
