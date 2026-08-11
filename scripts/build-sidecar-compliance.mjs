import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  chmod,
  cp,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  readdir,
  rename,
  rm,
  writeFile,
} from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { rustSidecarBuildPlan } from './build-rust-sidecar.mjs';
import {
  RUST_SIDECAR_MARKER,
  assertRustSidecarMarker,
  readArm64MachOUuid,
} from './lib/macBinaryProvenance.mjs';

const execFileAsync = promisify(execFile);
const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));

export const defaultProjectRoot = path.resolve(scriptDirectory, '..');
export const defaultComplianceOutput = path.join(
  defaultProjectRoot,
  'src-tauri',
  'generated',
  'sidecar-compliance'
);
export const defaultCompleteSourceOutput = path.join(
  defaultProjectRoot,
  'src-tauri',
  'generated',
  'sidecar-complete-source'
);

export const EXPECTED_UNM_CRATES = Object.freeze([
  'unm_api_utils',
  'unm_engine',
  'unm_engine_bilibili',
  'unm_engine_joox',
  'unm_engine_kugou',
  'unm_engine_kuwo',
  'unm_engine_pyncm',
  'unm_engine_qq',
  'unm_engine_ytdl',
  'unm_request',
  'unm_selector',
  'unm_types',
]);

const UNM_VERSION = '0.4.0';
const UNM_LICENSE = 'LGPL-3.0-or-later';
const UNM_REPOSITORY = 'https://github.com/UnblockNeteaseMusic/server-rust';
const GPL_DEPENDENCY = Object.freeze({
  name: 'random-string',
  version: '1.1.0',
  license: 'GPL-3.0-only',
});
const SIDECAR_PACKAGE = 'yesplaymusic-sidecar';
const SIDECAR_BINARY_LICENSE = 'GPL-3.0-only';

export function sidecarSourceArchiveName(version) {
  return `YesPlayMusic_${version}_sidecar-source.tar.gz`;
}

export function sidecarSourceOfferName(version) {
  return `YesPlayMusic_${version}_SOURCE-OFFER.md`;
}

function sha256(content) {
  return createHash('sha256').update(content).digest('hex');
}

function stableJson(value) {
  return `${JSON.stringify(value, null, 2)}\n`;
}

async function builtSidecarProvenance(projectRoot) {
  const plan = rustSidecarBuildPlan();
  const artifactPath = path.join(
    projectRoot,
    'src-tauri',
    'binaries',
    plan.outputName
  );
  const bytes = await readFile(artifactPath);
  assertRustSidecarMarker(bytes);
  return {
    targetTriple: plan.targetTriple,
    fileName: plan.outputName,
    sha256: sha256(bytes),
    rustMarker: RUST_SIDECAR_MARKER,
    machOUuid:
      plan.targetTriple === 'aarch64-apple-darwin'
        ? readArm64MachOUuid(bytes)
        : null,
  };
}

async function potentialRealpath(value) {
  let existing = value;
  const missingSegments = [];
  for (;;) {
    try {
      const resolved = await realpath(existing);
      return path.resolve(resolved, ...missingSegments);
    } catch (error) {
      if (
        !(error instanceof Error) ||
        !('code' in error) ||
        error.code !== 'ENOENT'
      ) {
        throw error;
      }
      const parent = path.dirname(existing);
      if (parent === existing) throw error;
      missingSegments.unshift(path.basename(existing));
      existing = parent;
    }
  }
}

async function rejectLinkedOutputAncestors(allowedRoot, outputDirectory) {
  const relative = path.relative(allowedRoot, outputDirectory);
  const candidates = [
    allowedRoot,
    ...relative
      .split(path.sep)
      .filter(Boolean)
      .map((_, index, segments) =>
        path.join(allowedRoot, ...segments.slice(0, index + 1))
      ),
  ];
  for (const candidate of candidates) {
    try {
      if ((await lstat(candidate)).isSymbolicLink()) {
        throw new Error(
          `Refusing to replace compliance output through a symbolic-link or reparse-point ancestor: ${candidate}`
        );
      }
    } catch (error) {
      if (
        error instanceof Error &&
        'code' in error &&
        error.code === 'ENOENT'
      ) {
        break;
      }
      throw error;
    }
  }
}

async function assertSafeOutput(outputDirectory, projectRoot) {
  const absoluteOutput = path.resolve(outputDirectory);
  const generatedRoot = path.resolve(projectRoot, 'src-tauri', 'generated');
  const tempRoot = path.resolve(os.tmpdir());
  const parent = path.dirname(absoluteOutput);
  const allowedRoot = absoluteOutput.startsWith(`${generatedRoot}${path.sep}`)
    ? generatedRoot
    : absoluteOutput.startsWith(`${tempRoot}${path.sep}`)
    ? tempRoot
    : null;

  if (!allowedRoot || absoluteOutput === allowedRoot) {
    throw new Error(
      `Refusing to replace compliance output outside a generated or temporary directory: ${absoluteOutput}`
    );
  }
  if (parent === absoluteOutput) {
    throw new Error(`Invalid compliance output directory: ${absoluteOutput}`);
  }

  await rejectLinkedOutputAncestors(allowedRoot, absoluteOutput);
  const [realAllowedRoot, realOutput] = await Promise.all([
    potentialRealpath(allowedRoot),
    potentialRealpath(absoluteOutput),
  ]);
  const realRelative = path.relative(realAllowedRoot, realOutput);
  if (
    !realRelative ||
    realRelative === '..' ||
    realRelative.startsWith(`..${path.sep}`) ||
    path.isAbsolute(realRelative)
  ) {
    throw new Error(
      `Refusing to replace compliance output outside its real generated or temporary root: ${realOutput}`
    );
  }
}

