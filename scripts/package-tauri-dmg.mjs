import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import {
  access,
  copyFile,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rename,
  rm,
  stat,
  symlink,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  defaultCompleteSourceOutput,
  sidecarSourceArchiveName,
  sidecarSourceOfferName,
} from './build-sidecar-compliance.mjs';
import {
  RUST_SIDECAR_MARKER,
  assertRustSidecarMarker,
  readArm64MachOUuid,
} from './lib/macBinaryProvenance.mjs';
import {
  assertAppComplianceMatchesDirectory,
  defaultAppComplianceOutput,
  verifyAppComplianceDirectory,
} from './build-app-compliance.mjs';

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..'
);

export const RUST_SIDECAR_APP_SIZE_LIMIT_BYTES = Math.floor(54.1 * 1024 * 1024);

export const defaultTauriAppPath = path.join(
  projectRoot,
  'src-tauri/target/aarch64-apple-darwin/release/bundle/macos/YesPlayMusic.app'
);

export const defaultBuiltSidecarPath = path.join(
  projectRoot,
  'src-tauri/binaries/yesplaymusic-sidecar-aarch64-apple-darwin'
);

export function tauriDmgName(version) {
  return `YesPlayMusic_${version}_aarch64.dmg`;
}

export function tauriBundledDmgPath(version) {
  return path.join(
    projectRoot,
    'src-tauri/target/aarch64-apple-darwin/release/bundle/dmg',
    tauriDmgName(version)
  );
}

function run(command, args, { cwd, env } = {}) {
  const result = Bun.spawnSync([command, ...args], {
    cwd,
    env,
    stdout: 'inherit',
    stderr: 'inherit',
  });
  if (result.exitCode !== 0) {
    throw new Error(`${command} 执行失败（退出码 ${result.exitCode}）`);
  }
}

async function listBundleEntries(root, directory = root) {
  const entries = await readdir(directory, { withFileTypes: true });
  const paths = [];
  for (const entry of entries) {
    const absolute = path.join(directory, entry.name);
    const relative = path.relative(root, absolute).split(path.sep).join('/');
    paths.push(relative);
    if (entry.isDirectory()) {
      paths.push(...(await listBundleEntries(root, absolute)));
    }
  }
  return paths;
}

function legacyRuntimeEntry(relativePath) {
  const segments = relativePath.split('/');
  const name = segments.at(-1)?.toLowerCase() ?? '';
  return (
    name === 'bun' ||
    name === 'bun.exe' ||
    name === 'node' ||
    name === 'node.exe' ||
    name === 'node_modules' ||
    name === '.bun' ||
    name === 'app.asar' ||
    name === 'electron framework.framework' ||
    name.endsWith('.payload')
  );
}

