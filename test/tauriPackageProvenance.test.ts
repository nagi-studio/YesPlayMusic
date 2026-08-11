import { afterEach, expect, test } from 'bun:test';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

import { sidecarSourceArchiveName } from '../scripts/build-sidecar-compliance.mjs';
import { assertMacBundleProvenance } from '../scripts/package-tauri-dmg.mjs';

const version = '0.7.1-canary.1';
const uuid = '00112233-4455-6677-8899-AABBCCDDEEFF';
const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(
    temporaryDirectories
      .splice(0)
      .map(directory => rm(directory, { recursive: true, force: true }))
  );
});

function fakeArm64MachO({ marker = true, uuidValue = uuid } = {}): Uint8Array {
  const markerBytes = new TextEncoder().encode('YPM_RUST_SIDECAR_V1');
  const bytes = new Uint8Array(56 + (marker ? markerBytes.length : 0));
  const view = new DataView(bytes.buffer);
  view.setUint32(0, 0xfeedfacf, true);
  view.setUint32(4, 0x0100000c, true);
  view.setUint32(16, 1, true);
  view.setUint32(20, 24, true);
  view.setUint32(32, 0x1b, true);
  view.setUint32(36, 24, true);
  bytes.set(
    Uint8Array.from(
      uuidValue
        .replaceAll('-', '')
        .match(/../g)
        ?.map(value => Number.parseInt(value, 16)) ?? []
    ),
    40
  );
  if (marker) bytes.set(markerBytes, 56);
  return bytes;
}

async function createFakeApp(manifestUuid = uuid): Promise<{
  appPath: string;
  builtSidecarPath: string;
  expectedAppComplianceDirectory: string;
}> {
  const root = await mkdtemp(path.join(os.tmpdir(), 'ypm-package-app-'));
  temporaryDirectories.push(root);
  const appPath = path.join(root, 'YesPlayMusic.app');
  const macOSDirectory = path.join(appPath, 'Contents', 'MacOS');
  const complianceDirectory = path.join(
    appPath,
    'Contents',
    'Resources',
    'sidecar-compliance'
  );
  const appComplianceDirectory = path.join(
    appPath,
    'Contents',
    'Resources',
    'app-compliance'
  );
  const expectedAppComplianceDirectory = path.join(root, 'expected-app-compliance');
  await Promise.all([
    mkdir(macOSDirectory, { recursive: true }),
    mkdir(complianceDirectory, { recursive: true }),
    mkdir(appComplianceDirectory, { recursive: true }),
    mkdir(expectedAppComplianceDirectory, { recursive: true }),
  ]);
  const appManifest = `${JSON.stringify({
    schemaVersion: 1,
    targetTriple: 'aarch64-apple-darwin',
    hostPackages: [{ name: 'tauri' }, { name: 'rusqlite' }],
    rendererPackages: [{ name: 'vue' }, { name: 'pinia' }],
  })}\n`;
  async function writeAppCompliance(directory: string) {
    const files = new Map([
      ['APP-COMPLIANCE-MANIFEST.json', appManifest],
      ['THIRD-PARTY-NOTICES.md', '# notices\n'],
      ['YESPLAYMUSIC-MIT.txt', 'MIT\n'],
    ]);
    for (const [name, content] of files) await writeFile(path.join(directory, name), content);
    const sums = [...files].map(([name, content]) =>
      `${createHash('sha256').update(content).digest('hex')}  ${name}`
    );
    await writeFile(path.join(directory, 'SHA256SUMS'), `${sums.sort().join('\n')}\n`);
  }
  await Promise.all([
    writeAppCompliance(appComplianceDirectory),
    writeAppCompliance(expectedAppComplianceDirectory),
  ]);
  const sourceAsset = sidecarSourceArchiveName(version);
  const builtSidecarPath = path.join(
    root,
    'yesplaymusic-sidecar-aarch64-apple-darwin'
  );
  const sidecarBytes = fakeArm64MachO();
  await Promise.all([
    writeFile(path.join(macOSDirectory, 'yesplaymusic-sidecar'), sidecarBytes),
    writeFile(builtSidecarPath, sidecarBytes),
    writeFile(
      path.join(complianceDirectory, 'SOURCE-MANIFEST.json'),
      `${JSON.stringify({
        schemaVersion: 2,
        sidecar: { name: 'yesplaymusic-sidecar', version },
        binaryProvenance: {
          targetTriple: 'aarch64-apple-darwin',
          fileName: path.basename(builtSidecarPath),
          sha256: createHash('sha256').update(sidecarBytes).digest('hex'),
          rustMarker: 'YPM_RUST_SIDECAR_V1',
          machOUuid: manifestUuid,
        },
        completeSource: {
          assetName: sourceAsset,
          dependencySourceCount: 1,
          offlineRebuildVerified: true,
        },
      })}\n`
    ),
    writeFile(
      path.join(complianceDirectory, 'SOURCE-OFFER.md'),
      `Download ${sourceAsset} from https://github.com/nagi-studio/YesPlayMusic/releases/download/v${version}/${sourceAsset}.\n`
    ),
  ]);
  return { appPath, builtSidecarPath, expectedAppComplianceDirectory };
}