async function runCargoMetadata(projectRoot) {
  const manifestPath = path.join(
    projectRoot,
    'src-tauri',
    'sidecar',
    'Cargo.toml'
  );
  const { stdout } = await execFileAsync(
    'cargo',
    [
      'metadata',
      '--manifest-path',
      manifestPath,
      '--format-version',
      '1',
      '--locked',
    ],
    { cwd: projectRoot, maxBuffer: 64 * 1024 * 1024 }
  );
  return JSON.parse(stdout);
}

function reachablePackages(metadata) {
  const root = metadata.packages.find(
    candidate => candidate.name === SIDECAR_PACKAGE
  );
  if (!root) {
    throw new Error(`${SIDECAR_PACKAGE} is missing from cargo metadata`);
  }
  if (root.license !== SIDECAR_BINARY_LICENSE) {
    throw new Error(
      `${SIDECAR_PACKAGE} must declare ${SIDECAR_BINARY_LICENSE}; found ${
        root.license ?? 'no license'
      }`
    );
  }

  const nodes = new Map(
    (metadata.resolve?.nodes ?? []).map(node => [node.id, node])
  );
  const packages = new Map(
    metadata.packages.map(candidate => [candidate.id, candidate])
  );
  const pending = [root.id];
  const seen = new Set();

  while (pending.length > 0) {
    const id = pending.pop();
    if (!id || seen.has(id)) continue;
    seen.add(id);
    for (const dependency of nodes.get(id)?.deps ?? []) {
      const dependencyKinds = dependency.dep_kinds ?? [];
      if (
        dependencyKinds.length > 0 &&
        dependencyKinds.every(({ kind }) => kind === 'dev')
      ) {
        continue;
      }
      pending.push(dependency.pkg);
    }
  }

  return [...seen]
    .map(id => packages.get(id))
    .filter(candidate => candidate !== undefined)
    .sort(
      (left, right) =>
        left.name.localeCompare(right.name) ||
        left.version.localeCompare(right.version)
    );
}

function validateCopyleftClosure(packages) {
  const missingLicense = packages.find(candidate => !candidate.license);
  if (missingLicense) {
    throw new Error(
      `${missingLicense.name} ${missingLicense.version} has no SPDX license metadata`
    );
  }

  const unmPackages = packages.filter(candidate =>
    candidate.name.startsWith('unm_')
  );
  const actualNames = unmPackages.map(({ name }) => name).sort();
  const expectedNames = [...EXPECTED_UNM_CRATES].sort();
  if (JSON.stringify(actualNames) !== JSON.stringify(expectedNames)) {
    throw new Error(
      `UNM dependency closure changed: expected ${expectedNames.join(
        ', '
      )}, found ${actualNames.join(', ')}`
    );
  }

  for (const candidate of unmPackages) {
    if (
      candidate.version !== UNM_VERSION ||
      candidate.license !== UNM_LICENSE ||
      candidate.repository !== UNM_REPOSITORY
    ) {
      throw new Error(
        `${candidate.name} metadata changed: ${candidate.version}, ${candidate.license}, ${candidate.repository}`
      );
    }
  }

  const gplDependency = packages.find(
    candidate => candidate.name === GPL_DEPENDENCY.name
  );
  if (
    !gplDependency ||
    gplDependency.version !== GPL_DEPENDENCY.version ||
    gplDependency.license !== GPL_DEPENDENCY.license
  ) {
    throw new Error(
      `${GPL_DEPENDENCY.name} ${GPL_DEPENDENCY.version} (${GPL_DEPENDENCY.license}) must remain explicit in the distribution audit`
    );
  }

  const expectedCopyleft = new Set([
    SIDECAR_PACKAGE,
    GPL_DEPENDENCY.name,
    ...EXPECTED_UNM_CRATES,
  ]);
  const unexpectedCopyleft = packages.filter(candidate => {
    const alternatives = (candidate.license ?? '').split(/\s+OR\s+|\//i);
    const everyChoiceIsCopyleft =
      alternatives.length > 0 &&
      alternatives.every(choice => /(?:A?GPL|LGPL)-/i.test(choice));
    return everyChoiceIsCopyleft && !expectedCopyleft.has(candidate.name);
  });
  if (unexpectedCopyleft.length > 0) {
    throw new Error(
      `New copyleft dependencies require source-distribution review: ${unexpectedCopyleft
        .map(({ name, version, license }) => `${name} ${version} (${license})`)
        .join(', ')}`
    );
  }

  return { gplDependency, unmPackages };
}

function registryChecksum(lockText, packageName, packageVersion) {
  for (const block of lockText.split('[[package]]').slice(1)) {
    const name = block.match(/^\s*name = "([^"]+)"/m)?.[1];
    const version = block.match(/^\s*version = "([^"]+)"/m)?.[1];
    if (name !== packageName || version !== packageVersion) continue;
    return block.match(/^\s*checksum = "([a-f0-9]{64})"/m)?.[1] ?? null;
  }
  return null;
}