export async function assertMacBundleProvenance(
  appPath,
  expectedVersion,
  builtSidecarPath = defaultBuiltSidecarPath,
  expectedAppComplianceDirectory = defaultAppComplianceOutput
) {
  const contents = path.join(appPath, 'Contents');
  const sidecarPath = path.join(contents, 'MacOS', 'yesplaymusic-sidecar');
  const complianceDirectory = path.join(
    contents,
    'Resources',
    'sidecar-compliance'
  );
  const manifestPath = path.join(complianceDirectory, 'SOURCE-MANIFEST.json');
  const sourceOfferPath = path.join(complianceDirectory, 'SOURCE-OFFER.md');
  const appComplianceDirectory = path.join(
    contents,
    'Resources',
    'app-compliance'
  );
  const [
    sidecarStat,
    sidecarBytes,
    builtSidecarStat,
    builtSidecarBytes,
    manifestText,
    sourceOffer,
    entries,
  ] = await Promise.all([
    lstat(sidecarPath),
    readFile(sidecarPath),
    lstat(builtSidecarPath),
    readFile(builtSidecarPath),
    readFile(manifestPath, 'utf8'),
    readFile(sourceOfferPath, 'utf8'),
    listBundleEntries(contents),
  ]);
  if (!sidecarStat.isFile() || !builtSidecarStat.isFile()) {
    throw new Error('macOS bundle 或当前构建的 Rust Sidecar 不是普通文件');
  }

  const manifest = JSON.parse(manifestText);
  const provenance = manifest.binaryProvenance;
  const completeSource = manifest.completeSource;
  if (
    manifest.schemaVersion !== 2 ||
    manifest.sidecar?.name !== 'yesplaymusic-sidecar' ||
    manifest.sidecar?.version !== expectedVersion ||
    provenance?.targetTriple !== 'aarch64-apple-darwin' ||
    provenance?.fileName !== path.basename(builtSidecarPath) ||
    !/^[a-f0-9]{64}$/.test(provenance?.sha256 ?? '') ||
    provenance?.rustMarker !== RUST_SIDECAR_MARKER ||
    typeof provenance.machOUuid !== 'string' ||
    typeof completeSource?.assetName !== 'string' ||
    completeSource?.offlineRebuildVerified !== true ||
    !Number.isSafeInteger(completeSource?.dependencySourceCount) ||
    completeSource.dependencySourceCount < 1
  ) {
    throw new Error('macOS bundle 的 Rust Sidecar provenance manifest 无效');
  }

  assertRustSidecarMarker(builtSidecarBytes);
  assertRustSidecarMarker(sidecarBytes);
  const sidecarUuid = readArm64MachOUuid(sidecarBytes);
  const builtSidecarUuid = readArm64MachOUuid(builtSidecarBytes);
  const builtSidecarSha256 = createHash('sha256')
    .update(builtSidecarBytes)
    .digest('hex');
  if (
    sidecarUuid !== provenance.machOUuid ||
    builtSidecarUuid !== provenance.machOUuid ||
    builtSidecarSha256 !== provenance.sha256
  ) {
    throw new Error(
      'macOS bundle、当前 Rust 构建产物与 provenance manifest 不一致'
    );
  }
  const expectedSourceAsset = sidecarSourceArchiveName(expectedVersion);
  const expectedSourceUrl = `https://github.com/nagi-studio/YesPlayMusic/releases/download/v${expectedVersion}/${expectedSourceAsset}`;
  if (
    completeSource.assetName !== expectedSourceAsset ||
    !sourceOffer.includes(expectedSourceAsset) ||
    !sourceOffer.includes(expectedSourceUrl)
  ) {
    throw new Error('macOS bundle 缺少当前版本的完整源码下载指引');
  }

  const forbidden = entries.find(
    entry =>
      legacyRuntimeEntry(entry) ||
      entry.startsWith('Resources/sidecar-compliance/source/vendor/')
  );
  if (forbidden) {
    throw new Error(
      `macOS bundle 含有禁止分发的旧运行时或内嵌 vendor：${forbidden}`
    );
  }

  await verifyAppComplianceDirectory(appComplianceDirectory, {
    targetTriple: 'aarch64-apple-darwin',
    requiredHostPackages: ['tauri', 'rusqlite'],
    requiredRendererPackages: ['vue', 'pinia'],
  });
  await assertAppComplianceMatchesDirectory(
    appComplianceDirectory,
    expectedAppComplianceDirectory
  );

  return {
    sidecarUuid,
    builtSidecarSha256,
    sourceArchiveName: expectedSourceAsset,
    dependencyCount: completeSource.dependencySourceCount,
  };
}

async function verifyCompleteSourceKit(
  sourceDirectory,
  expectedVersion,
  bundledProvenance
) {
  const manifest = JSON.parse(
    await readFile(path.join(sourceDirectory, 'SOURCE-MANIFEST.json'), 'utf8')
  );
  const sourcePackages = manifest.dependencySourcePackages;
  const sourceCount = manifest.completeSource?.dependencySourceCount;
  if (
    manifest.schemaVersion !== 2 ||
    manifest.sidecar?.version !== expectedVersion ||
    manifest.completeSource?.assetName !==
      sidecarSourceArchiveName(expectedVersion) ||
    manifest.completeSource?.offlineRebuildVerified !== true ||
    manifest.binaryProvenance?.machOUuid !== bundledProvenance.sidecarUuid ||
    manifest.binaryProvenance?.sha256 !==
      bundledProvenance.builtSidecarSha256 ||
    manifest.binaryProvenance?.rustMarker !== RUST_SIDECAR_MARKER ||
    !Array.isArray(sourcePackages) ||
    sourcePackages.length !== sourceCount ||
    manifest.dependencyNoticeCount !== sourceCount ||
    sourceCount !== bundledProvenance.dependencyCount
  ) {
    throw new Error('Sidecar complete-source kit 未通过完整闭包门禁');
  }

  const vendorDirectory = path.join(sourceDirectory, 'source', 'vendor');
  const vendorEntries = (
    await readdir(vendorDirectory, {
      withFileTypes: true,
    })
  ).filter(entry => entry.isDirectory());
  if (vendorEntries.length !== sourceCount) {
    throw new Error(
      `Sidecar complete-source kit 只有 ${vendorEntries.length}/${sourceCount} 个 vendor 目录`
    );
  }
  await Promise.all([
    access(path.join(sourceDirectory, '.cargo', 'config.toml')),
    access(path.join(sourceDirectory, 'rebuild.sh')),
    access(path.join(sourceDirectory, 'rebuild.ps1')),
  ]);
  const sourceOffer = await readFile(
    path.join(sourceDirectory, 'SOURCE-OFFER.md'),
    'utf8'
  );
  const sourceArchiveName = sidecarSourceArchiveName(expectedVersion);
  const sourceUrl = `https://github.com/nagi-studio/YesPlayMusic/releases/download/v${expectedVersion}/${sourceArchiveName}`;
  if (
    !sourceOffer.includes(sourceArchiveName) ||
    !sourceOffer.includes(sourceUrl)
  ) {
    throw new Error('Sidecar complete-source kit 缺少同版本下载指引');
  }
  run('./verify-sources.sh', [], { cwd: sourceDirectory });
  return { sourceCount };
}

