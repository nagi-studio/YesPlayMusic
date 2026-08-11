#!/usr/bin/env bun
import { readdirSync, realpathSync } from 'node:fs';
import { createConnection } from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseProcessTable } from './lib/processMetrics.mjs';

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..'
);
const baseUrl = 'http://127.0.0.1:28232';
const legacyPlayerUrl = 'http://127.0.0.1:27232/player';
const sidecarPorts = [12_754, 27_232, 27_233, 28_232];
const sleep = milliseconds =>
  new Promise(resolve => setTimeout(resolve, milliseconds));
const includeWebview = !process.argv.includes('--core-only');
let activeTauriProcess = null;

function uniqueArtifact(directory, suffix) {
  const matches = readdirSync(directory)
    .filter(name => name.endsWith(suffix))
    .sort();
  if (matches.length !== 1) {
    throw new Error(
      `Expected one ${suffix} artifact in ${directory}, found ${matches.length}`
    );
  }
  return path.join(directory, matches[0]);
}

export function resolveTauriSmokeExecutable({
  platform = process.platform,
  arch = process.arch,
  root = projectRoot,
  executablePath,
} = {}) {
  if (executablePath) return realpathSync(executablePath);
  if (platform === 'darwin' && arch === 'arm64') {
    return path.join(
      root,
      'src-tauri/target/aarch64-apple-darwin/release/bundle/macos/YesPlayMusic.app/Contents/MacOS/yesplaymusic-tauri'
    );
  }
  if (platform === 'win32' && arch === 'x64') {
    return path.join(
      root,
      'src-tauri/target/x86_64-pc-windows-msvc/release/yesplaymusic-tauri.exe'
    );
  }
  if (platform === 'linux' && arch === 'x64') {
    return uniqueArtifact(
      path.join(
        root,
        'src-tauri/target/x86_64-unknown-linux-gnu/release/bundle/appimage'
      ),
      '.AppImage'
    );
  }
  throw new Error(`Unsupported Tauri smoke host: ${platform}-${arch}`);
}

export function tauriSmokeExitTimeoutMs(includeWebview) {
  return includeWebview ? 35_000 : 15_000;
}

function readProcessTable() {
  const result = Bun.spawnSync([
    'ps',
    '-axo',
    'pid=,ppid=,rss=,%cpu=,command=',
  ]);
  if (result.exitCode !== 0) {
    throw new Error(new TextDecoder().decode(result.stderr).trim());
  }
  return parseProcessTable(new TextDecoder().decode(result.stdout));
}

function forwardOutput(stream, target, onText = () => {}) {
  return (async () => {
    const reader = stream.getReader();
    const decoder = new TextDecoder();
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      target.write(value);
      onText(decoder.decode(value, { stream: true }));
    }
  })();
}

async function waitForReady(timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`${baseUrl}/api/login/status`, {
        signal: AbortSignal.timeout(1_000),
      });
      if (response.ok) return response.json();
    } catch (_) {
      // Connection failures are expected while the sidecar loads its modules.
    }
    await sleep(100);
  }
  throw new Error(`Tauri sidecar 未在 ${timeoutMs / 1_000} 秒内进入 ready`);
}

function portAcceptsConnections(port, timeoutMs = 500) {
  return new Promise(resolve => {
    const socket = createConnection({ host: '127.0.0.1', port });
    const finish = acceptsConnections => {
      socket.destroy();
      resolve(acceptsConnections);
    };
    socket.setTimeout(timeoutMs);
    socket.once('connect', () => finish(true));
    socket.once('timeout', () => finish(false));
    socket.once('error', () => finish(false));
  });
}

async function assertPortStopped(port) {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    if (!(await portAcceptsConnections(port))) return;
    await sleep(100);
  }
  throw new Error(`Tauri 退出后 127.0.0.1:${port} 仍有 listener`);
}