function manifestDirectory(candidate) {
  if (!candidate.manifest_path) {
    throw new Error(`${candidate.name} has no manifest_path in cargo metadata`);
  }
  return path.dirname(candidate.manifest_path);
}

async function copyTree(
  source,
  destination,
  { excludeCargoArtifacts = false } = {}
) {
  const sourceRoot = path.resolve(source);
  await cp(source, destination, {
    recursive: true,
    filter: candidate => {
      if (!excludeCargoArtifacts) return true;
      const relative = path.relative(sourceRoot, path.resolve(candidate));
      return (
        relative !== 'target' &&
        !relative.startsWith(`target${path.sep}`) &&
        relative !== '.cargo-ok' &&
        relative !== '.cargo-checksum.json'
      );
    },
  });
}

async function listFiles(root, directory = root) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries.sort((left, right) =>
    left.name.localeCompare(right.name)
  )) {
    const absolute = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await listFiles(root, absolute)));
    } else if (entry.isFile()) {
      files.push(path.relative(root, absolute).split(path.sep).join('/'));
    }
  }
  return files;
}

function dependencyCoordinates(packages) {
  return packages
    .filter(candidate => candidate.name !== SIDECAR_PACKAGE)
    .map(({ name, version }) => `${name}@${version}`)
    .sort();
}

function assertExactDependencyResolution(expectedPackages, actualMetadata) {
  const expected = dependencyCoordinates(expectedPackages);
  const actualPackages = reachablePackages(actualMetadata);
  const expectedRoot = expectedPackages.find(
    candidate => candidate.name === SIDECAR_PACKAGE
  );
  const actualRoot = actualPackages.find(
    candidate => candidate.name === SIDECAR_PACKAGE
  );
  if (!expectedRoot || actualRoot?.version !== expectedRoot.version) {
    throw new Error('Offline relinking resolved a different Sidecar package');
  }
  const actual = dependencyCoordinates(actualPackages);
  const expectedSet = new Set(expected);
  const added = actual.filter(value => !expectedSet.has(value));
  if (added.length > 0) {
    throw new Error(
      `Offline relinking introduced dependencies outside the release graph: ${added
        .slice(0, 12)
        .join(', ')}`
    );
  }
  validateCopyleftClosure(actualPackages);
  return actualPackages;
}

async function writeCargoVendorChecksum(
  packageDirectory,
  registryPackageChecksum
) {
  if (!registryPackageChecksum) {
    throw new Error(
      `Missing Cargo.lock registry checksum for ${path.basename(
        packageDirectory
      )}`
    );
  }
  const files = await listFiles(packageDirectory);
  const checksums = {};
  for (const relativePath of files) {
    checksums[relativePath] = sha256(
      await readFile(path.join(packageDirectory, relativePath))
    );
  }
  await writeFile(
    path.join(packageDirectory, '.cargo-checksum.json'),
    stableJson({ files: checksums, package: registryPackageChecksum }),
    'utf8'
  );
}

async function vendorDependencySources(packages, lockText, vendorDirectory) {
  const dependencies = packages.filter(
    candidate => candidate.name !== SIDECAR_PACKAGE
  );
  const seenDirectories = new Set();
  const manifestPackages = [];
  await mkdir(vendorDirectory, { recursive: true });

  for (const candidate of dependencies) {
    if (!candidate.source?.startsWith('registry+')) {
      throw new Error(
        `Non-registry Sidecar dependency requires explicit source handling: ${candidate.name} ${candidate.version}`
      );
    }
    const directoryName = `${candidate.name}-${candidate.version}`;
    if (seenDirectories.has(directoryName)) {
      throw new Error(
        `Duplicate vendored dependency coordinate: ${directoryName}`
      );
    }
    seenDirectories.add(directoryName);
    const registryPackageChecksum = registryChecksum(
      lockText,
      candidate.name,
      candidate.version
    );
    const destination = path.join(vendorDirectory, directoryName);
    await copyTree(manifestDirectory(candidate), destination, {
      excludeCargoArtifacts: true,
    });
    await writeCargoVendorChecksum(destination, registryPackageChecksum);
    manifestPackages.push({
      name: candidate.name,
      version: candidate.version,
      license: candidate.license,
      repository: candidate.repository ?? null,
      registryChecksum: registryPackageChecksum,
    });
  }
  return manifestPackages;
}

function isLicenseOrNotice(relativePath) {
  const segments = relativePath.split('/');
  const name = segments.at(-1) ?? '';
  return (
    /^(license|licence|copying|copyright|notice)(\.|-|$)/i.test(name) ||
    segments.some(segment => /^licenses?$/i.test(segment))
  );
}

