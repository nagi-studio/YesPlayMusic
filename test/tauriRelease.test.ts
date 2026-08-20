import { expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import {
  assertTauriAppSize,
  RUST_SIDECAR_APP_SIZE_LIMIT_BYTES,
  tauriDmgName,
} from '../scripts/package-tauri-dmg.mjs';
import { verifyAppleReleaseEnvironment } from '../scripts/verify-apple-release-env.mjs';
import {
  readUniqueCargoLockPackageVersion,
  validateTauriVersions,
  verifyTauriVersions,
} from '../scripts/verify-tauri-version.mjs';

const workflow = readFileSync(
  new URL('../.github/workflows/build.yaml', import.meta.url),
  'utf8'
);
const readme = readFileSync(new URL('../README.md', import.meta.url), 'utf8');
const tauriSmoke = readFileSync(
  new URL('../scripts/smoke-tauri-local.mjs', import.meta.url),
  'utf8'
);
const packageJson = JSON.parse(
  readFileSync(new URL('../package.json', import.meta.url), 'utf8')
);

test('CI 官方 Actions 使用 Node.js 24 运行时版本', () => {
  for (const action of [
    'actions/checkout@v7',
    'actions/upload-artifact@v7',
    'actions/download-artifact@v8',
  ]) {
    expect(workflow).toContain(action);
  }

  for (const action of [
    'actions/checkout@v4',
    'actions/cache@v4',
    'actions/upload-artifact@v4',
    'actions/download-artifact@v4',
  ]) {
    expect(workflow).not.toContain(action);
  }
});

test('macOS CI 保留无签名和签名两条发布路径', () => {
  expect(workflow).toContain('targets: aarch64-apple-darwin');
  expect(workflow).toContain('run: bun run build:tauri');
  expect(workflow).toContain('run: bun run package:tauri:dmg');
  expect(workflow).toContain(
    'name: Verify complete macOS distribution set and checksums'
  );
  expect(workflow).toContain('YesPlayMusic_${VERSION}_aarch64.dmg');
  expect(workflow).toContain('YesPlayMusic_${VERSION}_sidecar-source.tar.gz');
  expect(workflow).toContain('YesPlayMusic_${VERSION}_SOURCE-OFFER.md');
  expect(workflow).toContain('shasum -a 256 -c ./*.sha256');
  expect(workflow).toContain('run: bun run verify:tauri:version');
  expect(workflow).toContain('path: dist_tauri/*');
  expect(workflow).toContain('name: YesPlayMusic-macos-aarch64');
  expect(workflow).toContain('files: release/**/*');
  expect(workflow).toContain('run: bun run build:tauri:release');
  expect(workflow).toContain('run: bun run collect:tauri:release-dmg');
});

test('Rust-only .app 体积门禁在 DMG 打包前失败', () => {
  expect(assertTauriAppSize(RUST_SIDECAR_APP_SIZE_LIMIT_BYTES)).toBe(
    RUST_SIDECAR_APP_SIZE_LIMIT_BYTES
  );
  expect(() =>
    assertTauriAppSize(RUST_SIDECAR_APP_SIZE_LIMIT_BYTES + 1)
  ).toThrow('54.1 MiB');
});

test('Windows CI 上传测试包并把 tag updater 交给 release job', () => {
  const windowsJob = workflow.slice(
    workflow.indexOf('  build-tauri-windows-x64:'),
    workflow.indexOf('  build-tauri-linux-x64:')
  );
  const releaseJob = workflow.slice(workflow.indexOf('  draft-release:'));

  expect(windowsJob).toContain('runs-on: windows-latest');
  expect(windowsJob).toContain('permissions:\n      contents: read');
  expect(windowsJob).toContain(
    'bun install --frozen-lockfile --ignore-scripts'
  );
  expect(windowsJob).toContain('run: bun run build:tauri:windows');
  expect(windowsJob).toContain(
    'yesplaymusic-sidecar-x86_64-pc-windows-msvc.exe'
  );
  expect(windowsJob).toContain('--unm-smoke-test');
  expect(windowsJob).toContain("if: github.event_name != 'pull_request'");
  expect(windowsJob).toContain('Get-FileHash $_.FullName -Algorithm SHA256');
  expect(windowsJob).toContain('dist_tauri_windows/SHA256SUMS-windows-x64.txt');
  expect(windowsJob).toContain('$hashes -join "`n"');
  expect(windowsJob).toContain('[System.IO.File]::WriteAllText');
  expect(windowsJob).toContain(
    'dist_tauri_windows/TESTING-NOTICE-windows-x64.txt'
  );
  expect(windowsJob).toContain('Do not disable antivirus');
  expect(windowsJob).toContain('retention-days: 7');
  expect(windowsJob).toContain(
    'verify-packaged-app-compliance.mjs windows-x86_64'
  );
  expect(windowsJob).toContain('Get-Command 7z -ErrorAction Stop');
  expect(releaseJob).toContain('YesPlayMusic-windows-x64');
});

test('Ubuntu CI 构建 AppImage、deb 并把 tag updater 交给 release job', () => {
  const linuxJob = workflow.slice(
    workflow.indexOf('  build-tauri-linux-x64:'),
    workflow.indexOf('  draft-release:')
  );
  const releaseJob = workflow.slice(workflow.indexOf('  draft-release:'));

  expect(linuxJob).toContain('runs-on: ubuntu-22.04');
  expect(linuxJob).toContain('dbus \\');
  expect(linuxJob).toContain('libwebkit2gtk-4.1-dev');
  expect(linuxJob).toContain('bun run build:tauri:linux');
  expect(packageJson.scripts['build:tauri:linux']).toContain('--verbose');
  expect(packageJson.scripts['build:tauri:linux']).toContain(
    '--bundles deb,appimage'
  );
  expect(linuxJob).toContain(
    'yesplaymusic-sidecar-x86_64-unknown-linux-gnu --unm-smoke-test'
  );
  expect(linuxJob).toContain('YPM_APPIMAGE_SMOKE_CACHE="$(mktemp -d)"');
  expect(linuxJob).toContain(
    'XDG_CACHE_HOME="$YPM_APPIMAGE_SMOKE_CACHE" dbus-run-session'
  );
  expect(linuxJob).toContain('dpkg-deb -x');
  expect(linuxJob).toContain(
    'test ! -e "$YPM_DEB_SMOKE_ROOT/usr/lib/yesplaymusic/sidecar.payload"'
  );
  expect(linuxJob).toContain(
    '"$YPM_DEB_SMOKE_ROOT/usr/bin/yesplaymusic-sidecar" --unm-smoke-test'
  );
  expect(linuxJob).toContain('bundle/appimage/*.AppImage');
  expect(linuxJob).toContain('bundle/deb/*.deb');
  expect(linuxJob).toContain('squashfs-tools');
  expect(linuxJob).toContain(
    'verify-packaged-app-compliance.mjs linux-x86_64-appimage'
  );
  expect(linuxJob).toContain(
    'verify-packaged-app-compliance.mjs linux-x86_64-deb'
  );
  expect(linuxJob).toContain('sha256sum -c SHA256SUMS-linux-x64.txt');
  expect(releaseJob).toContain('YesPlayMusic-linux-x64');
});

test('三平台发布资产 basename 不会在 draft release 中相互覆盖', () => {
  for (const name of [
    'SHA256SUMS-windows-x64.txt',
    'TESTING-NOTICE-windows-x64.txt',
    'SHA256SUMS-linux-x64.txt',
    'TESTING-NOTICE-linux-x64.txt',
  ]) {
    expect(workflow).toContain(name);
  }
  expect(workflow).not.toContain('dist_tauri_windows/SHA256SUMS.txt');
  expect(workflow).not.toContain('dist_tauri_windows/TESTING-NOTICE.txt');
  expect(workflow).not.toContain('> SHA256SUMS.txt');
  expect(workflow).not.toContain('> TESTING-NOTICE.txt');
});

test('三平台 CI 都在打包后启动 Tauri 主程序做 core smoke', () => {
  const jobs = [
    workflow.slice(
      workflow.indexOf('  build-tauri-arm64:'),
      workflow.indexOf('  build-tauri-windows-x64:')
    ),
    workflow.slice(
      workflow.indexOf('  build-tauri-windows-x64:'),
      workflow.indexOf('  build-tauri-linux-x64:')
    ),
    workflow.slice(
      workflow.indexOf('  build-tauri-linux-x64:'),
      workflow.indexOf('  draft-release:')
    ),
  ];
  for (const job of jobs) {
    expect(job).toContain('smoke:tauri:core');
  }
  expect(jobs[2]).toContain(
    'dbus-run-session -- xvfb-run -a bun run smoke:tauri:core'
  );
  expect(tauriSmoke).toContain('waitForReady(timeoutMs = 30_000)');
});

test('README 与 Tauri 打包配置一致要求 macOS 14', () => {
  expect(readme).toContain('macOS 14');
});

test('三平台测试包只保留七天，避免连续 push 堆积 Artifact', () => {
  expect(workflow.match(/retention-days: 7/g)).toHaveLength(3);
  expect(workflow).not.toContain('retention-days: 14');
});

test('三平台干净 runner 在 Rust 测试前先生成 Tauri 资源', () => {
  const jobBoundaries = [
    ['  build-tauri-arm64:', '  build-tauri-windows-x64:'],
    ['  build-tauri-windows-x64:', '  build-tauri-linux-x64:'],
    ['  build-tauri-linux-x64:', '  draft-release:'],
  ] as const;

  for (const [start, end] of jobBoundaries) {
    const job = workflow.slice(workflow.indexOf(start), workflow.indexOf(end));
    const sidecarTestIndex = job.indexOf(
      'cargo test --locked --manifest-path src-tauri/sidecar/Cargo.toml'
    );
    // Must be build:tauri:renderer: plain build:renderer skips
    // build-app-compliance.mjs, so generated/app-compliance is missing and the
    // Tauri build script rejects the declared resource path.
    const rendererIndex = job.indexOf('bun run build:tauri:renderer');
    const sidecarIndex = job.indexOf('bun run build:sidecar');
    const rustTestIndex = job.indexOf(
      'run: cargo test --locked --manifest-path src-tauri/Cargo.toml'
    );

    expect(sidecarTestIndex).toBeGreaterThan(-1);
    expect(rendererIndex).toBeGreaterThan(sidecarTestIndex);
    expect(sidecarIndex).toBeGreaterThan(rendererIndex);
    expect(rustTestIndex).toBeGreaterThan(sidecarIndex);
  }
});

test('版本 tag 默认走无 Developer ID 签名路径', () => {
  expect(workflow).toContain("vars.APPLE_SIGNING_ENABLED != 'true'");
  expect(workflow).toContain('run: bun run build:tauri');
  expect(workflow).toContain('run: bun run package:tauri:dmg');
});

test('显式开启 Apple 签名后才要求公证和 stapler 验证', () => {
  expect(workflow).toContain("vars.APPLE_SIGNING_ENABLED == 'true'");
  for (const secret of [
    'APPLE_CERTIFICATE',
    'APPLE_CERTIFICATE_PASSWORD',
    'APPLE_SIGNING_IDENTITY',
    'APPLE_ID',
    'APPLE_PASSWORD',
    'APPLE_TEAM_ID',
    'KEYCHAIN_PASSWORD',
  ]) {
    expect(workflow).toContain(`secrets.${secret}`);
  }
  expect(workflow).toContain('xcrun stapler validate');
  expect(workflow).toContain('spctl --assess --type execute');
  // Tauri only notarizes the .app; the DMG is the file that carries
  // quarantine, so it needs its own submission and its own staple. v0.9.2's
  // second attempt failed on exactly this — the app validated clean and the
  // DMG reported "does not have a ticket stapled to it".
  expect(workflow).toContain('xcrun notarytool submit');
  expect(workflow).toContain('xcrun stapler staple');
  expect(packageJson.scripts['build:tauri:release']).toContain(
    '--developer-id'
  );
  expect(packageJson.scripts['build:tauri:release']).not.toContain(
    'sign:tauri:local'
  );
});

test('缺少 Apple 发版密钥时在构建前立即失败', () => {
  expect(() =>
    verifyAppleReleaseEnvironment({ APPLE_ID: 'owner@example.com' })
  ).toThrow('APPLE_CERTIFICATE');
  expect(
    verifyAppleReleaseEnvironment({
      APPLE_CERTIFICATE: 'certificate',
      APPLE_CERTIFICATE_PASSWORD: 'certificate-password',
      APPLE_SIGNING_IDENTITY: 'Developer ID Application: Example',
      APPLE_ID: 'owner@example.com',
      APPLE_PASSWORD: 'app-specific-password',
      APPLE_TEAM_ID: 'TEAMID',
      KEYCHAIN_PASSWORD: 'temporary-keychain-password',
    })
  ).toBe(true);
});

test('tag 和所有应用版本字段必须完全一致', () => {
  const matchingVersions = {
    packageVersion: '0.6.0',
    tauriVersion: '0.6.0',
    cargoVersion: '0.6.0',
    coreVersion: '0.6.0',
    sidecarVersion: '0.6.0',
    tuiVersion: '0.6.0',
    lockCargoVersion: '0.6.0',
    lockCoreVersion: '0.6.0',
    lockSidecarVersion: '0.6.0',
    lockTuiVersion: '0.6.0',
  };

  expect(
    validateTauriVersions({
      ...matchingVersions,
      tag: 'v0.6.0',
    })
  ).toBe('0.6.0');
  expect(() =>
    validateTauriVersions({
      ...matchingVersions,
      tauriVersion: '0.5.0',
      tag: 'v0.6.0',
    })
  ).toThrow('版本号不一致');
  expect(() =>
    validateTauriVersions({
      ...matchingVersions,
      sidecarVersion: '0.5.0',
      tag: 'v0.6.0',
    })
  ).toThrow('sidecar=0.5.0');
  expect(() =>
    validateTauriVersions({
      ...matchingVersions,
      tuiVersion: '0.5.0',
      tag: 'v0.6.0',
    })
  ).toThrow('tui=0.5.0');
  expect(() =>
    validateTauriVersions({
      ...matchingVersions,
      lockCargoVersion: '0.5.0',
      tag: 'v0.6.0',
    })
  ).toThrow('lock-cargo=0.5.0');
  expect(() =>
    validateTauriVersions({
      ...matchingVersions,
      lockSidecarVersion: '0.5.0',
      tag: 'v0.6.0',
    })
  ).toThrow('lock-sidecar=0.5.0');
  expect(() =>
    validateTauriVersions({
      ...matchingVersions,
      lockTuiVersion: '0.5.0',
      tag: 'v0.6.0',
    })
  ).toThrow('lock-tui=0.5.0');
  expect(() =>
    validateTauriVersions({
      ...matchingVersions,
      coreVersion: '0.5.0',
      tag: 'v0.6.0',
    })
  ).toThrow('core=0.5.0');
  expect(() =>
    validateTauriVersions({
      ...matchingVersions,
      lockCoreVersion: '0.5.0',
      tag: 'v0.6.0',
    })
  ).toThrow('lock-core=0.5.0');
});

test('Cargo.lock 中四个 workspace package 都必须唯一存在', () => {
  for (const packageName of [
    'yesplaymusic-tauri',
    'yesplaymusic-core',
    'yesplaymusic-sidecar',
    'yesplaymusic-tui',
  ]) {
    const block = `[[package]]\nname = "${packageName}"\nversion = "0.8.0-canary.1"\n`;
    expect(readUniqueCargoLockPackageVersion(block, packageName)).toBe(
      '0.8.0-canary.1'
    );
    expect(() => readUniqueCargoLockPackageVersion('', packageName)).toThrow(
      `${packageName} 必须且只能出现一次`
    );
    expect(() =>
      readUniqueCargoLockPackageVersion(`${block}\n${block}`, packageName)
    ).toThrow(`${packageName} 必须且只能出现一次`);
  }
});

test('当前 Tauri 发布版本保持一致', async () => {
  expect(await verifyTauriVersions()).toBe(packageJson.version);
});

test('版本 tag 创建草稿 release，带连字符的版本保持 prerelease', () => {
  expect(workflow).toContain("if: startsWith(github.ref, 'refs/tags/v')");
  expect(workflow).toContain('contents: write');
  expect(workflow).toContain('draft: true');
  expect(workflow).toContain(
    "prerelease: ${{ contains(github.ref_name, '-') }}"
  );
});

test('DMG 文件名明确标记版本和 Apple Silicon 架构', () => {
  expect(tauriDmgName('0.6.0')).toBe('YesPlayMusic_0.6.0_aarch64.dmg');
});

test('README 区分三次重构与各平台支持状态', () => {
  expect(readme).toContain('macOS Tauri 重构版');
  expect(readme).toContain('## 三次桌面重构');
  expect(readme).toContain('Electron → Tauri 2');
  expect(readme).toContain('Bun Sidecar → Rust Sidecar');
  expect(readme).toContain('core::ncm');
  // Exact figures live in docs/performance-baseline.md; README keeps rounded ones.
  expect(readme).toContain('82.6 MiB 降到 23.0 MiB');
  expect(readme).toContain('DMG 12.0 MiB');
  expect(readme).toContain('Rust Sidecar');
  expect(readme).toContain('桌面包不再携带 Bun runtime');
  expect(readme).toContain('ad-hoc Hardened Runtime seal');
  expect(readme).toContain('stable 只');
  expect(readme).toContain('canary 只接收 canary 更新');
  expect(readme).toContain('`master` push');
  expect(readme).toContain('docs/performance-baseline.md');
  expect(readme).toContain('bun run build:tauri');
  expect(readme).toContain('bun run package:tauri:dmg');
  expect(readme).toContain('bun run build:tauri:windows');
  expect(readme).toContain('bun run build:tauri:linux');
  expect(readme).toContain('NSIS `.exe`');
  expect(readme).toContain('AppImage');
  expect(readme).not.toContain('Intel 选 `x64`');
});
