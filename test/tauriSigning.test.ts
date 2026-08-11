import { expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { createLocalSigningSteps } from '../scripts/tauriSigning.mjs';

test('Tauri 本地包分别签 Rust Sidecar、主程序和 app', () => {
  const appPath = path.join(path.sep, 'tmp', 'YesPlayMusic.app');
  const steps = createLocalSigningSteps(appPath);

  expect(steps.map(step => step.label)).toEqual([
    '签名 Rust Sidecar',
    '签名 Tauri 主程序',
    '签名 app bundle',
    '严格校验完整 app bundle',
  ]);
  expect(steps[0].args.at(-1)).toBe(
    path.join(appPath, 'Contents', 'MacOS', 'yesplaymusic-sidecar')
  );
  expect(steps[1].args.at(-1)).toBe(
    path.join(appPath, 'Contents', 'MacOS', 'yesplaymusic-tauri')
  );
  expect(steps[3].args).toContain('--deep');
  expect(steps[3].args).toContain('--strict');
});

test('Rust-only hardened runtime 不申请 Bun 所需的弱化权限', () => {
  const entitlements = readFileSync('src-tauri/Entitlements.plist', 'utf8');
  for (const entitlement of [
    'com.apple.security.cs.allow-jit',
    'com.apple.security.cs.allow-unsigned-executable-memory',
    'com.apple.security.cs.disable-executable-page-protection',
    'com.apple.security.cs.disable-library-validation',
    'com.apple.security.cs.allow-dyld-environment-variables',
  ]) {
    expect(entitlements).not.toContain(entitlement);
  }
});