async function copyDependencyNotices(packages, outputDirectory) {
  const noticeRoot = path.join(outputDirectory, 'third-party-license-files');
  await mkdir(noticeRoot, { recursive: true });
  const records = [];
  const copiedByDigest = new Map();

  for (const candidate of packages) {
    if (candidate.name === SIDECAR_PACKAGE) continue;
    const sourceDirectory = manifestDirectory(candidate);
    const sourceFiles = await listFiles(sourceDirectory);
    const candidates = sourceFiles.filter(isLicenseOrNotice);
    const copied = [];

    for (const relativePath of candidates) {
      const content = await readFile(path.join(sourceDirectory, relativePath));
      const digest = sha256(content);
      let bundledPath = copiedByDigest.get(digest);
      if (!bundledPath) {
        bundledPath = `third-party-license-files/${digest}.txt`;
        await writeFile(path.join(outputDirectory, bundledPath), content);
        copiedByDigest.set(digest, bundledPath);
      }
      copied.push(bundledPath);
    }

    records.push({
      name: candidate.name,
      version: candidate.version,
      license: candidate.license ?? 'NOASSERTION',
      authors: candidate.authors ?? [],
      repository: candidate.repository ?? null,
      licenseFiles: [...new Set(copied)].sort(),
    });
  }
  return records;
}

function markdownCell(value) {
  return String(value ?? '')
    .replaceAll('|', '\\|')
    .replaceAll('\n', ' ');
}

function thirdPartyNotice(records) {
  const rows = records.map(record => {
    const source = record.repository
      ? `[source](${record.repository})`
      : 'Cargo registry metadata';
    const files =
      record.licenseFiles.length > 0
        ? record.licenseFiles.map(entry => `\`${entry}\``).join('<br>')
        : 'No license file was present in the published crate; SPDX metadata is recorded here.';
    return `| ${markdownCell(record.name)} | ${markdownCell(
      record.version
    )} | ${markdownCell(record.license)} | ${markdownCell(
      record.authors.join('; ')
    )} | ${source} | ${files} |`;
  });

  return `# YesPlayMusic Rust Sidecar third-party notices

This inventory is generated from the exact non-development dependency closure
used by the Rust Sidecar release graph in \`Cargo.lock\`.
The Rust Sidecar binary contains \`random-string 1.1.0\`, which is
\`GPL-3.0-only\`; the combined Sidecar executable is therefore distributed
under GPL-3.0-only. The Tauri host is a separate process and is not part of
this statically linked executable.

The twelve UnblockNeteaseMusic crates listed below are published as
\`LGPL-3.0-or-later\`. The companion complete-source archive contains the
application source and every registry dependency source in \`source/vendor/\`.
Canonical GPLv3 and LGPLv3 texts are included with both distributions.

| Package | Version | SPDX license | Authors | Upstream | Included notice/license files |
| --- | --- | --- | --- | --- | --- |
${rows.join('\n')}
`;
}

