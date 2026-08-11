#!/usr/bin/env bun
import { createHash } from 'node:crypto';
import {
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  realpath,
  rename,
  rm,
  writeFile,
} from 'node:fs/promises';
import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..'
);

export const defaultAppComplianceOutput = path.join(
  projectRoot,
  'src-tauri',
  'generated',
  'app-compliance'
);
export const defaultRendererManifest = path.join(
  projectRoot,
  'src-tauri',
  'generated',
  'renderer-dependencies.json'
);

function sha256(content) {
  return createHash('sha256').update(content).digest('hex');
}

function stableJson(value) {
  return `${JSON.stringify(value, null, 2)}\n`;
}

function toPosix(relativePath) {
  return relativePath.split(path.sep).join('/');
}

function isWithin(parent, child) {
  const relative = path.relative(parent, child);
  return (
    relative !== '' &&
    relative !== '..' &&
    !relative.startsWith(`..${path.sep}`)
  );
}

async function assertSafeOutput(outputDirectory, root) {
  const resolvedRoot = path.resolve(root);
  const resolvedOutput = path.resolve(outputDirectory);
  if (!isWithin(resolvedRoot, resolvedOutput)) {
    throw new Error(
      `App compliance output must remain inside project root: ${resolvedOutput}`
    );
  }
  let current = resolvedOutput;
  while (isWithin(resolvedRoot, current)) {
    try {
      const entry = await lstat(current);
      if (entry.isSymbolicLink()) {
        throw new Error(
          `App compliance output has a symbolic-link ancestor: ${current}`
        );
      }
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error;
    }
    current = path.dirname(current);
  }
}

export function collectCargoRuntimePackages(metadata, rootName, allowedCoordinates) {
  if (!metadata?.resolve?.nodes || !Array.isArray(metadata.packages)) {
    throw new Error('Cargo metadata has no resolved graph');
  }
  const rootPackage = metadata.packages.find(
    candidate => candidate.name === rootName
  );
  if (!rootPackage) throw new Error(`Cargo metadata has no ${rootName} package`);
  if (allowedCoordinates) {
    const rootCoordinate = `${rootPackage.name}@${rootPackage.version}`;
    if (!allowedCoordinates.has(rootCoordinate)) {
      throw new Error(`Cargo tree does not contain host root ${rootCoordinate}`);
    }
    const matched = metadata.packages.filter(candidate =>
      allowedCoordinates.has(`${candidate.name}@${candidate.version}`)
    );
    return matched
      .filter(candidate => candidate.id !== rootPackage.id)
      .sort((left, right) => `${left.name}@${left.version}`.localeCompare(`${right.name}@${right.version}`));
  }
  const nodes = new Map(metadata.resolve.nodes.map(node => [node.id, node]));
  const packages = new Map(
    metadata.packages.map(candidate => [candidate.id, candidate])
  );
  const reachable = new Set([rootPackage.id]);
  const queue = [rootPackage.id];
  while (queue.length > 0) {
    const id = queue.shift();
    const node = nodes.get(id);
    if (!node) throw new Error(`Cargo metadata has no resolve node for ${id}`);
    for (const dependency of node.deps ?? []) {
      const kinds = dependency.dep_kinds ?? [];
      const isNormal =
        kinds.length === 0 || kinds.some(entry => entry.kind == null);
      if (!isNormal || reachable.has(dependency.pkg)) continue;
      reachable.add(dependency.pkg);
      queue.push(dependency.pkg);
    }
  }
  return [...reachable]
    .filter(id => id !== rootPackage.id)
    .map(id => {
      const candidate = packages.get(id);
      if (!candidate) throw new Error(`Cargo metadata has no package for ${id}`);
      return candidate;
    })
    .sort((left, right) =>
      `${left.name}@${left.version}`.localeCompare(
        `${right.name}@${right.version}`
      )
    );
}

