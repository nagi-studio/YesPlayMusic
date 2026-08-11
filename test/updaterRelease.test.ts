import { expect, test } from 'bun:test';
import { mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import {
  mkdir,
  mkdtemp,
  rm,
  rmdir,
  symlink,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { collectUpdaterArtifacts } from '../scripts/collect-updater-artifacts.mjs';
import { createUpdaterManifest } from '../scripts/generate-updater-manifest.mjs';
import { readUpdaterArtifactVersion } from '../scripts/read-updater-artifact-version.mjs';
import { verifyUpdaterReleaseEnvironment } from '../scripts/verify-updater-release-env.mjs';
import {
  CANARY_UPDATER_ENDPOINT,
  createUpdaterBuildConfig,
} from '../scripts/build-tauri-updater.mjs';
import { resolveTauriSmokeExecutable } from '../scripts/smoke-tauri-local.mjs';
import { verifyCanaryUpdaterFeedAdvance } from '../scripts/verify-updater-feed-advance.mjs';

const packageJson = JSON.parse(
  readFileSync(new URL('../package.json', import.meta.url), 'utf8')
);
const tauriConfig = JSON.parse(
  readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8')
);
const updaterConfig = JSON.parse(
  readFileSync(
    new URL('../src-tauri/tauri.updater.conf.json', import.meta.url),
    'utf8'
  )
);
const capabilities = JSON.parse(
  readFileSync(
    new URL('../src-tauri/capabilities/default.json', import.meta.url),
    'utf8'
  )
);
const cargo = readFileSync(
  new URL('../src-tauri/Cargo.toml', import.meta.url),
  'utf8'
);
const rustMain = readFileSync(
  new URL('../src-tauri/src/main.rs', import.meta.url),
  'utf8'
);
const workflow = readFileSync(
  new URL('../.github/workflows/build.yaml', import.meta.url),
  'utf8'
);
const canaryFeedWorkflow = readFileSync(
  new URL(
    '../.github/workflows/publish-canary-updater-feed.yaml',
    import.meta.url
  ),
  'utf8'
);

const updaterTestPublicKey = Buffer.from(
  [
    'untrusted comment: minisign public key E7620F1842B4E81F',
    'RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3',
  ].join('\n')
).toString('base64');
const updaterWrongKeyPayload = Buffer.from(
  'RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3',
  'base64'
);
updaterWrongKeyPayload[2] = (updaterWrongKeyPayload[2] ?? 0) ^ 1;
const updaterWrongPublicKey = Buffer.from(
  [
    'untrusted comment: minisign public key with a different key ID',
    updaterWrongKeyPayload.toString('base64'),
  ].join('\n')
).toString('base64');
const updaterTestSignature = Buffer.from(
  [
    'untrusted comment: signature from minisign secret key',
    'RWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=',
    'trusted comment: timestamp:1555779966\tfile:test',
    'QtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==',
  ].join('\n')
).toString('base64');

async function writeSignedUpdaterFixtures(root: string): Promise<void> {
  for (const [directory, name] of [
    ['macos', 'YesPlayMusic.app.tar.gz'],
    ['windows', 'YesPlayMusic_0.7.0_x64-setup.exe'],
    ['linux-appimage', 'YesPlayMusic_0.7.0_amd64.AppImage'],
    ['linux-deb', 'YesPlayMusic_0.7.0_amd64.deb'],
  ] as const) {
    const target = path.join(root, directory);
    await mkdir(target, { recursive: true });
    await writeFile(path.join(target, name), 'test');
    await writeFile(path.join(target, `${name}.sig`), updaterTestSignature);
  }
}

test('macOS deployment target matches the proxy API requirement', () => {
  expect(cargo).toContain('"macos-proxy"');
  expect(tauriConfig.bundle.macOS.minimumSystemVersion).toBe('14.0');
});

test('desktop builds use the official Tauri updater and process plugins', () => {
  expect(packageJson.dependencies['@tauri-apps/plugin-updater']).toBeTruthy();
  expect(packageJson.dependencies['@tauri-apps/plugin-process']).toBeTruthy();
  expect(cargo).toContain('tauri-plugin-updater = "=2.10.1"');
  expect(cargo).toContain('tauri-plugin-process = "2"');
  expect(capabilities.permissions).toContain('updater:default');
  expect(capabilities.permissions).toContain('process:allow-restart');
  expect(tauriConfig.plugins.updater.endpoints).toEqual([
    'https://github.com/nagi-studio/YesPlayMusic/releases/latest/download/latest.json',
  ]);
  expect(tauriConfig.bundle.createUpdaterArtifacts).toBe(false);
  expect(updaterConfig.bundle.createUpdaterArtifacts).toBe(true);
  expect(tauriConfig.bundle.resources['tauri.conf.json']).toBe(
    'updater-artifact-version.json'
  );
});

test('tag CI signs all updater targets and publishes latest.json', () => {
  expect(workflow).toContain('secrets.TAURI_SIGNING_PRIVATE_KEY');
  expect(workflow).toContain('vars.TAURI_UPDATER_PUBKEY');
  expect(workflow).toContain('build:tauri:macos:updater');
  expect(workflow).toContain('build:tauri:windows:updater');
  expect(workflow).toContain('build:tauri:linux:updater');
  expect(workflow).toContain('verify-macos-updater.mjs');
  expect(workflow).toContain('generate-updater-manifest.mjs');
  expect(workflow).toContain('release/latest.json');
  expect(workflow).toContain('p7zip-full squashfs-tools');
  expect(workflow).toContain(
    'TAURI_UPDATER_PUBKEY: ${{ vars.TAURI_UPDATER_PUBKEY }}'
  );
});

test('published canaries advance a separate verified updater feed', () => {
  expect(canaryFeedWorkflow).toContain('types: [published]');
  expect(canaryFeedWorkflow).toContain(
    "contains(github.event.release.tag_name, '-canary.')"
  );
  expect(canaryFeedWorkflow).toContain('generate-updater-manifest.mjs');
  expect(canaryFeedWorkflow).toContain('YPM_RELEASE_PUBLISHED_AT');
  expect(canaryFeedWorkflow).toContain('p7zip-full squashfs-tools');
  expect(canaryFeedWorkflow).toContain('updater-feed');
  expect(canaryFeedWorkflow).toContain('channels/canary.json');

  expect(
    verifyCanaryUpdaterFeedAdvance(
      { version: '0.7.1-canary.1' },
      { version: '0.7.1-canary.2' }
    )
  ).toBe(true);
  expect(
    verifyCanaryUpdaterFeedAdvance(
      { version: '0.7.1-canary.2' },
      { version: '0.7.1-canary.2' }
    )
  ).toBe(true);
  expect(() =>
    verifyCanaryUpdaterFeedAdvance(
      { version: '0.7.2-canary.1' },
      { version: '0.7.1-canary.9' }
    )
  ).toThrow('backwards');
  expect(() =>
    verifyCanaryUpdaterFeedAdvance(
      {
        version: '0.7.1-canary.2',
        platforms: { 'darwin-aarch64': { signature: 'old', url: 'old' } },
      },
      {
        version: '0.7.1-canary.2',
        platforms: { 'darwin-aarch64': { signature: 'new', url: 'new' } },
      }
    )
  ).toThrow('same version');
});

test('release builds inject the public key into createUpdaterArtifacts config', async () => {
  const current = await createUpdaterBuildConfig(updaterTestPublicKey);
  const explicitCurrent = await createUpdaterBuildConfig(
    updaterTestPublicKey,
    packageJson.version
  );
  const stable = await createUpdaterBuildConfig(updaterTestPublicKey, '0.7.1');
  const canary = await createUpdaterBuildConfig(
    updaterTestPublicKey,
    '0.7.2-canary.1'
  );
  expect(stable).toMatchObject({
    bundle: { createUpdaterArtifacts: true },
    plugins: {
      updater: {
        pubkey: updaterTestPublicKey,
        endpoints: [
          'https://github.com/nagi-studio/YesPlayMusic/releases/latest/download/latest.json',
        ],
      },
    },
  });
  expect(canary).toMatchObject({
    plugins: {
      updater: {
        endpoints: [CANARY_UPDATER_ENDPOINT],
      },
    },
  });
  expect(current).toEqual(explicitCurrent);
  await expect(
    createUpdaterBuildConfig(updaterTestPublicKey, '0.7.2-beta.1')
  ).rejects.toThrow('Unsupported updater prerelease channel');
});

test('updater manifest rejects an artifact not signed by the injected public key', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'yesplaymusic-updater-test-'));
  try {
    await writeSignedUpdaterFixtures(root);
    await writeFile(
      path.join(root, 'windows', 'YesPlayMusic_0.7.0_x64-setup.exe'),
      'tampered'
    );

    await expect(
      createUpdaterManifest({
        artifactsDir: root,
        version: '0.7.0',
        publishedAt: '2026-08-10T00:00:00Z',
        publicKey: updaterTestPublicKey,
        artifactVersionReader: async () => '0.7.0',
      })
    ).rejects.toThrow('signature');
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('updater manifest rejects signatures created for another injected public key', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'yesplaymusic-updater-test-'));
  try {
    await writeSignedUpdaterFixtures(root);
    await expect(
      createUpdaterManifest({
        artifactsDir: root,
        version: '0.7.0',
        publishedAt: '2026-08-10T00:00:00Z',
        publicKey: updaterWrongPublicKey,
      })
    ).rejects.toThrow('key ID does not match');
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('updater manifest rejects an older signed artifact relabeled as a newer canary', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'yesplaymusic-updater-test-'));
  try {
    await writeSignedUpdaterFixtures(root);
    await expect(
      createUpdaterManifest({
        artifactsDir: root,
        version: '0.7.1-canary.2',
        tag: 'v0.7.1-canary.2',
        publishedAt: '2026-08-11T00:00:00Z',
        publicKey: updaterTestPublicKey,
        artifactVersionReader: async () => '0.7.0',
      })
    ).rejects.toThrow('artifact version');
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('updater manifest requires tag and manifest versions to match exactly', async () => {
  await expect(
    createUpdaterManifest({
      artifactsDir: '.',
      version: '0.7.1-canary.2',
      tag: 'v0.7.1-canary.1',
      publicKey: updaterTestPublicKey,
    })
  ).rejects.toThrow('tag/version mismatch');
});

test('updater release configuration requires a private and public key', () => {
  expect(() => verifyUpdaterReleaseEnvironment({})).toThrow(
    'TAURI_SIGNING_PRIVATE_KEY, TAURI_SIGNING_PRIVATE_KEY_PASSWORD, TAURI_UPDATER_PUBKEY'
  );
  expect(
    verifyUpdaterReleaseEnvironment({
      TAURI_SIGNING_PRIVATE_KEY: 'private-key',
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: 'private-key-password',
      TAURI_UPDATER_PUBKEY: updaterTestPublicKey,
    })
  ).toBe(true);
  expect(() =>
    verifyUpdaterReleaseEnvironment({
      TAURI_SIGNING_PRIVATE_KEY: 'private-key',
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: 'private-key-password',
      TAURI_UPDATER_PUBKEY: 'not-a-minisign-key',
    })
  ).toThrow('updater public key');
});

test('latest.json maps every supported target to its signed release asset', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'yesplaymusic-updater-test-'));
  try {
    await writeSignedUpdaterFixtures(root);

    const manifest = await createUpdaterManifest({
      artifactsDir: root,
      version: '0.7.0',
      publishedAt: '2026-08-10T00:00:00Z',
      publicKey: updaterTestPublicKey,
      artifactVersionReader: async () => '0.7.0',
    });
    expect(Object.keys(manifest.platforms).sort()).toEqual([
      'darwin-aarch64',
      'linux-x86_64-appimage',
      'linux-x86_64-deb',
      'windows-x86_64',
    ]);
    expect(manifest.platforms['darwin-aarch64']?.url).toEndWith(
      '/v0.7.0/YesPlayMusic.app.tar.gz'
    );
    expect(manifest.platforms['windows-x86_64']?.signature).toBe(
      updaterTestSignature
    );
    expect(manifest.platforms['linux-x86_64-appimage']?.url).toEndWith(
      '/v0.7.0/YesPlayMusic_0.7.0_amd64.AppImage'
    );
    expect(manifest.platforms['linux-x86_64-deb']?.url).toEndWith(
      '/v0.7.0/YesPlayMusic_0.7.0_amd64.deb'
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('macOS updater archive exposes the signed embedded app version', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'yesplaymusic-version-test-'));
  try {
    const resources = path.join(root, 'YesPlayMusic.app/Contents/Resources');
    await mkdir(resources, { recursive: true });
    await writeFile(
      path.join(resources, 'updater-artifact-version.json'),
      JSON.stringify({ version: '0.7.1-canary.2' })
    );
    const archive = path.join(root, 'YesPlayMusic.app.tar.gz');
    const tar = Bun.spawnSync(
      ['tar', '-czf', archive, '-C', root, 'YesPlayMusic.app'],
      { stdout: 'pipe', stderr: 'pipe' }
    );
    expect(tar.exitCode).toBe(0);
    expect(await readUpdaterArtifactVersion('darwin-aarch64', archive)).toBe(
      '0.7.1-canary.2'
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('Windows, AppImage and deb readers require the embedded app version resource', async () => {
  const cases = [
    ['windows-x86_64', '7z'],
    ['linux-x86_64-appimage', 'unsquashfs'],
    ['linux-x86_64-deb', 'dpkg-deb'],
  ] as const;
  for (const [target, expectedCommand] of cases) {
    const root = await mkdtemp(
      path.join(tmpdir(), 'yesplaymusic-version-test-')
    );
    try {
      const artifact = path.join(root, 'artifact');
      const content = new Uint8Array(new ArrayBuffer(64));
      if (target === 'linux-x86_64-appimage') {
        content.set(new TextEncoder().encode('hsqs'), 0);
        const view = new DataView(content.buffer);
        view.setUint32(12, 131_072, true);
        view.setUint16(22, 17, true);
        view.setUint16(28, 4, true);
      }
      await writeFile(artifact, content);
      let called = false;
      const version = await readUpdaterArtifactVersion(
        target,
        artifact,
        (command, args) => {
          called = true;
          expect(command).toBe(expectedCommand);
          const output =
            target === 'windows-x86_64'
              ? args.find(value => value.startsWith('-o'))?.slice(2)
              : target === 'linux-x86_64-appimage'
              ? args[args.indexOf('-d') + 1]
              : args.at(-1);
          if (!output) throw new Error('missing extraction output');
          mkdirSync(output, { recursive: true });
          writeFileSync(
            path.join(output, 'updater-artifact-version.json'),
            JSON.stringify({ version: '0.7.1-canary.2' })
          );
        }
      );
      expect(called).toBe(true);
      expect(version).toBe('0.7.1-canary.2');
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  }
});

test('Linux updater selects a manifest target from the installed bundle type', () => {
  expect(rustMain).toContain('BundleType::AppImage');
  expect(rustMain).toContain('"linux-x86_64-appimage"');
  expect(rustMain).toContain('BundleType::Deb');
  expect(rustMain).toContain('"linux-x86_64-deb"');
});

test('Linux updater collector preserves both signed package formats', async () => {
  const root = await mkdtemp(
    path.join(tmpdir(), 'yesplaymusic-collector-test-')
  );
  const output = path.join(root, 'output');
  try {
    const bundleRoot = path.join(
      root,
      'src-tauri/target/x86_64-unknown-linux-gnu/release/bundle'
    );
    for (const [directory, name] of [
      ['appimage', 'YesPlayMusic_0.7.0_amd64.AppImage'],
      ['deb', 'YesPlayMusic_0.7.0_amd64.deb'],
    ] as const) {
      const source = path.join(bundleRoot, directory);
      await mkdir(source, { recursive: true });
      await writeFile(path.join(source, name), 'test');
      await writeFile(path.join(source, `${name}.sig`), updaterTestSignature);
    }

    const appImage = await collectUpdaterArtifacts(
      'linux-x86_64-appimage',
      output,
      root,
      updaterTestPublicKey
    );
    const deb = await collectUpdaterArtifacts(
      'linux-x86_64-deb',
      output,
      root,
      updaterTestPublicKey
    );
    expect(appImage.artifactName).toEndWith('.AppImage');
    expect(deb.artifactName).toEndWith('.deb');
    expect(
      await Bun.file(path.join(output, `${appImage.artifactName}.sig`)).text()
    ).toBe(updaterTestSignature);
    expect(
      await Bun.file(path.join(output, `${deb.artifactName}.sig`)).text()
    ).toBe(updaterTestSignature);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('core smoke launches each platform from its packaged runtime layout', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'yesplaymusic-smoke-test-'));
  try {
    expect(
      resolveTauriSmokeExecutable({ platform: 'darwin', arch: 'arm64', root })
    ).toBe(
      path.join(
        root,
        'src-tauri',
        'target',
        'aarch64-apple-darwin',
        'release',
        'bundle',
        'macos',
        'YesPlayMusic.app',
        'Contents',
        'MacOS',
        'yesplaymusic-tauri'
      )
    );
    expect(
      resolveTauriSmokeExecutable({ platform: 'win32', arch: 'x64', root })
    ).toBe(
      path.join(
        root,
        'src-tauri',
        'target',
        'x86_64-pc-windows-msvc',
        'release',
        'yesplaymusic-tauri.exe'
      )
    );

    const appImageDirectory = path.join(
      root,
      'src-tauri/target/x86_64-unknown-linux-gnu/release/bundle/appimage'
    );
    await mkdir(appImageDirectory, { recursive: true });
    await writeFile(
      path.join(appImageDirectory, 'YesPlayMusic_0.7.0_amd64.AppImage'),
      'artifact'
    );
    expect(
      resolveTauriSmokeExecutable({ platform: 'linux', arch: 'x64', root })
    ).toBe(path.join(appImageDirectory, 'YesPlayMusic_0.7.0_amd64.AppImage'));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('installed smoke canonicalizes symlinked launch paths before Tauri resolves resources', async () => {
  const realRoot = await mkdtemp(
    path.join(tmpdir(), 'yesplaymusic-smoke-real-')
  );
  const aliasRoot = `${realRoot}-alias`;
  try {
    const executable = path.join(realRoot, 'yesplaymusic-tauri');
    await writeFile(executable, 'artifact');
    await symlink(realRoot, aliasRoot, 'dir');

    expect(
      resolveTauriSmokeExecutable({
        executablePath: path.join(aliasRoot, 'yesplaymusic-tauri'),
      })
    ).toBe(realpathSync(executable));
  } finally {
    // Windows needs rmdir for a directory symlink; unlink/rm rejects it.
    if (process.platform === 'win32') {
      await rmdir(aliasRoot).catch(() => undefined);
    } else {
      await rm(aliasRoot, { force: true });
    }
    await rm(realRoot, { recursive: true, force: true });
  }
});