function standaloneWorkspaceManifest(releaseWorkspaceManifest) {
  const releaseProfile = releaseWorkspaceManifest.match(
    /^\[profile\.release\]\r?\n(?:^(?!\[).*?(?:\r?\n|$))*/m
  )?.[0];
  if (!releaseProfile) {
    throw new Error('Release workspace manifest has no [profile.release]');
  }
  return `[workspace]\nmembers = ["sidecar"]\nresolver = "2"\n\n${releaseProfile.trim()}\n`;
}

function sourceOffer({ packageVersion, sourceArchiveName }) {
  return `# Complete Corresponding Source

The Rust Sidecar in YesPlayMusic ${packageVersion} is distributed under
GPL-3.0-only. Its complete machine-readable Corresponding Source is the
companion release asset:

- ${sourceArchiveName}
- https://github.com/nagi-studio/YesPlayMusic/releases/download/v${packageVersion}/${sourceArchiveName}

The source asset is offered at no additional charge from the same GitHub
Release as the object-code downloads. It contains the exact application
source, the full locked non-development Cargo dependency source closure,
license notices, checksums, and offline rebuild/relink scripts. Keep this
direction file with redistributed object code and keep the companion source
asset available through the same distribution channel.

This notice documents the project's GPLv3 section 6(d) distribution path; it
is not legal advice.
`;
}

function rebuildReadme({ packageVersion, rustVersion, dependencyCount }) {
  return `# YesPlayMusic Rust Sidecar source and relinking kit

This directory accompanies YesPlayMusic ${packageVersion}. It contains the
preferred source needed to modify and rebuild/relink the Sidecar executable.

## What is included

- \`source/vendor/\`: all ${dependencyCount} exact registry dependency sources
  reachable from the release Sidecar after excluding development-only edges.
- \`source/application/\`: the exact YesPlayMusic Sidecar source, route
  manifest, original release workspace manifest/lock, and a standalone
  workspace whose resolved dependency coordinates are checked against the
  release graph.
- \`.cargo/config.toml\`: replaces crates.io with \`source/vendor/\` and forces
  Cargo offline, so rebuilding cannot silently fetch missing source.
- \`THIRD-PARTY-NOTICES.md\` and \`third-party-license-files/\`: the complete
  locked dependency inventory and license/notice files shipped by those crates.
- \`GPL-3.0.txt\` and \`LGPL-3.0.txt\`: canonical license terms.

The UNM crates declare LGPL-3.0-or-later. \`unm_engine_kuwo\` links
\`random-string 1.1.0\`, which declares GPL-3.0-only, so the resulting
Sidecar executable is distributed under GPL-3.0-only. YesPlayMusic modified
the integration in 2026; the bundled published crate directories themselves
are copied without source edits.

## Verify the unmodified kit

On macOS/Linux run \`./verify-sources.sh\`. On Windows PowerShell run
\`.\\verify-sources.ps1\`. Run this before editing because intentional changes
will naturally change the recorded checksums.

## Build a modified Sidecar

Install rustup from <https://rustup.rs/>. The included \`rust-toolchain.toml\`
selects Rust ${rustVersion}, derived from the Sidecar manifest's minimum Rust
version. The rebuild scripts pass \`--offline --locked\`; no dependency source
is downloaded during the build.

- macOS/Linux: \`./rebuild.sh\`
- Windows PowerShell: \`.\\rebuild.ps1\`
- Cross target: pass Cargo flags, for example
  \`./rebuild.sh --target x86_64-unknown-linux-gnu\`.

The executable is written below
\`source/application/src-tauri/target/<profile>/\`. Modify any local
crate below \`source/vendor/\`, rerun the script, and Cargo relinks the
Sidecar against that modified source.

## Install or test the replacement

Quit YesPlayMusic first. Keep the executable name and executable permission.
The Sidecar protocol is documented by its bundled source: Tauri passes ports
and the parent PID as arguments and sends a per-launch health token on stdin.

- macOS: replace \`YesPlayMusic.app/Contents/MacOS/yesplaymusic-sidecar\`, then
  ad-hoc sign the modified app with
  \`codesign --force --deep --sign - YesPlayMusic.app\`.
- Windows current-user install: replace \`yesplaymusic-sidecar.exe\` in the
  installed application directory. Windows may warn because the publisher's
  Authenticode signature no longer matches the modified file.
- Linux deb: replace \`/usr/bin/yesplaymusic-sidecar\` using administrator
  privileges.
- Linux AppImage: run \`Your.AppImage --appimage-extract\`, replace
  \`squashfs-root/usr/bin/yesplaymusic-sidecar\`, then run
  \`squashfs-root/AppRun\` or repack with \`appimagetool\`.

No publisher private signing key is required to build or run a locally
modified Sidecar on these general-purpose operating systems. Published update
signatures do not cover user-built replacements; keep automatic updates off
for that local test copy.

This kit is a compliance aid, not legal advice. Redistributors must preserve
the license texts, notices, source, scripts, and any installation information
required for their own distribution channel.
`;
}

const CARGO_VENDOR_CONFIG = `[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "source/vendor"

[net]
offline = true
`;

const VERIFY_SHELL = `#!/bin/sh
set -eu
cd "$(dirname "$0")"
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum -c --quiet SHA256SUMS
elif command -v shasum >/dev/null 2>&1; then
  report="\${TMPDIR:-/tmp}/yesplaymusic-source-verify-$$.log"
  trap 'rm -f "$report"' EXIT HUP INT TERM
  if ! shasum -a 256 -c SHA256SUMS >"$report"; then
    cat "$report" >&2
    exit 1
  fi
else
  echo "sha256sum or shasum is required" >&2
  exit 1
fi
echo "Sidecar complete source checksums verified."
`;

const VERIFY_POWERSHELL = `$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
Get-Content SHA256SUMS | ForEach-Object {
  if ($_ -notmatch '^([a-f0-9]{64})  (.+)$') { throw "Invalid SHA256SUMS line: $_" }
  $expected = $Matches[1]
  $filePath = $Matches[2]
  $stream = [System.IO.File]::OpenRead($filePath)
  try {
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
      $actual = [System.BitConverter]::ToString($sha256.ComputeHash($stream)).Replace('-', '').ToLowerInvariant()
    } finally {
      $sha256.Dispose()
    }
  } finally {
    $stream.Dispose()
  }
  if ($actual -ne $expected) { throw "Checksum mismatch: $filePath" }
}
Write-Host 'Sidecar compliance source checksums verified.'
`;

const REBUILD_SHELL = `#!/bin/sh
set -eu
cd "$(dirname "$0")"
cargo build --manifest-path source/application/src-tauri/sidecar/Cargo.toml --package yesplaymusic-sidecar --offline --locked --release "$@"
`;

const REBUILD_POWERSHELL = `$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
cargo build --manifest-path source/application/src-tauri/sidecar/Cargo.toml --package yesplaymusic-sidecar --offline --locked --release @args
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
`;

async function verifyOfflineRebuild(
  sourceDirectory,
  expectedPackages,
  targetTriple
) {
  const cargoHome = await mkdtemp(
    path.join(os.tmpdir(), 'yesplaymusic-offline-cargo-')
  );
  const targetDirectory = await mkdtemp(
    path.join(os.tmpdir(), 'yesplaymusic-offline-target-')
  );
  const manifestPath = path.join(
    sourceDirectory,
    'source',
    'application',
    'src-tauri',
    'sidecar',
    'Cargo.toml'
  );
  const environment = {
    ...process.env,
    CARGO_HOME: cargoHome,
    CARGO_TARGET_DIR: targetDirectory,
  };

  try {
    await execFileAsync(
      'cargo',
      [
        'generate-lockfile',
        '--manifest-path',
        manifestPath,
        '--offline',
        '--quiet',
      ],
      {
        cwd: sourceDirectory,
        env: environment,
        maxBuffer: 64 * 1024 * 1024,
      }
    );
    const { stdout } = await execFileAsync(
      'cargo',
      [
        'metadata',
        '--manifest-path',
        manifestPath,
        '--format-version',
        '1',
        '--offline',
        '--locked',
      ],
      {
        cwd: sourceDirectory,
        env: environment,
        maxBuffer: 64 * 1024 * 1024,
      }
    );
    const resolvedPackages = assertExactDependencyResolution(
      expectedPackages,
      JSON.parse(stdout)
    );
    await execFileAsync(
      'cargo',
      [
        'build',
        '--manifest-path',
        manifestPath,
        '--package',
        SIDECAR_PACKAGE,
        '--offline',
        '--locked',
        '--release',
        '--target',
        targetTriple,
        '--quiet',
      ],
      {
        cwd: sourceDirectory,
        env: environment,
        maxBuffer: 64 * 1024 * 1024,
      }
    );
    return resolvedPackages;
  } finally {
    await Promise.all([
      rm(cargoHome, { recursive: true, force: true }),
      rm(targetDirectory, { recursive: true, force: true }),
    ]);
  }
}

async function writeChecksums(outputDirectory) {
  const files = (await listFiles(outputDirectory)).filter(
    relativePath => relativePath !== 'SHA256SUMS'
  );
  const lines = [];
  for (const relativePath of files) {
    const content = await readFile(path.join(outputDirectory, relativePath));
    lines.push(`${sha256(content)}  ${relativePath}`);
  }
  await writeFile(
    path.join(outputDirectory, 'SHA256SUMS'),
    `${lines.join('\n')}\n`,
    'utf8'
  );
}

export async function buildSidecarCompliance({
  projectRoot = defaultProjectRoot,
  outputDirectory = defaultComplianceOutput,
  completeSourceDirectory = outputDirectory === defaultComplianceOutput
    ? defaultCompleteSourceOutput
    : `${outputDirectory}-complete-source`,
  metadata,
  binaryProvenance,
  skipOfflineRebuild = false,
  noticesOnly = false,
} = {}) {
  await assertSafeOutput(outputDirectory, projectRoot);
  if (!noticesOnly) {
    await assertSafeOutput(completeSourceDirectory, projectRoot);
    if (
      path.resolve(outputDirectory) === path.resolve(completeSourceDirectory)
    ) {
      throw new Error('Compliance and complete-source outputs must differ');
    }
  }
  const cargoMetadata = metadata ?? (await runCargoMetadata(projectRoot));
  const workspacePackages = reachablePackages(cargoMetadata);
  const { gplDependency, unmPackages } =
    validateCopyleftClosure(workspacePackages);
  const rootPackage = workspacePackages.find(
    candidate => candidate.name === SIDECAR_PACKAGE
  );
  const rustVersion = rootPackage?.rust_version;
  if (!rootPackage || !rustVersion) {
    throw new Error(`${SIDECAR_PACKAGE} must declare rust-version`);
  }
  const dependencyPackages = workspacePackages.filter(
    candidate => candidate.name !== SIDECAR_PACKAGE
  );
  let distributionPackages = workspacePackages;
  const provenance =
    binaryProvenance ?? (await builtSidecarProvenance(projectRoot));
  const sourceArchiveName = sidecarSourceArchiveName(rootPackage.version);

  const complianceParent = path.dirname(outputDirectory);
  const sourceParent = path.dirname(completeSourceDirectory);
  await Promise.all([
    mkdir(complianceParent, { recursive: true }),
    noticesOnly ? Promise.resolve() : mkdir(sourceParent, { recursive: true }),
  ]);
  const complianceStaging = await mkdtemp(
    path.join(complianceParent, '.sidecar-compliance-')
  );
  const sourceStaging = noticesOnly
    ? null
    : await mkdtemp(path.join(sourceParent, '.sidecar-complete-source-'));
  const workingDirectory = sourceStaging ?? complianceStaging;

  try {
    const legalDirectory = path.join(projectRoot, 'legal');
    const rootLockPath = path.join(projectRoot, 'src-tauri', 'Cargo.lock');
    const lockText = await readFile(rootLockPath, 'utf8');

    await Promise.all([
      cp(
        path.join(legalDirectory, 'GPL-3.0.txt'),
        path.join(workingDirectory, 'GPL-3.0.txt')
      ),
      cp(
        path.join(legalDirectory, 'LGPL-3.0.txt'),
        path.join(workingDirectory, 'LGPL-3.0.txt')
      ),
      cp(
        path.join(projectRoot, 'LICENSE'),
        path.join(workingDirectory, 'YESPLAYMUSIC-MIT.txt')
      ),
    ]);

    let sourceManifestPackages = dependencyPackages.map(candidate => ({
      name: candidate.name,
      version: candidate.version,
      license: candidate.license,
      repository: candidate.repository ?? null,
      registryChecksum: registryChecksum(
        lockText,
        candidate.name,
        candidate.version
      ),
    }));

    if (sourceStaging) {
      const sourceDirectory = path.join(sourceStaging, 'source');
      const applicationDirectory = path.join(sourceDirectory, 'application');
      const bundledSidecarDirectory = path.join(
        applicationDirectory,
        'src-tauri',
        'sidecar'
      );
      const vendorDirectory = path.join(sourceDirectory, 'vendor');
      await Promise.all([
        mkdir(bundledSidecarDirectory, { recursive: true }),
        mkdir(path.join(applicationDirectory, 'src'), { recursive: true }),
        mkdir(path.join(sourceStaging, '.cargo'), { recursive: true }),
      ]);

      await copyTree(
        path.join(projectRoot, 'src-tauri', 'sidecar', 'src'),
        path.join(bundledSidecarDirectory, 'src')
      );
      await cp(
        path.join(projectRoot, 'src', 'sidecar-route-manifest.json'),
        path.join(applicationDirectory, 'src', 'sidecar-route-manifest.json')
      );

      const [sidecarManifest, releaseWorkspaceManifest] = await Promise.all([
        readFile(
          path.join(projectRoot, 'src-tauri', 'sidecar', 'Cargo.toml'),
          'utf8'
        ),
        readFile(path.join(projectRoot, 'src-tauri', 'Cargo.toml'), 'utf8'),
      ]);
      await Promise.all([
        writeFile(
          path.join(bundledSidecarDirectory, 'Cargo.toml'),
          sidecarManifest,
          'utf8'
        ),
        writeFile(
          path.join(bundledSidecarDirectory, 'Cargo.toml.release'),
          sidecarManifest,
          'utf8'
        ),
        writeFile(
          path.join(applicationDirectory, 'src-tauri', 'Cargo.toml'),
          standaloneWorkspaceManifest(releaseWorkspaceManifest),
          'utf8'
        ),
        writeFile(
          path.join(applicationDirectory, 'src-tauri', 'Cargo.toml.release'),
          releaseWorkspaceManifest,
          'utf8'
        ),
        cp(
          rootLockPath,
          path.join(applicationDirectory, 'src-tauri', 'Cargo.lock')
        ),
        cp(
          rootLockPath,
          path.join(applicationDirectory, 'src-tauri', 'Cargo.lock.release')
        ),
        writeFile(
          path.join(sourceStaging, '.cargo', 'config.toml'),
          CARGO_VENDOR_CONFIG,
          'utf8'
        ),
      ]);

      sourceManifestPackages = await vendorDependencySources(
        workspacePackages,
        lockText,
        vendorDirectory
      );
      if (sourceManifestPackages.length !== dependencyPackages.length) {
        throw new Error(
          `Complete source closure has ${sourceManifestPackages.length} packages; expected ${dependencyPackages.length}`
        );
      }
      if (!skipOfflineRebuild) {
        distributionPackages = await verifyOfflineRebuild(
          sourceStaging,
          workspacePackages,
          provenance.targetTriple
        );
        const resolvedCoordinates = new Set(
          dependencyCoordinates(distributionPackages)
        );
        await Promise.all(
          dependencyPackages
            .filter(
              ({ name, version }) =>
                !resolvedCoordinates.has(`${name}@${version}`)
            )
            .map(({ name, version }) =>
              rm(path.join(vendorDirectory, `${name}-${version}`), {
                recursive: true,
                force: true,
              })
            )
        );
        sourceManifestPackages = sourceManifestPackages.filter(
          ({ name, version }) => resolvedCoordinates.has(`${name}@${version}`)
        );
      }
    }

    const distributionDependencies = distributionPackages.filter(
      candidate => candidate.name !== SIDECAR_PACKAGE
    );
    const notices = await copyDependencyNotices(
      distributionPackages,
      workingDirectory
    );
    if (
      notices.length !== distributionDependencies.length ||
      sourceManifestPackages.length !== distributionDependencies.length
    ) {
      throw new Error(
        `Dependency notice/source count mismatch: ${notices.length} notices and ${sourceManifestPackages.length} sources for ${distributionDependencies.length} dependencies`
      );
    }

    const copyleftNames = new Set([
      gplDependency.name,
      ...unmPackages.map(({ name }) => name),
    ]);
    const manifest = {
      schemaVersion: 2,
      sidecar: {
        name: rootPackage.name,
        version: rootPackage.version,
        license: rootPackage.license,
        rustVersion,
      },
      binaryProvenance: provenance,
      completeSource: {
        delivery: 'GPL-3.0-only section 6(d)',
        assetName: sourceArchiveName,
        dependencySourceCount: sourceManifestPackages.length,
        offlineRebuildVerified: Boolean(sourceStaging && !skipOfflineRebuild),
      },
      copyleftSourcePackages: sourceManifestPackages.filter(({ name }) =>
        copyleftNames.has(name)
      ),
      dependencySourcePackages: sourceManifestPackages,
      dependencyNoticeCount: notices.length,
    };

    await Promise.all([
      writeFile(
        path.join(workingDirectory, 'THIRD-PARTY-NOTICES.md'),
        thirdPartyNotice(notices),
        'utf8'
      ),
      writeFile(
        path.join(workingDirectory, 'SOURCE-OFFER.md'),
        sourceOffer({
          packageVersion: rootPackage.version,
          sourceArchiveName,
        }),
        'utf8'
      ),
      writeFile(
        path.join(workingDirectory, 'SOURCE-MANIFEST.json'),
        stableJson(manifest),
        'utf8'
      ),
      writeFile(
        path.join(workingDirectory, 'verify-sources.sh'),
        VERIFY_SHELL,
        'utf8'
      ),
      writeFile(
        path.join(workingDirectory, 'verify-sources.ps1'),
        VERIFY_POWERSHELL,
        'utf8'
      ),
    ]);
    await chmod(path.join(workingDirectory, 'verify-sources.sh'), 0o755);

    if (sourceStaging) {
      await Promise.all([
        writeFile(
          path.join(sourceStaging, 'README-RELINKING.md'),
          rebuildReadme({
            packageVersion: rootPackage.version,
            rustVersion,
            dependencyCount: distributionDependencies.length,
          }),
          'utf8'
        ),
        writeFile(
          path.join(sourceStaging, 'rust-toolchain.toml'),
          `[toolchain]\nchannel = "${
            /^\d+\.\d+$/.test(rustVersion) ? `${rustVersion}.0` : rustVersion
          }"\nprofile = "minimal"\n`,
          'utf8'
        ),
        writeFile(
          path.join(sourceStaging, 'rebuild.sh'),
          REBUILD_SHELL,
          'utf8'
        ),
        writeFile(
          path.join(sourceStaging, 'rebuild.ps1'),
          REBUILD_POWERSHELL,
          'utf8'
        ),
      ]);
      await chmod(path.join(sourceStaging, 'rebuild.sh'), 0o755);
      await writeChecksums(sourceStaging);

      for (const relativePath of [
        'GPL-3.0.txt',
        'LGPL-3.0.txt',
        'YESPLAYMUSIC-MIT.txt',
        'THIRD-PARTY-NOTICES.md',
        'SOURCE-OFFER.md',
        'SOURCE-MANIFEST.json',
        'verify-sources.sh',
        'verify-sources.ps1',
      ]) {
        await cp(
          path.join(sourceStaging, relativePath),
          path.join(complianceStaging, relativePath)
        );
      }
      await cp(
        path.join(sourceStaging, 'third-party-license-files'),
        path.join(complianceStaging, 'third-party-license-files'),
        { recursive: true }
      );
      await chmod(path.join(complianceStaging, 'verify-sources.sh'), 0o755);
    }
    await writeChecksums(complianceStaging);

    await assertSafeOutput(outputDirectory, projectRoot);
    if (sourceStaging) {
      await assertSafeOutput(completeSourceDirectory, projectRoot);
      await rm(completeSourceDirectory, { recursive: true, force: true });
      await rename(sourceStaging, completeSourceDirectory);
    }
    await rm(outputDirectory, { recursive: true, force: true });
    await rename(complianceStaging, outputDirectory);
  } catch (error) {
    await Promise.all([
      rm(complianceStaging, { recursive: true, force: true }),
      sourceStaging
        ? rm(sourceStaging, { recursive: true, force: true })
        : Promise.resolve(),
    ]);
    throw error;
  }

  return {
    outputDirectory,
    completeSourceDirectory: noticesOnly ? null : completeSourceDirectory,
    dependencyCount: distributionPackages.length - 1,
    copyleftSourceCount: EXPECTED_UNM_CRATES.length + 1,
  };
}

async function main() {
  const arguments_ = process.argv.slice(2);
  const noticesOnly = arguments_.includes('--notices-only');
  const unknownArguments = arguments_.filter(
    argument => argument !== '--notices-only'
  );
  if (unknownArguments.length > 0) {
    throw new Error(
      `Usage: bun scripts/build-sidecar-compliance.mjs [--notices-only] (unexpected: ${unknownArguments.join(
        ' '
      )})`
    );
  }
  const result = await buildSidecarCompliance({ noticesOnly });
  console.log(
    `[sidecar-compliance] ${
      result.dependencyCount
    } complete dependency sources; bundled notices -> ${
      result.outputDirectory
    }${
      result.completeSourceDirectory
        ? `; source kit -> ${result.completeSourceDirectory}`
        : ''
    }`
  );
}

if (import.meta.main) {
  main().catch(error => {
    const message = error instanceof Error ? error.message : String(error);
    console.error(`[sidecar-compliance] ${message}`);
    process.exitCode = 1;
  });
}