function packageJsonPathForModule(moduleId, root) {
  const clean = moduleId
    .replace(/^\0/, '')
    .split('?')[0]
    .replaceAll('\\', '/');
  const normalizedRoot = root.replaceAll('\\', '/').replace(/\/$/, '');
  const marker = '/node_modules/';
  const markerIndex = clean.lastIndexOf(marker);
  if (markerIndex < 0) return null;
  const packageSegments = clean
    .slice(markerIndex + marker.length)
    .split('/');
  if (!packageSegments[0]) return null;
  const packageNameSegments = packageSegments[0].startsWith('@')
    ? packageSegments.slice(0, 2)
    : packageSegments.slice(0, 1);
  if (packageNameSegments.some(segment => !segment)) return null;
  const packageJson = `${clean.slice(
    0,
    markerIndex + marker.length
  )}${packageNameSegments.join('/')}/package.json`;
  const rootPrefix = `${normalizedRoot}/`;
  if (!packageJson.startsWith(rootPrefix)) {
    throw new Error(`Renderer package escaped project root: ${packageJson}`);
  }
  return packageJson.slice(rootPrefix.length);
}

export function collectRendererPackageJsonPaths(
  moduleIds,
  root = projectRoot
) {
  return [
    ...new Set(
      moduleIds
        .map(id => packageJsonPathForModule(id, root))
        .filter(Boolean)
    ),
  ].sort();
}

export function rendererDependencyManifestPlugin({
  projectRoot: root = projectRoot,
  outputPath = defaultRendererManifest,
} = {}) {
  return {
    name: 'yesplaymusic-renderer-dependency-manifest',
    generateBundle(_outputOptions, bundle) {
      const moduleIds = [];
      for (const output of Object.values(bundle)) {
        if (output.type === 'chunk') {
          moduleIds.push(...Object.keys(output.modules));
        }
      }
      const packageJsonPaths = collectRendererPackageJsonPaths(moduleIds, root);
      if (packageJsonPaths.length === 0) {
        throw new Error('Renderer bundle contains no node_modules packages');
      }
      mkdirSync(path.dirname(outputPath), { recursive: true });
      writeFileSync(
        outputPath,
        stableJson({ schemaVersion: 1, packageJsonPaths }),
        'utf8'
      );
    },
  };
}

function repositoryUrl(repository) {
  if (typeof repository === 'string') return repository;
  if (repository && typeof repository.url === 'string') return repository.url;
  return null;
}

function person(value) {
  if (typeof value === 'string') return value;
  if (!value || typeof value !== 'object') return '';
  return [value.name, value.email ? `<${value.email}>` : null]
    .filter(Boolean)
    .join(' ');
}

function isLicenseOrNotice(relativePath) {
  const segments = relativePath.split('/');
  const name = segments.at(-1) ?? '';
  return (
    /^(license|licence|copying|copyright|notice)(?:$|[-_].*|\.(?:txt|md|markdown|html|rst|spdx))$/i.test(name) ||
    segments.some(segment => /^licenses?$/i.test(segment))
  );
}

async function listLicenseFiles(root, directory = root) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    if (
      entry.name === 'node_modules' ||
      entry.name === '.git' ||
      entry.name === 'target'
    ) {
      continue;
    }
    const absolute = path.join(directory, entry.name);
    if (entry.isSymbolicLink()) continue;
    if (entry.isDirectory()) {
      files.push(...(await listLicenseFiles(root, absolute)));
      continue;
    }
    if (!entry.isFile()) continue;
    const relative = toPosix(path.relative(root, absolute));
    if (isLicenseOrNotice(relative)) files.push(relative);
  }
  return files.sort();
}

