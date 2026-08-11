import path from 'node:path';
import { fileURLToPath } from 'node:url';

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..'
);
const defaultEntitlements = path.join(
  projectRoot,
  'src-tauri/Entitlements.plist'
);

export const defaultTauriAppPath = path.join(
  projectRoot,
  'src-tauri/target/aarch64-apple-darwin/release/bundle/macos/YesPlayMusic.app'
);

function signingArgs(entitlements, target) {
  return [
    '--entitlements',
    entitlements,
    '--force',
    '--options',
    'runtime',
    '--timestamp=none',
    '--sign',
    '-',
    target,
  ];
}

export function createLocalSigningSteps(
  appPath = defaultTauriAppPath,
  entitlements = defaultEntitlements
) {
  const macOSDir = path.join(appPath, 'Contents/MacOS');
  const sidecar = path.join(macOSDir, 'yesplaymusic-sidecar');
  const main = path.join(macOSDir, 'yesplaymusic-tauri');

  return [
    {
      label: '签名 Rust Sidecar',
      args: signingArgs(entitlements, sidecar),
    },
    {
      label: '签名 Tauri 主程序',
      args: signingArgs(entitlements, main),
    },
    {
      label: '签名 app bundle',
      args: signingArgs(entitlements, appPath),
    },
    {
      label: '严格校验完整 app bundle',
      args: ['--verify', '--deep', '--strict', '--verbose=4', appPath],
    },
  ];
}

export function signLocalTauriBundle(
  appPath = defaultTauriAppPath,
  run = args => Bun.spawnSync(['codesign', ...args])
) {
  for (const step of createLocalSigningSteps(appPath)) {
    console.log(`[tauri-sign] ${step.label}`);
    const result = run(step.args);
    if (result.exitCode !== 0) {
      const error = new TextDecoder().decode(result.stderr || '').trim();
      throw new Error(`${step.label}失败${error ? `：${error}` : ''}`);
    }
  }
}
