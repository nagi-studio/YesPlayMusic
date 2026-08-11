#!/usr/bin/env bun
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
} from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..'
);
const sidecarManifestPath = path.join(
  projectRoot,
  'src-tauri',
  'sidecar',
  'Cargo.toml'
);
const cargoTargetDirectory = path.join(projectRoot, 'src-tauri', 'target');

export const SIDECAR_TARGETS = Object.freeze({
  'aarch64-apple-darwin': { extension: '' },
  'x86_64-pc-windows-msvc': { extension: '.exe' },
  'x86_64-unknown-linux-gnu': { extension: '' },
});

export const WINDOWS_GUI_SUBSYSTEM = 2;

/**
 * Read IMAGE_OPTIONAL_HEADER.Subsystem from a PE image.
 * @param {Uint8Array} bytes
 */
export function parseWindowsPeSubsystem(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const requireBytes = (offset, size, label) => {
    if (offset < 0 || size < 0 || offset + size > view.byteLength) {
      throw new Error(`Windows PE 产物缺少 ${label}`);
    }
  };

  requireBytes(0, 64, 'DOS header');
  if (view.getUint16(0, true) !== 0x5a4d) {
    throw new Error('Windows PE 产物不是 MZ 文件');
  }

  const peHeaderOffset = view.getUint32(0x3c, true);
  requireBytes(peHeaderOffset, 24, 'PE header');
  if (view.getUint32(peHeaderOffset, true) !== 0x00004550) {
    throw new Error('Windows PE 产物缺少 PE\\0\\0 签名');
  }

  const optionalHeaderSize = view.getUint16(peHeaderOffset + 20, true);
  const optionalHeaderOffset = peHeaderOffset + 24;
  requireBytes(optionalHeaderOffset, optionalHeaderSize, 'optional header');
  if (optionalHeaderSize < 70) {
    throw new Error('Windows PE optional header 太短，无法读取 Subsystem');
  }

  const optionalHeaderMagic = view.getUint16(optionalHeaderOffset, true);
  if (optionalHeaderMagic !== 0x10b && optionalHeaderMagic !== 0x20b) {
    throw new Error('Windows PE optional header magic 无效');
  }

  return view.getUint16(optionalHeaderOffset + 68, true);
}

/**
 * Assert that a Windows image is a GUI executable rather than a console app.
 * @param {Uint8Array} bytes
 */
export function assertWindowsGuiPe(bytes) {
  const subsystem = parseWindowsPeSubsystem(bytes);
  if (subsystem !== WINDOWS_GUI_SUBSYSTEM) {
    throw new Error(
      `Windows Sidecar 必须是 GUI PE（Subsystem=${WINDOWS_GUI_SUBSYSTEM}），实际为 ${subsystem}`
    );
  }
}

export function shouldAssertWindowsGuiPe({ targetTriple, profile }) {
  return targetTriple === 'x86_64-pc-windows-msvc' && profile === 'release';
}

export function hostTargetTriple({
  platform = process.platform,
  arch = process.arch,
} = {}) {
  const targetTriple = {
    'darwin-arm64': 'aarch64-apple-darwin',
    'win32-x64': 'x86_64-pc-windows-msvc',
    'linux-x64': 'x86_64-unknown-linux-gnu',
  }[`${platform}-${arch}`];
  if (!targetTriple) {
    throw new Error(`暂不支持在 ${platform}/${arch} 构建 Tauri Sidecar`);
  }
  return targetTriple;
}

export function rustSidecarBuildPlan({
  targetTriple = process.env.TAURI_ENV_TARGET_TRIPLE || hostTargetTriple(),
  profile = 'release',
} = {}) {
  const target = Object.hasOwn(SIDECAR_TARGETS, targetTriple)
    ? SIDECAR_TARGETS[targetTriple]
    : undefined;
  if (!target) {
    throw new Error(`暂不支持 Tauri target：${targetTriple}`);
  }
  if (profile !== 'debug' && profile !== 'release') {
    throw new Error(`暂不支持 Cargo profile：${profile}`);
  }

  const executableName = `yesplaymusic-sidecar${target.extension}`;
  const outputName = `yesplaymusic-sidecar-${targetTriple}${target.extension}`;
  const args = [
    'build',
    '--locked',
    '--manifest-path',
    sidecarManifestPath,
    '--bin',
    'yesplaymusic-sidecar',
    '--target',
    targetTriple,
    '--target-dir',
    cargoTargetDirectory,
  ];
  if (profile === 'release') args.push('--release');

  return {
    targetTriple,
    profile,
    outputName,
    outputPath: path.join(projectRoot, 'src-tauri', 'binaries', outputName),
    artifactPath: path.join(
      cargoTargetDirectory,
      targetTriple,
      profile,
      executableName
    ),
    args,
  };
}

export function installRustSidecarArtifact({ artifactPath, outputPath }) {
  mkdirSync(path.dirname(outputPath), { recursive: true });
  const temporaryPath = `${outputPath}.tmp-${process.pid}`;
  rmSync(temporaryPath, { force: true });
  try {
    copyFileSync(artifactPath, temporaryPath);
    if (process.platform !== 'win32') chmodSync(temporaryPath, 0o755);
    renameSync(temporaryPath, outputPath);
  } catch (error) {
    rmSync(temporaryPath, { force: true });
    throw error;
  }
}

export function buildRustSidecar(options) {
  const plan = rustSidecarBuildPlan(options);
  const result = Bun.spawnSync([process.env.CARGO || 'cargo', ...plan.args], {
    cwd: projectRoot,
    stdout: 'inherit',
    stderr: 'inherit',
  });
  if (result.exitCode !== 0) {
    throw new Error(`Rust Sidecar 构建失败，退出码 ${result.exitCode}`);
  }
  if (shouldAssertWindowsGuiPe(plan)) {
    assertWindowsGuiPe(readFileSync(plan.artifactPath));
  }
  installRustSidecarArtifact(plan);
  console.log(`[rust-sidecar] built ${plan.outputName} (${plan.profile})`);
  return plan;
}

function profileFromArguments(args) {
  if (args.length !== 1 || !['--debug', '--release'].includes(args[0])) {
    throw new Error(
      '用法：bun scripts/build-rust-sidecar.mjs --debug|--release'
    );
  }
  return args[0] === '--debug' ? 'debug' : 'release';
}

if (import.meta.main) {
  try {
    buildRustSidecar({ profile: profileFromArguments(process.argv.slice(2)) });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.error(`[rust-sidecar] ${message}`);
    process.exit(1);
  }
}