const curatedLicenseFallbacks = new Map();
function registerCurated(coordinates, entry) {
  for (const coordinate of coordinates) curatedLicenseFallbacks.set(coordinate, entry);
}
registerCurated(['alloc-stdlib@0.2.4'], {
  license: 'BSD-3-Clause', repository: 'https://github.com/dropbox/rust-alloc-no-stdlib',
  revisions: ['ae42d22078b98549e987d2f03d12df7b984fde47'],
  files: [['legal/alloc-stdlib-0.2.4-LICENSE.txt', 'c0c56f26d9c051cac4d200c34c84e7ae9aaa853e01a982a1df08b09931e518ae']],
});
registerCurated(['brotli@8.0.4'], {
  license: 'BSD-3-Clause AND MIT', repository: 'https://github.com/dropbox/rust-brotli',
  revisions: ['9651aa3ebfd23be83ca14d914fba2d7c12b32d2f'],
  files: [
    ['legal/app-license-donors/brotli/LICENSE.BSD-3-Clause', 'c0c56f26d9c051cac4d200c34c84e7ae9aaa853e01a982a1df08b09931e518ae'],
    ['legal/app-license-donors/brotli/LICENSE.MIT', '3d180008e36922a4e8daec11c34c7af264fed5962d07924aea928c38e8663c94'],
  ],
});
registerCurated(['block2@0.6.2'], {
  license: 'MIT', repository: 'https://github.com/madsmtm/objc2', revisions: ['b4167b582b2f75f9a1be75495c41b765344fd03c'],
  files: [['legal/app-license-donors/objc2/LICENSE.md', '7f976f7e9cb2d87df7230606feb932c3f21ac0e664045a775b600046ff850c54']],
});
registerCurated(['dispatch2@0.3.1', 'objc2@0.6.4'], {
  repository: 'https://github.com/madsmtm/objc2', revisions: ['8852b424193ca41602281b3d7540d7c8ed51e49a'],
  files: [['legal/app-license-donors/objc2/LICENSE.md', '7f976f7e9cb2d87df7230606feb932c3f21ac0e664045a775b600046ff850c54']],
});
registerCurated(['objc2-encode@4.1.0', 'objc2-exception-helper@0.1.1'], {
  repository: 'https://github.com/madsmtm/objc2', revisions: ['8d214f5477365ffcbcbb7de058c86ed9a518efb7'],
  files: [['legal/app-license-donors/objc2/LICENSE.md', '7f976f7e9cb2d87df7230606feb932c3f21ac0e664045a775b600046ff850c54']],
});
registerCurated([
  'objc2-app-kit@0.3.2', 'objc2-cloud-kit@0.3.2', 'objc2-core-data@0.3.2',
  'objc2-core-foundation@0.3.2', 'objc2-core-graphics@0.3.2', 'objc2-core-image@0.3.2',
  'objc2-core-text@0.3.2', 'objc2-core-video@0.3.2', 'objc2-foundation@0.3.2',
  'objc2-io-surface@0.3.2', 'objc2-osa-kit@0.3.2', 'objc2-quartz-core@0.3.2',
  'objc2-web-kit@0.3.2',
], {
  repository: 'https://github.com/madsmtm/objc2', revisions: ['7b1abfd750a2cacaea71d6a56ecfb83cb7de560b'],
  files: [['legal/app-license-donors/objc2/LICENSE.md', '7f976f7e9cb2d87df7230606feb932c3f21ac0e664045a775b600046ff850c54']],
});
registerCurated(['crc32c@0.6.8'], {
  license: 'Apache-2.0/MIT', repository: 'https://github.com/zowens/crc32c', revisions: ['254a861f7cc71bfe455e86f0e1d86e3c83c33390'],
  files: [
    ['legal/app-license-donors/spdx/Apache-2.0.txt', '074e6e32c86a4c0ef8b3ed25b721ca23aca83df277cd88106ef7177c354615ff'],
    ['legal/app-license-donors/spdx/MIT.txt', 'b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5'],
  ],
});
registerCurated(['selectors@0.36.1'], {
  license: 'MPL-2.0', repository: 'https://github.com/servo/stylo', revisions: ['635e1a19d02960588a00e189bd4bd5bdb150ec3d'],
  files: [['legal/app-license-donors/spdx/MPL-2.0.txt', '66a3107d5ad6a058aab753eaac2047ccb2ed0e39465dd0fe5844da3e300d5172']],
});
registerCurated(['sigchld@0.2.4'], {
  license: 'MIT', repository: 'https://github.com/oconnor663/sigchld.rs', revisions: ['07b95e2fe38b18b376b0f635f2766bf2e641b80b'],
  noticeSource: 'curated-upstream-omitted-license-text',
  files: [
    ['legal/app-license-donors/sigchld/Cargo.toml.orig', '79bbbf7c7e7c9dfad8d8592223ae4d7fe90f769ab4e65b9d2081b5e5335c8901'],
    ['legal/app-license-donors/spdx/MIT.txt', 'b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5'],
  ],
});
registerCurated(['system-configuration@0.5.1', 'system-configuration-sys@0.5.0'], {
  repository: 'https://github.com/mullvad/system-configuration-rs', revisions: ['56e415eb24b0897bef96a9a38fe7883cfbe2bb94', '592a485494be9d63a60e54944dd2fcc4f077ec58'],
  files: [['legal/app-license-donors/mullvad/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2']],
});
registerCurated(['unic-char-property@0.9.0', 'unic-char-range@0.9.0', 'unic-common@0.9.0', 'unic-ucd-ident@0.9.0', 'unic-ucd-version@0.9.0'], {
  repository: 'https://github.com/open-i18n/rust-unic', revisions: ['5878605364af97a3358368a6eaef02104af2e016', '8a6ce83063d90b91ae2ce59eddb803edd393fca9'],
  files: [
    ['legal/app-license-donors/rust-unic/COPYRIGHT.md', 'f5c342c49f3ac804f3e8e7bb62a8040a44c50d47bb36902b1abd13f66a1adf8b'],
    ['legal/app-license-donors/rust-unic/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'],
  ],
});
registerCurated(['@vue/devtools-api@6.6.4'], {
  license: 'MIT', repository: 'https://github.com/vuejs/vue-devtools', revisions: [],
  files: [['legal/app-license-donors/vue-devtools-api/LICENSE', '050bbca6960784db52ff387271bf2ecc5cbed7cf8581b415d528a6ecb6585015']],
});
registerCurated(['webview2-com@0.38.2', 'webview2-com-sys@0.38.2', 'webview2-com-macros@0.8.1'], {
  license: 'MIT', repository: 'https://github.com/wravery/webview2-rs',
  revisions: ['b74dc5e2b394044bea5191052868ce7a106c202c', 'dffa41a8a46d3f5565eefbff2de57d38d399f158'],
  files: [['legal/app-license-donors/webview2-rs/LICENSE', '0dcf41516e608bbcb6cdc5229feb7b86fe4a643b85e7df251133c93408fdac73']],
});
registerCurated(['dlopen2@0.8.2', 'dlopen2_derive@0.4.3'], {
  license: 'MIT', repository: 'https://github.com/OpenByteDev/dlopen2',
  revisions: ['cc80e4a0a90d499b677fdf7743699b4b3a43a989'],
  files: [['legal/app-license-donors/dlopen2/LICENSE', '39fa265207450e77c62e90c5594a06c085b655d8374c7ced4bf7894b6bd95dd2']],
});
registerCurated(['libappindicator-sys@0.9.0'], {
  license: 'Apache-2.0 OR MIT', repository: '',
  revisions: ['eafd1e3682a1247f595410266091e9684021cb6f'],
  files: [['legal/app-license-donors/libappindicator/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2']],
});

