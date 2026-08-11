import { afterEach, describe, expect, test } from 'bun:test';
import { cpSync, mkdirSync, symlinkSync, writeFileSync } from 'node:fs';
import {
  mkdir,
  mkdtemp,
  readFile,
  rm,
  symlink,
  writeFile,
} from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {
  buildAppCompliance,
  collectCargoRuntimePackages,
  collectRendererPackageJsonPaths,
  verifyAppComplianceDirectory,
} from '../scripts/build-app-compliance.mjs';
import { verifyPackagedAppCompliance } from '../scripts/verify-packaged-app-compliance.mjs';

const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(
    temporaryDirectories.splice(0).map(directory =>
      rm(directory, { recursive: true, force: true })
    )
  );
});

async function packageFixture(
  root: string,
  name: string,
  version: string,
  license = 'MIT'
) {
  const directory = path.join(root, 'packages', `${name}-${version}`);
  await mkdir(directory, { recursive: true });
  await writeFile(
    path.join(directory, 'Cargo.toml'),
    `[package]\nname = "${name}"\nversion = "${version}"\nlicense = "${license}"\n`,
    'utf8'
  );
  await writeFile(
    path.join(directory, 'LICENSE'),
    `${name} ${version} fixture license\n`,
    'utf8'
  );
  return {
    id: `${name} ${version}`,
    name,
    version,
    license,
    license_file: null,
    authors: [`${name} authors`],
    repository: `https://example.invalid/${name}`,
    manifest_path: path.join(directory, 'Cargo.toml'),
    source: 'registry+fixture',
  };
}