test('macOS package gate accepts the signed-binary-stable Rust build UUID', async () => {
  const { appPath, builtSidecarPath, expectedAppComplianceDirectory } = await createFakeApp();
  await expect(
    assertMacBundleProvenance(appPath, version, builtSidecarPath, expectedAppComplianceDirectory)
  ).resolves.toEqual({
    sidecarUuid: uuid,
    builtSidecarSha256: createHash('sha256')
      .update(fakeArm64MachO())
      .digest('hex'),
    sourceArchiveName: sidecarSourceArchiveName(version),
    dependencyCount: 1,
  });
});

test('macOS package gate rejects a stale Sidecar that does not match build provenance', async () => {
  const { appPath, builtSidecarPath } = await createFakeApp(
    'FFEEDDCC-BBAA-9988-7766-554433221100'
  );
  await expect(
    assertMacBundleProvenance(appPath, version, builtSidecarPath)
  ).rejects.toThrow('与 provenance manifest 不一致');
});

test('macOS package gate rejects internally consistent but stale app notices', async () => {
  const { appPath, builtSidecarPath, expectedAppComplianceDirectory } = await createFakeApp();
  await writeFile(path.join(expectedAppComplianceDirectory, 'YESPLAYMUSIC-MIT.txt'), 'changed\n');
  const content = 'changed\n';
  const manifest = await Bun.file(path.join(expectedAppComplianceDirectory, 'APP-COMPLIANCE-MANIFEST.json')).text();
  const notice = await Bun.file(path.join(expectedAppComplianceDirectory, 'THIRD-PARTY-NOTICES.md')).text();
  const files: Array<[string, string]> = [
    ['APP-COMPLIANCE-MANIFEST.json', manifest],
    ['THIRD-PARTY-NOTICES.md', notice],
    ['YESPLAYMUSIC-MIT.txt', content],
  ];
  const sums = files.map(
    ([name, value]) => `${createHash('sha256').update(value).digest('hex')}  ${name}`
  );
  await writeFile(path.join(expectedAppComplianceDirectory, 'SHA256SUMS'), `${sums.sort().join('\n')}\n`);
  await expect(
    assertMacBundleProvenance(appPath, version, builtSidecarPath, expectedAppComplianceDirectory)
  ).rejects.toThrow('stale');
});

test('macOS package gate rejects a stale current build artifact even when the bundle matches its manifest', async () => {
  const { appPath, builtSidecarPath } = await createFakeApp();
  await writeFile(
    builtSidecarPath,
    fakeArm64MachO({
      uuidValue: 'FFEEDDCC-BBAA-9988-7766-554433221100',
    })
  );
  await expect(
    assertMacBundleProvenance(appPath, version, builtSidecarPath)
  ).rejects.toThrow('与 provenance manifest 不一致');
});

test('macOS package gate rejects a legacy runtime payload', async () => {
  const { appPath, builtSidecarPath } = await createFakeApp();
  await writeFile(
    path.join(appPath, 'Contents', 'Resources', 'sidecar.payload'),
    'legacy payload'
  );
  await expect(
    assertMacBundleProvenance(appPath, version, builtSidecarPath)
  ).rejects.toThrow('含有禁止分发的旧运行时');
});

test('macOS package gate rejects an arm64 non-Rust Sidecar without the production marker', async () => {
  const { appPath, builtSidecarPath } = await createFakeApp();
  await writeFile(
    path.join(appPath, 'Contents', 'MacOS', 'yesplaymusic-sidecar'),
    fakeArm64MachO({ marker: false })
  );
  await expect(
    assertMacBundleProvenance(appPath, version, builtSidecarPath)
  ).rejects.toThrow('缺少 Rust production marker');
});