function normalizeRepository(value) {
  return String(value ?? '').replace(/^git\+/, '').replace(/\.git$/, '').replace(/\/$/, '');
}

async function copyPackageLicenses(
  candidate,
  outputDirectory,
  copiedByDigest,
  fallbackIndex
) {
  const packageDirectory = candidate.packageDirectory;
  const candidates = await listLicenseFiles(packageDirectory);
  let sourceFiles = candidates.map(relativePath =>
    path.join(packageDirectory, relativePath)
  );
  let noticeSource = 'package';
  if (sourceFiles.length === 0) {
    const coordinate = `${candidate.name}@${candidate.version}`;
    const fallback = curatedLicenseFallbacks.get(coordinate);
    if (fallback) {
      if (
        (fallback.license && fallback.license !== candidate.license) ||
        normalizeRepository(fallback.repository) !== normalizeRepository(candidate.repository) ||
        (fallback.revisions.length > 0 && !fallback.revisions.includes(candidate.vcsRevision))
      ) {
        throw new Error(`Curated license metadata mismatch: ${coordinate}`);
      }
      sourceFiles = [];
      for (const [relativePath, expectedDigest] of fallback.files) {
        const fallbackPath = path.join(fallbackIndex.projectRoot, relativePath);
        const content = await readFile(fallbackPath);
        if (sha256(content) !== expectedDigest) {
          throw new Error(`Curated license digest mismatch: ${coordinate}`);
        }
        sourceFiles.push(fallbackPath);
      }
      noticeSource = fallback.noticeSource ?? 'curated';
    }
  }
  if (sourceFiles.length === 0) {
    throw new Error(
      `Distributed package has no resolvable license text: ${candidate.name} ${candidate.version} (${candidate.license})`
    );
  }
  const copied = [];
  for (const sourceFile of sourceFiles) {
    const content = await readFile(sourceFile);
    const digest = sha256(content);
    let bundledPath = copiedByDigest.get(digest);
    if (!bundledPath) {
      bundledPath = `license-files/${digest}.txt`;
      await writeFile(path.join(outputDirectory, bundledPath), content);
      copiedByDigest.set(digest, bundledPath);
    }
    copied.push(bundledPath);
  }
  return {
    licenseFiles: [...new Set(copied)].sort(),
    noticeSource,
  };
}