describe('Tauri host and renderer distribution notices', () => {
  test('host closure follows only normal runtime edges', () => {
    const packages = [
      { id: 'host', name: 'yesplaymusic-tauri' },
      { id: 'runtime', name: 'tauri' },
      { id: 'build', name: 'tauri-build' },
      { id: 'dev', name: 'test-only' },
      { id: 'unrelated', name: 'sidecar-only' },
    ];
    const metadata = {
      packages,
      resolve: {
        nodes: [
          {
            id: 'host',
            deps: [
              { pkg: 'runtime', dep_kinds: [{ kind: null }] },
              { pkg: 'build', dep_kinds: [{ kind: 'build' as const }] },
              { pkg: 'dev', dep_kinds: [{ kind: 'dev' as const }] },
            ],
          },
          { id: 'runtime', deps: [] },
          { id: 'build', deps: [] },
          { id: 'dev', deps: [] },
          { id: 'unrelated', deps: [] },
        ],
      },
    };

    expect(
      collectCargoRuntimePackages(metadata, 'yesplaymusic-tauri').map(
        candidate => candidate.name
      )
    ).toEqual(['tauri']);
    expect(
      collectCargoRuntimePackages(
        metadata,
        'yesplaymusic-tauri',
        new Set(['yesplaymusic-tauri@undefined', 'tauri@undefined'])
      ).map(candidate => candidate.name)
    ).toEqual(['tauri']);
  });

  test('renderer closure comes from final chunk modules only', () => {
    const root = '/repo';
    expect(
      collectRendererPackageJsonPaths(
        [
          '/repo/src/main.ts',
          '/repo/node_modules/vue/dist/vue.runtime.esm-bundler.js',
          '/repo/node_modules/@tauri-apps/api/core.js?commonjs-proxy',
          '\\0virtual:svg-icons-register',
        ],
        root
      )
    ).toEqual([
      'node_modules/@tauri-apps/api/package.json',
      'node_modules/vue/package.json',
    ]);

    expect(
      collectRendererPackageJsonPaths(
        [
          'C:/repo/node_modules/@scope/pkg/dist/index.js',
          'C:/repo/src/main.ts',
        ],
        'C:/repo'
      )
    ).toEqual(['node_modules/@scope/pkg/package.json']);
    expect(
      collectRendererPackageJsonPaths(
        ['C:\\repo\\node_modules\\plain-pkg\\index.js'],
        'C:\\repo'
      )
    ).toEqual(['node_modules/plain-pkg/package.json']);
  });

  test('builder writes a verified unified host and renderer inventory', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ypm-app-notices-'));
    temporaryDirectories.push(root);
    const host = await packageFixture(root, 'yesplaymusic-tauri', '0.7.1');
    const tauri = await packageFixture(root, 'tauri', '2.11.1');
    const buildOnly = await packageFixture(root, 'tauri-build', '2.5.6');
    const vueRoot = path.join(root, 'node_modules', 'vue');
    await mkdir(vueRoot, { recursive: true });
    await writeFile(
      path.join(vueRoot, 'package.json'),
      JSON.stringify({
        name: 'vue',
        version: '3.5.0',
        license: 'MIT',
        author: 'Vue contributors',
        repository: 'https://github.com/vuejs/core',
      }),
      'utf8'
    );
    await writeFile(path.join(vueRoot, 'LICENSE_MIT'), 'Vue fixture license\n');
    const rendererManifestPath = path.join(
      root,
      'renderer-dependencies.json'
    );
    await writeFile(
      rendererManifestPath,
      JSON.stringify({
        schemaVersion: 1,
        packageJsonPaths: ['node_modules/vue/package.json'],
      }),
      'utf8'
    );
    await writeFile(path.join(root, 'LICENSE'), 'YesPlayMusic MIT\n');
    const outputDirectory = path.join(root, 'generated', 'app-compliance');
    const metadata = {
      packages: [host, tauri, buildOnly],
      resolve: {
        nodes: [
          {
            id: host.id,
            deps: [
              { pkg: tauri.id, dep_kinds: [{ kind: null }] },
              { pkg: buildOnly.id, dep_kinds: [{ kind: 'build' as const }] },
            ],
          },
          { id: tauri.id, deps: [] },
          { id: buildOnly.id, deps: [] },
        ],
      },
    };

    const result = await buildAppCompliance({
      projectRoot: root,
      outputDirectory,
      rendererManifestPath,
      cargoMetadata: metadata,
      targetTriple: 'aarch64-apple-darwin',
    });

    expect(result).toEqual({ hostDependencyCount: 1, rendererPackageCount: 1 });
    const notice = await readFile(
      path.join(outputDirectory, 'THIRD-PARTY-NOTICES.md'),
      'utf8'
    );
    expect(notice).toContain('## Tauri host');
    expect(notice).toContain('| tauri | 2.11.1 |');
    expect(notice).not.toContain('tauri-build');
    expect(notice).toContain('## Renderer');
    expect(notice).toContain('| vue | 3.5.0 |');
    await verifyAppComplianceDirectory(outputDirectory, {
      targetTriple: 'aarch64-apple-darwin',
      requiredHostPackages: ['tauri'],
      requiredRendererPackages: ['vue'],
    });

    const fakeArtifact = path.join(root, 'fake-package');
    const squashfs = Buffer.alloc(64);
    squashfs.write('hsqs');
    squashfs.writeUInt32LE(4096, 12);
    squashfs.writeUInt16LE(12, 22);
    squashfs.writeUInt16LE(4, 28);
    squashfs.writeUInt16LE(0, 30);
    await writeFile(fakeArtifact, new Uint8Array(squashfs));
    for (const target of [
      'windows-x86_64',
      'linux-x86_64-appimage',
      'linux-x86_64-deb',
    ]) {
      const verifiedPackage = await verifyPackagedAppCompliance(
          target,
          fakeArtifact,
          outputDirectory,
          (_command, args) => {
            const outputArgument = args.find(
              argument => argument.startsWith('-o') && argument !== '-o'
            );
            const dashDirectory = args.indexOf('-d');
            const extracted = outputArgument
              ? outputArgument.slice(2)
              : dashDirectory >= 0
                ? args[dashDirectory + 1]
                : args.at(-1);
            if (!extracted) throw new Error('fixture extraction directory missing');
            mkdirSync(extracted, { recursive: true });
            if (process.platform !== 'win32') {
              symlinkSync('missing-runtime-target', path.join(extracted, 'AppRun-link'));
            }
            cpSync(
              outputDirectory,
              path.join(extracted, 'app-compliance'),
              { recursive: true }
            );
          }
        );
      expect(verifiedPackage).toContain('app-compliance');
    }
    await expect(
      verifyPackagedAppCompliance(
        'linux-x86_64-deb',
        fakeArtifact,
        outputDirectory,
        (_command, args) => {
          const extracted = args.at(-1);
          if (!extracted) throw new Error('fixture extraction directory missing');
          mkdirSync(extracted, { recursive: true });
        }
      )
    ).rejects.toThrow('found 0');
    let windowsExtraction = 0;
    await expect(
      verifyPackagedAppCompliance(
        'windows-x86_64',
        fakeArtifact,
        outputDirectory,
        (_command, args) => {
          const outputArgument = args.find(argument => argument.startsWith('-o'));
          if (!outputArgument) throw new Error('fixture extraction directory missing');
          const extracted = outputArgument.slice(2);
          mkdirSync(extracted, { recursive: true });
          if (windowsExtraction++ === 0) {
            writeFileSync(
              path.join(extracted, 'updater-artifact-version.json'),
              '{"version":"fixture"}\n'
            );
            writeFileSync(path.join(extracted, 'payload.7z'), 'nested');
          } else {
            cpSync(
              outputDirectory,
              path.join(extracted, 'app-compliance'),
              { recursive: true }
            );
          }
        }
      )
    ).resolves.toContain('nested-0');

    const nestedTarget = path.join(root, 'nested-target');
    await writeFile(nestedTarget, 'target');
    await symlink(nestedTarget, path.join(outputDirectory, 'nested-link'));
    await expect(verifyAppComplianceDirectory(outputDirectory)).rejects.toThrow(
      'symbolic link'
    );
    await rm(path.join(outputDirectory, 'nested-link'));

    const linkedDirectory = path.join(root, 'linked-app-compliance');
    await symlink(outputDirectory, linkedDirectory);
    await expect(verifyAppComplianceDirectory(linkedDirectory)).rejects.toThrow(
      'real directory'
    );

    await rm(path.join(path.dirname(tauri.manifest_path), 'LICENSE'));
    await expect(
      buildAppCompliance({
        projectRoot: root,
        outputDirectory,
        rendererManifestPath,
        cargoMetadata: metadata,
        targetTriple: 'aarch64-apple-darwin',
      })
    ).rejects.toThrow('no resolvable license text');

    tauri.name = 'alloc-stdlib';
    tauri.version = '0.2.4';
    tauri.license = 'BSD-3-Clause';
    tauri.repository = 'https://github.com/dropbox/rust-alloc-no-stdlib';
    await writeFile(
      path.join(path.dirname(tauri.manifest_path), '.cargo_vcs_info.json'),
      JSON.stringify({
        git: { sha1: 'ae42d22078b98549e987d2f03d12df7b984fde47' },
      })
    );
    const curatedPath = path.join(
      root,
      'legal',
      'alloc-stdlib-0.2.4-LICENSE.txt'
    );
    await mkdir(path.dirname(curatedPath), { recursive: true });
    await writeFile(
      curatedPath,
      await readFile(
        path.resolve(
          import.meta.dir,
          '..',
          'legal',
          'alloc-stdlib-0.2.4-LICENSE.txt'
        ),
        'utf8'
      )
    );
    await expect(
      buildAppCompliance({
        projectRoot: root,
        outputDirectory,
        rendererManifestPath,
        cargoMetadata: metadata,
        targetTriple: 'aarch64-apple-darwin',
      })
    ).resolves.toEqual({ hostDependencyCount: 1, rendererPackageCount: 1 });
    await writeFile(curatedPath, 'tampered\n');
    await expect(
      buildAppCompliance({
        projectRoot: root,
        outputDirectory,
        rendererManifestPath,
        cargoMetadata: metadata,
        targetTriple: 'aarch64-apple-darwin',
      })
    ).rejects.toThrow('Curated license digest mismatch');

    await rm(
      path.join(outputDirectory, 'THIRD-PARTY-NOTICES.md'),
      { force: true }
    );
    await expect(
      verifyAppComplianceDirectory(outputDirectory, {
        targetTriple: 'aarch64-apple-darwin',
      })
    ).rejects.toThrow('THIRD-PARTY-NOTICES.md');
  });

  test('Tauri packages the generated app notices on every platform', async () => {
    const projectRoot = path.resolve(import.meta.dir, '..');
    const tauriConfig = JSON.parse(
      await readFile(
        path.join(projectRoot, 'src-tauri', 'tauri.conf.json'),
        'utf8'
      )
    ) as { bundle: { resources: Record<string, string> } };
    expect(tauriConfig.bundle.resources['generated/app-compliance/']).toBe(
      'app-compliance/'
    );

    const packageJson = JSON.parse(
      await readFile(path.join(projectRoot, 'package.json'), 'utf8')
    ) as { scripts: Record<string, string> };
    expect(packageJson.scripts['build:tauri:renderer']).toContain(
      'build-app-compliance.mjs'
    );
  });
});