export function assertTauriAppSize(
  allocatedBytes,
  limitBytes = RUST_SIDECAR_APP_SIZE_LIMIT_BYTES
) {
  if (!Number.isSafeInteger(allocatedBytes) || allocatedBytes < 0) {
    throw new Error('Tauri .app allocated size must be a non-negative integer');
  }
  if (allocatedBytes > limitBytes) {
    throw new Error(
      `Rust-only Tauri .app 超过 54.1 MiB 门禁：${(
        allocatedBytes /
        1024 /
        1024
      ).toFixed(2)} MiB`
    );
  }
  return allocatedBytes;
}

function tauriAppAllocatedBytes(appPath) {
  const result = Bun.spawnSync(['du', '-sk', appPath]);
  if (result.exitCode !== 0) {
    throw new Error(new TextDecoder().decode(result.stderr).trim());
  }
  const kibibytes = Number(
    new TextDecoder().decode(result.stdout).trim().split(/\s+/, 1)[0]
  );
  if (!Number.isSafeInteger(kibibytes) || kibibytes < 0) {
    throw new Error('无法解析 Tauri .app 的 du 大小');
  }
  return assertTauriAppSize(kibibytes * 1024);
}

async function sha256(file) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest('hex');
}

async function writeChecksum(file) {
  const checksumPath = `${file}.sha256`;
  const checksum = await sha256(file);
  await writeFile(
    checksumPath,
    `${checksum}  ${path.basename(file)}\n`,
    'utf8'
  );
  return checksumPath;
}

async function packageCompleteSource({
  sourceDirectory,
  outputDir,
  version,
  sourceCount,
}) {
  const archivePath = path.join(outputDir, sidecarSourceArchiveName(version));
  const temporaryArchive = `${archivePath}.tmp-${process.pid}`;
  await mkdir(outputDir, { recursive: true });
  await rm(temporaryArchive, { force: true });
  try {
    run('tar', ['-czf', temporaryArchive, '-C', sourceDirectory, '.'], {
      env: { ...process.env, COPYFILE_DISABLE: '1' },
    });
    await rm(archivePath, { force: true });
    await rename(temporaryArchive, archivePath);
  } catch (error) {
    await rm(temporaryArchive, { force: true });
    throw error;
  }
  const checksumPath = await writeChecksum(archivePath);
  const sourceOfferPath = path.join(outputDir, sidecarSourceOfferName(version));
  await copyFile(
    path.join(sourceDirectory, 'SOURCE-OFFER.md'),
    sourceOfferPath
  );
  const sourceOfferChecksumPath = await writeChecksum(sourceOfferPath);
  return {
    archivePath,
    checksumPath,
    archiveBytes: (await stat(archivePath)).size,
    sourceOfferPath,
    sourceOfferChecksumPath,
    sourceCount,
  };
}