async function cargoRecords(
  packages,
  outputDirectory,
  copiedByDigest,
  fallbackIndex
) {
  const records = [];
  for (const candidate of packages) {
    if (!candidate.license) {
      throw new Error(
        `${candidate.name} ${candidate.version} has no SPDX license metadata`
      );
    }
    const packageDirectory = path.dirname(candidate.manifest_path);
    let vcsRevision = null;
    try {
      const vcsInfo = JSON.parse(
        await readFile(path.join(packageDirectory, '.cargo_vcs_info.json'), 'utf8')
      );
      vcsRevision = vcsInfo.git?.sha1 ?? null;
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error;
    }
    const copied = await copyPackageLicenses(
      {
        ...candidate,
        packageDirectory,
        authors: candidate.authors ?? [],
        vcsRevision,
      },
      outputDirectory,
      copiedByDigest,
      fallbackIndex
    );
    records.push({
      name: candidate.name,
      version: candidate.version,
      license: candidate.license,
      authors: candidate.authors ?? [],
      repository: candidate.repository ?? null,
      ...copied,
    });
  }
  return records;
}

async function rendererRecords(
  manifest,
  root,
  outputDirectory,
  copiedByDigest,
  fallbackIndex
) {
  if (
    manifest?.schemaVersion !== 1 ||
    !Array.isArray(manifest.packageJsonPaths)
  ) {
    throw new Error('Renderer dependency manifest is invalid');
  }
  const records = [];
  const seen = new Set();
  const resolvedNodeModules = await realpath(path.join(root, 'node_modules'));
  for (const relativePath of manifest.packageJsonPaths) {
    if (
      typeof relativePath !== 'string' ||
      !relativePath.startsWith('node_modules/') ||
      relativePath.includes('..') ||
      path.isAbsolute(relativePath)
    ) {
      throw new Error(`Renderer package path is invalid: ${relativePath}`);
    }
    const packageJsonPath = path.join(root, relativePath);
    const packageDirectory = path.dirname(packageJsonPath);
    const resolvedPackage = await realpath(packageDirectory);
    if (!isWithin(resolvedNodeModules, resolvedPackage)) {
      throw new Error(
        `Renderer package escaped node_modules: ${relativePath}`
      );
    }
    const packageJson = JSON.parse(await readFile(packageJsonPath, 'utf8'));
    if (!packageJson.name || !packageJson.version) {
      throw new Error(`Renderer package metadata incomplete: ${relativePath}`);
    }
    const declaredLicense = packageJson.license ?? 'SEE LICENSE FILES';
    const coordinate = `${packageJson.name}@${packageJson.version}`;
    if (seen.has(coordinate)) continue;
    seen.add(coordinate);
    const contributors = Array.isArray(packageJson.contributors)
      ? packageJson.contributors.map(person).filter(Boolean)
      : [];
    const author = person(packageJson.author);
    const authors = [author, ...contributors].filter(Boolean);
    const copied = await copyPackageLicenses(
      {
        name: packageJson.name,
        version: packageJson.version,
        license: declaredLicense,
        authors,
        repository: repositoryUrl(packageJson.repository),
        packageDirectory,
      },
      outputDirectory,
      copiedByDigest,
      fallbackIndex
    );
    records.push({
      name: packageJson.name,
      version: packageJson.version,
      license: declaredLicense,
      authors,
      repository: repositoryUrl(packageJson.repository),
      ...copied,
    });
  }
  return records.sort((left, right) =>
    `${left.name}@${left.version}`.localeCompare(
      `${right.name}@${right.version}`
    )
  );
}