async function main() {
  if (includeWebview && process.platform !== 'darwin') {
    throw new Error('隐藏 WebView smoke 目前只支持 macOS');
  }
  const executable = resolveTauriSmokeExecutable({
    executablePath: process.env.YPM_TAURI_SMOKE_EXECUTABLE,
  });
  const beforePids = includeWebview
    ? new Set(readProcessTable().map(process => process.pid))
    : new Set();
  let resolveWebviewReady;
  let observedOutput = '';
  const startedAt = performance.now();
  let webviewReadyAt = null;
  const webviewReady = new Promise(resolve => {
    resolveWebviewReady = resolve;
  });
  const tauriProcess = Bun.spawn(
    [executable, includeWebview ? '--webview-smoke-test' : '--smoke-test'],
    {
      cwd: projectRoot,
      env:
        process.platform === 'linux'
          ? { ...process.env, APPIMAGE_EXTRACT_AND_RUN: '1' }
          : process.env,
      stdout: 'pipe',
      stderr: 'pipe',
    }
  );
  activeTauriProcess = tauriProcess;
  const stdoutTask = forwardOutput(
    tauriProcess.stdout,
    process.stdout,
    text => {
      observedOutput = `${observedOutput}${text}`.slice(-4_096);
      if (observedOutput.includes('[tauri] webview-ready:')) {
        webviewReadyAt ??= performance.now();
        resolveWebviewReady();
      }
    }
  );
  const stderrTask = forwardOutput(tauriProcess.stderr, process.stderr);

  const loginStatus = await waitForReady();
  const apiReadyAt = performance.now();
  const home = await fetch(baseUrl, {
    signal: AbortSignal.timeout(5_000),
  }).then(response => response.text());
  const [playerInfo, playerInfoAlias] = await Promise.all([
    fetch(legacyPlayerUrl, { signal: AbortSignal.timeout(5_000) }).then(
      response => response.json()
    ),
    fetch(`${baseUrl}/player`, {
      signal: AbortSignal.timeout(5_000),
    }).then(response => response.json()),
  ]);
  if (!home.includes('<div id="app"></div>')) {
    throw new Error('Tauri 首页没有返回 Vue 挂载点');
  }
  if (loginStatus?.data?.code !== 200) {
    throw new Error('Tauri 同源 API 没有返回 200');
  }
  if (
    typeof playerInfo !== 'object' ||
    playerInfo === null ||
    typeof playerInfo.progress !== 'number' ||
    !('currentTrack' in playerInfo)
  ) {
    throw new Error('Tauri 本地播放器状态 API 返回无效');
  }
  if (JSON.stringify(playerInfoAlias) !== JSON.stringify(playerInfo)) {
    throw new Error('28232 /player 别名与 27232 兼容 API 不一致');
  }
  const rendererReadyAt = performance.now();

  let webkitPids = [];
  if (includeWebview) {
    const loaded = await Promise.race([
      webviewReady.then(() => true),
      sleep(10_000).then(() => false),
    ]);
    if (!loaded) throw new Error('隐藏 WebView 没有在 10 秒内完成页面加载');
    await sleep(3_000);
    webkitPids = readProcessTable()
      .filter(
        process =>
          !beforePids.has(process.pid) &&
          process.command.includes('com.apple.WebKit.')
      )
      .map(process => process.pid);
    if (webkitPids.length === 0) {
      throw new Error('没有识别到本次启动创建的 WebKit XPC 进程');
    }
  }

  let metricsOutput = '';
  const supportsProcessMetrics = process.platform !== 'win32';
  if (supportsProcessMetrics) {
    const metricsArgs = [
      'bun',
      'scripts/measure-process-tree.mjs',
      '--pid',
      String(tauriProcess.pid),
      '--duration',
      includeWebview ? '8' : '5',
      '--interval',
      '1',
      '--label',
      includeWebview ? 'tauri-hidden-webview-smoke' : 'tauri-core-smoke',
    ];
    if (webkitPids.length) {
      metricsArgs.push('--include-pids', webkitPids.join(','));
    }
    const metricsProcess = Bun.spawn(metricsArgs, {
      cwd: projectRoot,
      stdout: 'pipe',
      stderr: 'pipe',
    });
    const [output, metricsError, metricsExitCode] = await Promise.all([
      new Response(metricsProcess.stdout).text(),
      new Response(metricsProcess.stderr).text(),
      metricsProcess.exited,
    ]);
    if (metricsExitCode !== 0) throw new Error(metricsError.trim());
    metricsOutput = output;
  }

  const exitCode = await Promise.race([
    tauriProcess.exited,
    sleep(tauriSmokeExitTimeoutMs(includeWebview)).then(() => null),
  ]);
  if (exitCode === null) {
    tauriProcess.kill();
    throw new Error('Tauri smoke 进程没有按时自动退出');
  }
  if (exitCode !== 0) throw new Error(`Tauri smoke 退出码为 ${exitCode}`);
  activeTauriProcess = null;

  await Promise.all([stdoutTask, stderrTask]);
  if (!observedOutput.includes('[sidecar] exited: Some(0)')) {
    throw new Error('Tauri 退出时 Rust Sidecar 未完成 stdin 优雅关闭');
  }
  await Promise.all(sidecarPorts.map(assertPortStopped));
  if (metricsOutput) console.log(metricsOutput.trim());
  console.log(
    JSON.stringify(
      {
        label: includeWebview
          ? 'tauri-installed-webview-readiness'
          : 'tauri-installed-core-readiness',
        milliseconds: {
          apiReady: Math.round(apiReadyAt - startedAt),
          rendererReady: Math.round(rendererReadyAt - startedAt),
          ...(webviewReadyAt === null
            ? {}
            : { webviewReady: Math.round(webviewReadyAt - startedAt) }),
          cleanExit: Math.round(performance.now() - startedAt),
        },
      },
      null,
      2
    )
  );
  console.log(
    `[tauri-smoke] UI、API、${includeWebview ? '隐藏 WebView、' : ''}${
      supportsProcessMetrics ? '内存采样和' : ''
    }进程回收全部通过`
  );
}

if (import.meta.main) {
  main().catch(error => {
    if (activeTauriProcess?.exitCode === null) {
      activeTauriProcess.kill();
    }
    console.error(`[tauri-smoke] ${error.message}`);
    process.exit(1);
  });
}