export async function packageTauriDmg({
  appPath = defaultTauriAppPath,
  outputDir = path.join(projectRoot, 'dist_tauri'),
  completeSourceDirectory = defaultCompleteSourceOutput,
} = {}) {
  await access(appPath);
  const pkg = await Bun.file(path.join(projectRoot, 'package.json')).json();
  const bundledProvenance = await assertMacBundleProvenance(
    appPath,
    pkg.version
  );
  const { sourceCount } = await verifyCompleteSourceKit(
    completeSourceDirectory,
    pkg.version,
    bundledProvenance
  );
  const dmgPath = path.join(outputDir, tauriDmgName(pkg.version));
  const checksumPath = `${dmgPath}.sha256`;
  const stagingDir = await mkdtemp(path.join(tmpdir(), 'yesplaymusic-dmg-'));
  const appAllocatedBytes = tauriAppAllocatedBytes(appPath);

  try {
    await mkdir(outputDir, { recursive: true });
    await rm(dmgPath, { force: true });
    await rm(checksumPath, { force: true });
    run('codesign', ['--verify', '--deep', '--strict', '--verbose=2', appPath]);
    run('ditto', [appPath, path.join(stagingDir, 'YesPlayMusic.app')]);
    await symlink('/Applications', path.join(stagingDir, 'Applications'));
    run('hdiutil', [
      'create',
      '-volname',
      'YesPlayMusic',
      '-srcfolder',
      stagingDir,
      '-ov',
      '-format',
      'UDZO',
      dmgPath,
    ]);
    run('hdiutil', ['verify', dmgPath]);

    await writeChecksum(dmgPath);
    const source = await packageCompleteSource({
      sourceDirectory: completeSourceDirectory,
      outputDir,
      version: pkg.version,
      sourceCount,
    });
    return {
      dmgPath,
      checksumPath,
      appAllocatedBytes,
      sourceArchivePath: source.archivePath,
      sourceChecksumPath: source.checksumPath,
      sourceArchiveBytes: source.archiveBytes,
      sourceDependencyCount: source.sourceCount,
      sourceOfferPath: source.sourceOfferPath,
      sourceOfferChecksumPath: source.sourceOfferChecksumPath,
    };
  } finally {
    await rm(stagingDir, { recursive: true, force: true });
  }
}

export async function collectTauriReleaseDmg({
  sourcePath,
  appPath = defaultTauriAppPath,
  outputDir = path.join(projectRoot, 'dist_tauri'),
  completeSourceDirectory = defaultCompleteSourceOutput,
} = {}) {
  const pkg = await Bun.file(path.join(projectRoot, 'package.json')).json();
  const resolvedSource = sourcePath || tauriBundledDmgPath(pkg.version);
  const dmgPath = path.join(outputDir, tauriDmgName(pkg.version));
  const bundledProvenance = await assertMacBundleProvenance(
    appPath,
    pkg.version
  );
  const { sourceCount } = await verifyCompleteSourceKit(
    completeSourceDirectory,
    pkg.version,
    bundledProvenance
  );
  const appAllocatedBytes = tauriAppAllocatedBytes(appPath);

  await access(resolvedSource);
  await access(appPath);
  await mkdir(outputDir, { recursive: true });
  await rm(dmgPath, { force: true });
  await rm(`${dmgPath}.sha256`, { force: true });
  run('codesign', ['--verify', '--deep', '--strict', '--verbose=2', appPath]);
  run('hdiutil', ['verify', resolvedSource]);
  await copyFile(resolvedSource, dmgPath);
  const checksumPath = await writeChecksum(dmgPath);
  const source = await packageCompleteSource({
    sourceDirectory: completeSourceDirectory,
    outputDir,
    version: pkg.version,
    sourceCount,
  });
  return {
    dmgPath,
    checksumPath,
    appAllocatedBytes,
    sourceArchivePath: source.archivePath,
    sourceChecksumPath: source.checksumPath,
    sourceArchiveBytes: source.archiveBytes,
    sourceDependencyCount: source.sourceCount,
    sourceOfferPath: source.sourceOfferPath,
    sourceOfferChecksumPath: source.sourceOfferChecksumPath,
  };
}

if (import.meta.main) {
  const result = process.argv.includes('--collect-release')
    ? await collectTauriReleaseDmg()
    : await packageTauriDmg();
  console.log(`[tauri-package] DMG: ${result.dmgPath}`);
  console.log(`[tauri-package] SHA-256: ${result.checksumPath}`);
  console.log(
    `[tauri-package] complete source: ${result.sourceArchivePath} (${
      result.sourceDependencyCount
    } dependencies, ${(result.sourceArchiveBytes / 1024 / 1024).toFixed(
      2
    )} MiB)`
  );
  console.log(`[tauri-package] source SHA-256: ${result.sourceChecksumPath}`);
  console.log(`[tauri-package] source directions: ${result.sourceOfferPath}`);
  console.log(
    `[tauri-package] directions SHA-256: ${result.sourceOfferChecksumPath}`
  );
  console.log(
    `[tauri-package] installed .app: ${(
      result.appAllocatedBytes /
      1024 /
      1024
    ).toFixed(2)} MiB / 54.1 MiB`
  );
}