function markdownCell(value) {
  return String(value ?? '')
    .replaceAll('|', '\\|')
    .replaceAll('\n', ' ');
}

function noticeRows(records) {
  return records
    .map(record => {
      const upstream = record.repository
        ? `[source](${record.repository})`
        : 'package metadata';
      const files = record.licenseFiles
        .map(file => `\`${file}\``)
        .join('<br>');
      return `| ${markdownCell(record.name)} | ${markdownCell(
        record.version
      )} | ${markdownCell(record.license)} | ${markdownCell(
        record.authors.join('; ')
      )} | ${upstream} | ${markdownCell(record.noticeSource)} | ${files} |`;
    })
    .join('\n');
}

function thirdPartyNotice({ targetTriple, hostPackages, rendererPackages }) {
  const header =
    '| Package | Version | SPDX license | Authors | Upstream | Notice source | Included license/notice files |\n| --- | --- | --- | --- | --- | --- | --- |';
  return `# YesPlayMusic application third-party notices

This inventory covers the exact third-party code distributed in the Tauri
host normal dependency closure for \`${targetTriple}\` and the packages whose
modules are present in the final renderer chunks. Cargo dev/build-only edges
and JavaScript packages absent from the final chunks are intentionally omitted.

The separately executed Rust Sidecar has its own GPL/LGPL inventory and
complete-source offer in \`../sidecar-compliance/\`.

## Tauri host

${header}
${noticeRows(hostPackages)}

## Renderer

${header}
${noticeRows(rendererPackages)}
`;
}

async function runCargoMetadata(root, targetTriple) {
  const result = Bun.spawnSync(
    [
      'cargo',
      'metadata',
      '--format-version',
      '1',
      '--locked',
      '--filter-platform',
      targetTriple,
      '--manifest-path',
      path.join(root, 'src-tauri', 'Cargo.toml'),
    ],
    { cwd: root, stdout: 'pipe', stderr: 'pipe' }
  );
  if (result.exitCode !== 0) {
    throw new Error(
      `cargo metadata failed: ${result.stderr.toString().trim()}`
    );
  }
  return JSON.parse(result.stdout.toString());
}

async function runCargoTreeCoordinates(root, targetTriple) {
  const result = Bun.spawnSync(
    [
      'cargo', 'tree', '--locked', '--target', targetTriple,
      '--package', 'yesplaymusic-tauri', '--edges', 'normal',
      '--prefix', 'none', '--format', '{p}', '--manifest-path',
      path.join(root, 'src-tauri', 'Cargo.toml'),
    ],
    { cwd: root, stdout: 'pipe', stderr: 'pipe' }
  );
  if (result.exitCode !== 0) {
    throw new Error(`cargo tree failed: ${result.stderr.toString().trim()}`);
  }
  const coordinates = new Set();
  for (const line of result.stdout.toString().split('\n')) {
    const match = line.replace(/ \(\*\)$/, '').match(/^(\S+) v(\S+)/);
    if (match) coordinates.add(`${match[1]}@${match[2]}`);
  }
  if (coordinates.size === 0) throw new Error('Cargo tree closure is empty');
  return coordinates;
}

function defaultTargetTriple() {
  if (process.env.TAURI_ENV_TARGET_TRIPLE) {
    return process.env.TAURI_ENV_TARGET_TRIPLE;
  }
  if (process.platform === 'darwin' && process.arch === 'arm64') {
    return 'aarch64-apple-darwin';
  }
  if (process.platform === 'win32' && process.arch === 'x64') {
    return 'x86_64-pc-windows-msvc';
  }
  if (process.platform === 'linux' && process.arch === 'x64') {
    return 'x86_64-unknown-linux-gnu';
  }
  throw new Error('TAURI_ENV_TARGET_TRIPLE is required for this platform');
}

