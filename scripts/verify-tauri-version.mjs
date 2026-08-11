import path from 'node:path';
import { fileURLToPath } from 'node:url';

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..'
);

export function readUniqueCargoLockPackageVersion(cargoLock, packageName) {
  const packageBlocks = cargoLock
    .split(/(?=^\[\[package\]\]\s*$)/m)
    .filter(block => {
      const name = block.match(/^name\s*=\s*"([^"]+)"\s*$/m)?.[1];
      return name === packageName;
    });
  if (packageBlocks.length !== 1) {
    throw new Error(
      `Cargo.lock 中 ${packageName} 必须且只能出现一次，实际 ${packageBlocks.length} 次`
    );
  }
  const version = packageBlocks[0].match(
    /^version\s*=\s*"([^"]+)"\s*$/m
  )?.[1];
  if (!version) {
    throw new Error(`Cargo.lock 中 ${packageName} 缺少 version`);
  }
  return version;
}

export function validateTauriVersions({
  packageVersion,
  tauriVersion,
  cargoVersion,
  sidecarVersion,
  lockCargoVersion,
  lockSidecarVersion,
  tag,
}) {
  const versions = new Set([
    packageVersion,
    tauriVersion,
    cargoVersion,
    sidecarVersion,
    lockCargoVersion,
    lockSidecarVersion,
  ]);
  if (versions.size !== 1) {
    throw new Error(
      `版本号不一致：package=${packageVersion}, tauri=${tauriVersion}, cargo=${cargoVersion}, sidecar=${sidecarVersion}, lock-cargo=${lockCargoVersion}, lock-sidecar=${lockSidecarVersion}`
    );
  }
  if (tag && tag !== `v${packageVersion}`) {
    throw new Error(`tag ${tag} 与应用版本 v${packageVersion} 不一致`);
  }
  return packageVersion;
}

export async function verifyTauriVersions(tag = '') {
  const pkg = await Bun.file(path.join(projectRoot, 'package.json')).json();
  const tauri = await Bun.file(
    path.join(projectRoot, 'src-tauri/tauri.conf.json')
  ).json();
  const cargo = await Bun.file(
    path.join(projectRoot, 'src-tauri/Cargo.toml')
  ).text();
  const sidecarCargo = await Bun.file(
    path.join(projectRoot, 'src-tauri/sidecar/Cargo.toml')
  ).text();
  const cargoLock = await Bun.file(
    path.join(projectRoot, 'src-tauri/Cargo.lock')
  ).text();
  const cargoVersion = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  const sidecarVersion = sidecarCargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  const lockCargoVersion = readUniqueCargoLockPackageVersion(
    cargoLock,
    'yesplaymusic-tauri'
  );
  const lockSidecarVersion = readUniqueCargoLockPackageVersion(
    cargoLock,
    'yesplaymusic-sidecar'
  );
  return validateTauriVersions({
    packageVersion: pkg.version,
    tauriVersion: tauri.version,
    cargoVersion,
    sidecarVersion,
    lockCargoVersion,
    lockSidecarVersion,
    tag,
  });
}

if (import.meta.main) {
  const version = await verifyTauriVersions(process.argv[2] || '');
  console.log(`[tauri-version] ${version}`);
}