async function listFiles(root, directory = root) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const absolute = path.join(directory, entry.name);
    if (entry.isSymbolicLink()) {
      throw new Error(
        `App compliance contains symbolic link: ${toPosix(path.relative(root, absolute))}`
      );
    }
    if (entry.isDirectory()) {
      files.push(...(await listFiles(root, absolute)));
    } else if (entry.isFile()) {
      files.push(toPosix(path.relative(root, absolute)));
    }
  }
  return files.sort();
}

async function writeChecksums(directory) {
  const files = (await listFiles(directory)).filter(
    file => file !== 'SHA256SUMS'
  );
  const lines = [];
  for (const relativePath of files) {
    lines.push(
      `${sha256(
        await readFile(path.join(directory, relativePath))
      )}  ${relativePath}`
    );
  }
  await writeFile(
    path.join(directory, 'SHA256SUMS'),
    `${lines.join('\n')}\n`,
    'utf8'
  );
}

export async function buildAppCompliance({
  projectRoot: root = projectRoot,
  outputDirectory = defaultAppComplianceOutput,
  rendererManifestPath = defaultRendererManifest,
  cargoMetadata: suppliedMetadata,
  cargoTreeCoordinates: suppliedCargoTreeCoordinates,
  targetTriple = defaultTargetTriple(),
} = {}) {
  await assertSafeOutput(outputDirectory, root);
  const [metadata, rendererManifest, cargoTreeCoordinates] = await Promise.all([
    suppliedMetadata ?? runCargoMetadata(root, targetTriple),
    readFile(rendererManifestPath, 'utf8').then(JSON.parse),
    suppliedCargoTreeCoordinates ??
      (suppliedMetadata ? undefined : runCargoTreeCoordinates(root, targetTriple)),
  ]);
  const hostDependencies = collectCargoRuntimePackages(
    metadata,
    'yesplaymusic-tauri',
    cargoTreeCoordinates
  );
  if (hostDependencies.length === 0) {
    throw new Error('Tauri host runtime closure is empty');
  }
  const parent = path.dirname(outputDirectory);
  await mkdir(parent, { recursive: true });
  const staging = await mkdtemp(path.join(parent, '.app-compliance-'));
  let rendererPackages;
  try {
    await mkdir(path.join(staging, 'license-files'), { recursive: true });
    const copiedByDigest = new Map();
    const fallbackIndex = { projectRoot: root };
    const hostPackages = await cargoRecords(
      hostDependencies,
      staging,
      copiedByDigest,
      fallbackIndex
    );
    rendererPackages = await rendererRecords(
      rendererManifest,
      root,
      staging,
      copiedByDigest,
      fallbackIndex
    );
    if (rendererPackages.length === 0) {
      throw new Error('Renderer package closure is empty');
    }
    const manifest = {
      schemaVersion: 1,
      targetTriple,
      hostPackages,
      rendererPackages,
    };
    await Promise.all([
      writeFile(
        path.join(staging, 'THIRD-PARTY-NOTICES.md'),
        thirdPartyNotice({ targetTriple, hostPackages, rendererPackages }),
        'utf8'
      ),
      writeFile(
        path.join(staging, 'APP-COMPLIANCE-MANIFEST.json'),
        stableJson(manifest),
        'utf8'
      ),
      readFile(path.join(root, 'LICENSE')).then(content =>
        writeFile(path.join(staging, 'YESPLAYMUSIC-MIT.txt'), content)
      ),
    ]);
    await writeChecksums(staging);
    await verifyAppComplianceDirectory(staging, { targetTriple });
    await assertSafeOutput(outputDirectory, root);
    await rm(outputDirectory, { recursive: true, force: true });
    await rename(staging, outputDirectory);
  } catch (error) {
    await rm(staging, { recursive: true, force: true });
    throw error;
  }
  return {
    hostDependencyCount: hostDependencies.length,
    rendererPackageCount: rendererPackages.length,
  };
}

export async function verifyAppComplianceDirectory(
  directory,
  { targetTriple, requiredHostPackages = [], requiredRendererPackages = [] } = {}
) {
  const rootStat = await lstat(directory);
  if (!rootStat.isDirectory() || rootStat.isSymbolicLink()) {
    throw new Error('App compliance root must be a real directory');
  }
  const requiredFiles = [
    'APP-COMPLIANCE-MANIFEST.json',
    'THIRD-PARTY-NOTICES.md',
    'YESPLAYMUSIC-MIT.txt',
    'SHA256SUMS',
  ];
  for (const requiredFile of requiredFiles) {
    try {
      await lstat(path.join(directory, requiredFile));
    } catch {
      throw new Error(`App compliance missing ${requiredFile}`);
    }
  }
  const manifest = JSON.parse(
    await readFile(
      path.join(directory, 'APP-COMPLIANCE-MANIFEST.json'),
      'utf8'
    )
  );
  if (
    manifest.schemaVersion !== 1 ||
    !manifest.targetTriple ||
    !Array.isArray(manifest.hostPackages) ||
    manifest.hostPackages.length === 0 ||
    !Array.isArray(manifest.rendererPackages) ||
    manifest.rendererPackages.length === 0 ||
    (targetTriple && manifest.targetTriple !== targetTriple)
  ) {
    throw new Error('App compliance manifest is invalid');
  }
  const hostNames = new Set(
    manifest.hostPackages.map(candidate => candidate.name)
  );
  const rendererNames = new Set(
    manifest.rendererPackages.map(candidate => candidate.name)
  );
  if (requiredHostPackages.some(name => !hostNames.has(name))) {
    throw new Error('App compliance host closure is incomplete');
  }
  if (requiredRendererPackages.some(name => !rendererNames.has(name))) {
    throw new Error('App compliance renderer closure is incomplete');
  }
  const expectedChecksums = new Map();
  const checksumText = await readFile(
    path.join(directory, 'SHA256SUMS'),
    'utf8'
  );
  for (const line of checksumText.trim().split('\n')) {
    const match = line.match(/^([a-f0-9]{64})  (.+)$/);
    if (!match || expectedChecksums.has(match[2])) {
      throw new Error('App compliance SHA256SUMS is invalid');
    }
    expectedChecksums.set(match[2], match[1]);
  }
  const files = (await listFiles(directory)).filter(
    file => file !== 'SHA256SUMS'
  );
  if (
    files.length !== expectedChecksums.size ||
    files.some(file => !expectedChecksums.has(file))
  ) {
    throw new Error('App compliance file set does not match SHA256SUMS');
  }
  for (const relativePath of files) {
    const digest = sha256(
      await readFile(path.join(directory, relativePath))
    );
    if (digest !== expectedChecksums.get(relativePath)) {
      throw new Error(`App compliance checksum mismatch: ${relativePath}`);
    }
  }
  return manifest;
}

export async function assertAppComplianceMatchesDirectory(actual, expected) {
  await Promise.all([
    verifyAppComplianceDirectory(actual),
    verifyAppComplianceDirectory(expected),
  ]);
  const [actualFiles, expectedFiles] = await Promise.all([
    listFiles(actual),
    listFiles(expected),
  ]);
  if (JSON.stringify(actualFiles) !== JSON.stringify(expectedFiles)) {
    throw new Error('Bundled app compliance file set is stale');
  }
  for (const relativePath of actualFiles) {
    const [actualBytes, expectedBytes] = await Promise.all([
      readFile(path.join(actual, relativePath)),
      readFile(path.join(expected, relativePath)),
    ]);
    if (sha256(actualBytes) !== sha256(expectedBytes)) {
      throw new Error(`Bundled app compliance is stale: ${relativePath}`);
    }
  }
}

async function main() {
  const result = await buildAppCompliance();
  console.log(
    `[app-compliance] ${result.hostDependencyCount} host + ${result.rendererPackageCount} renderer packages -> ${defaultAppComplianceOutput}`
  );
}

if (import.meta.main) {
  main().catch(error => {
    console.error(`[app-compliance] ${error.message}`);
    process.exit(1);
  });
}
