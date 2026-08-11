#!/usr/bin/env bun
import { realpathSync } from 'node:fs';
import { createConnection, createServer } from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseProcessTable } from './lib/processMetrics.mjs';
import { resolveTauriSmokeExecutable } from './smoke-tauri-local.mjs';

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..'
);
const baseUrl = 'http://127.0.0.1:28232';
const sidecarPorts = [12_754, 27_232, 27_233, 28_232];
const sleep = milliseconds =>
  new Promise(resolve => setTimeout(resolve, milliseconds));
let activeHost = null;

export function commandMatchesExecutable(command, executable) {
  return command === executable || command.startsWith(`${executable} `);
}

function hasParentPidArgument(command, hostPid) {
  return new RegExp(`(?:^|\\s)--parent-pid\\s+${hostPid}(?:\\s|$)`).test(
    command
  );
}

export function findOwnedSidecarProcesses(
  processes,
  { hostPid, sidecarExecutable, requireDirectParent = true }
) {
  return processes.filter(
    process =>
      (!requireDirectParent || process.ppid === hostPid) &&
      commandMatchesExecutable(process.command, sidecarExecutable) &&
      hasParentPidArgument(process.command, hostPid)
  );
}

function readProcessTable() {
  const result = Bun.spawnSync([
    'ps',
    '-ww',
    '-axo',
    'pid=,ppid=,rss=,%cpu=,command=',
  ]);
  if (result.exitCode !== 0) {
    throw new Error(new TextDecoder().decode(result.stderr).trim());
  }
  return parseProcessTable(new TextDecoder().decode(result.stdout));
}

function portAcceptsConnections(port, timeoutMs = 300) {
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

async function assertPortsStopped(label, ports = sidecarPorts) {
  for (let attempt = 0; attempt < 30; attempt += 1) {
    const listening = (
      await Promise.all(
        ports.map(async port => [port, await portAcceptsConnections(port)])
      )
    )
      .filter(([, acceptsConnections]) => acceptsConnections)
      .map(([port]) => port);
    if (listening.length === 0) return;
    if (attempt === 29) {
      throw new Error(
        `${label}仍有 listener：${listening
          .map(port => `127.0.0.1:${port}`)
          .join(', ')}`
      );
    }
    await sleep(100);
  }
}

export async function localRuntimeReady(runtimeBaseUrl = baseUrl) {
  try {
    const [healthResponse, playerResponse] = await Promise.all([
      fetch(`${runtimeBaseUrl}/__yesplaymusic/health`, {
        signal: AbortSignal.timeout(1_000),
      }),
      fetch(`${runtimeBaseUrl}/player`, {
        signal: AbortSignal.timeout(1_000),
      }),
    ]);
    if (!healthResponse.ok || !playerResponse.ok) return false;
    const [health, player] = await Promise.all([
      healthResponse.json(),
      playerResponse.json(),
    ]);
    return (
      healthResponse.headers.get('x-yesplaymusic-backend') === 'rust' &&
      health?.service === 'yesplaymusic-sidecar' &&
      health?.protocol === 1 &&
      typeof player === 'object' &&
      player !== null &&
      typeof player.progress === 'number' &&
      'currentTrack' in player
    );
  } catch (_) {
    return false;
  }
}

function captureOutput(stream, target, observed) {
  return (async () => {
    const reader = stream.getReader();
    const decoder = new TextDecoder();
    while (true) {
      const { done, value } = await reader.read();
      if (done) {
        observed.text = `${observed.text}${decoder.decode()}`.slice(-64 * 1024);
        return;
      }
      target.write(value);
      observed.text = `${observed.text}${decoder.decode(value, {
        stream: true,
      })}`.slice(-64 * 1024);
    }
  })();
}

function spawnHost(hostExecutable) {
  const observed = { text: '' };
  const subprocess = Bun.spawn([hostExecutable, '--smoke-test'], {
    cwd: projectRoot,
    env: globalThis.process.env,
    stdout: 'pipe',
    stderr: 'pipe',
  });
  return {
    process: subprocess,
    observed,
    outputTasks: [
      captureOutput(subprocess.stdout, globalThis.process.stdout, observed),
      captureOutput(subprocess.stderr, globalThis.process.stderr, observed),
    ],
  };
}

function assertHostIdentity(processes, hostPid, hostExecutable) {
  const host = processes.find(process => process.pid === hostPid);
  if (!host || !commandMatchesExecutable(host.command, hostExecutable)) {
    throw new Error(`测试 host ${hostPid} 已退出或 executable 身份不匹配`);
  }
}

async function waitForHealthyGeneration({
  host,
  hostExecutable,
  sidecarExecutable,
  excludedPids,
  timeoutMs,
}) {
  const deadline = Date.now() + timeoutMs;
  let lastDiagnostic = '尚未读取进程表';
  while (Date.now() < deadline) {
    const processes = readProcessTable();
    assertHostIdentity(processes, host.pid, hostExecutable);
    const candidates = findOwnedSidecarProcesses(processes, {
      hostPid: host.pid,
      sidecarExecutable,
    }).filter(process => !excludedPids.has(process.pid));
    const runtimeReady = await localRuntimeReady();
    lastDiagnostic = `candidates=${
      candidates.map(process => `${process.pid}/${process.ppid}`).join(',') ||
      'none'
    }, runtimeReady=${runtimeReady}`;
    if (candidates.length > 1) {
      throw new Error(
        `host ${host.pid} 同时出现多个符合身份约束的 Sidecar：${candidates
          .map(process => process.pid)
          .join(', ')}`
      );
    }
    if (candidates.length === 1 && runtimeReady) {
      return candidates[0];
    }
    await sleep(50);
  }
  throw new Error(
    `未在 ${
      timeoutMs / 1_000
    } 秒内观察到新 Sidecar PID 与本地服务恢复（${lastDiagnostic}）`
  );
}

function killVerifiedSidecar({
  host,
  hostExecutable,
  sidecarExecutable,
  expectedPid,
}) {
  const processes = readProcessTable();
  assertHostIdentity(processes, host.pid, hostExecutable);
  const candidates = findOwnedSidecarProcesses(processes, {
    hostPid: host.pid,
    sidecarExecutable,
  });
  if (candidates.length !== 1 || candidates[0]?.pid !== expectedPid) {
    throw new Error(
      `拒绝 SIGKILL：预期 Sidecar ${expectedPid} 的 executable/parent 身份已变化`
    );
  }
  globalThis.process.kill(expectedPid, 'SIGKILL');
}

async function waitForOutput(observed, needle, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (observed.text.includes(needle)) return;
    await sleep(50);
  }
  throw new Error(`未在 ${timeoutMs / 1_000} 秒内观察到日志：${needle}`);
}

async function waitForExit(host, timeoutMs) {
  const timeout = Symbol('timeout');
  const result = await Promise.race([
    host.process.exited,
    sleep(timeoutMs).then(() => timeout),
  ]);
  if (result === timeout) {
    throw new Error(`Tauri host 未在 ${timeoutMs / 1_000} 秒内退出`);
  }
  await Promise.all(host.outputTasks);
  return result;
}

async function waitForNoOwnedSidecars(hostPid, sidecarExecutable) {
  const deadline = Date.now() + 3_000;
  while (Date.now() < deadline) {
    const owned = findOwnedSidecarProcesses(readProcessTable(), {
      hostPid,
      sidecarExecutable,
      requireDirectParent: false,
    });
    if (owned.length === 0) return;
    await sleep(50);
  }
  throw new Error(`host ${hostPid} 退出后仍有测试 Sidecar 残留`);
}

async function reservePort(port) {
  const body = '{"service":"occupied-port-fixture"}';
  const server = createServer(socket => {
    socket.end(
      [
        'HTTP/1.1 200 OK',
        'Content-Type: application/json',
        `Content-Length: ${Buffer.byteLength(body)}`,
        'Connection: close',
        '',
        body,
      ].join('\r\n')
    );
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen({ host: '127.0.0.1', port, exclusive: true }, resolve);
  });
  return server;
}

async function closeServer(server) {
  await new Promise((resolve, reject) =>
    server.close(error => (error ? reject(error) : resolve()))
  );
}

async function cleanupHost(host, sidecarExecutable) {
  if (!host) return;
  if (host.process.exitCode === null) host.process.kill();
  try {
    await waitForExit(host, 10_000);
  } catch (_) {
    if (host.process.exitCode === null) host.process.kill('SIGKILL');
    await host.process.exited;
    await Promise.allSettled(host.outputTasks);
  }
  await waitForNoOwnedSidecars(host.process.pid, sidecarExecutable);
}

async function runConcurrentColdStartScenario(
  hostExecutable,
  sidecarExecutable
) {
  await assertPortsStopped('并发冷启动场景开始前');
  const hosts = Array.from({ length: 4 }, () => spawnHost(hostExecutable));
  try {
    const deadline = Date.now() + 9_000;
    let primary = null;
    while (Date.now() < deadline) {
      const running = hosts.filter(host => host.process.exitCode === null);
      const exited = hosts.filter(host => host.process.exitCode !== null);
      if (
        running.length === 1 &&
        exited.length === hosts.length - 1 &&
        (await localRuntimeReady())
      ) {
        primary = running[0];
        break;
      }
      await sleep(50);
    }
    if (!primary) {
      throw new Error(
        `4 个并发冷启动未收敛为 1 个主实例（running=${
          hosts
            .filter(host => host.process.exitCode === null)
            .map(host => host.process.pid)
            .join(',') || 'none'
        }）`
      );
    }

    const secondaries = hosts.filter(host => host !== primary);
    for (const secondary of secondaries) {
      const exitCode = await waitForExit(secondary, 1_000);
      if (exitCode !== 0) {
        throw new Error(
          `并发冷启动 secondary ${secondary.process.pid} 退出码为 ${exitCode}`
        );
      }
      if (
        secondary.observed.text.includes('Address already in use') ||
        secondary.observed.text.includes('exhausted its restart budget')
      ) {
        throw new Error(
          `并发冷启动 secondary ${secondary.process.pid} 曾错误启动 Sidecar`
        );
      }
    }

    const processes = readProcessTable();
    assertHostIdentity(processes, primary.process.pid, hostExecutable);
    const sidecars = findOwnedSidecarProcesses(processes, {
      hostPid: primary.process.pid,
      sidecarExecutable,
    });
    if (sidecars.length !== 1) {
      throw new Error(
        `并发冷启动收敛后预期 1 个 Sidecar，实际为 ${sidecars.length}`
      );
    }
    const primaryExit = await waitForExit(primary, 15_000);
    if (primaryExit !== 0) {
      throw new Error(`并发冷启动 primary 退出码为 ${primaryExit}`);
    }
    await waitForNoOwnedSidecars(primary.process.pid, sidecarExecutable);
    await assertPortsStopped('并发冷启动场景结束后');
    console.log(
      `[tauri-supervisor-smoke] 4 路并发冷启动：primary ${primary.process.pid} + Sidecar ${sidecars[0].pid}，3 个 secondary 均以 0 退出`
    );
  } finally {
    for (const host of hosts) {
      await cleanupHost(host, sidecarExecutable);
    }
  }
}

async function runOccupiedPortScenario(hostExecutable, sidecarExecutable) {
  await assertPortsStopped('端口占用场景开始前');
  const blocker = await reservePort(28_232);
  let host = null;
  try {
    host = spawnHost(hostExecutable);
    activeHost = { host, sidecarExecutable };
    const exitCode = await waitForExit(host, 15_000);
    activeHost = null;
    if (exitCode === 0) {
      throw new Error('28232 被预占时 Tauri host 意外以 0 退出');
    }
    const bindFailures =
      host.observed.text.split('Address already in use').length - 1;
    if (bindFailures !== 4) {
      throw new Error(
        `端口占用场景预期 4 次真实 bind failure，实际为 ${bindFailures}`
      );
    }
    if (!host.observed.text.includes('exhausted its restart budget')) {
      throw new Error('端口占用场景没有明确耗尽 restart budget');
    }
    await waitForNoOwnedSidecars(host.process.pid, sidecarExecutable);
    await assertPortsStopped('端口占用 host 退出后', [12_754, 27_232, 27_233]);
    if (!blocker.listening || !(await portAcceptsConnections(28_232))) {
      throw new Error(
        '端口占用 host 退出时测试 fixture 意外丢失 28232 listener'
      );
    }
    console.log(
      `[tauri-supervisor-smoke] 28232 占用：host ${host.process.pid} 明确失败且无 Sidecar 残留`
    );
  } finally {
    await cleanupHost(host, sidecarExecutable);
    activeHost = null;
    await closeServer(blocker);
  }
  await assertPortsStopped('端口占用场景结束后');
}

async function runRestartBudgetScenario(hostExecutable, sidecarExecutable) {
  await assertPortsStopped('restart-budget 场景开始前');
  const hostRun = spawnHost(hostExecutable);
  activeHost = { host: hostRun, sidecarExecutable };
  const killedPids = new Set();
  const generations = [];
  try {
    let generation = await waitForHealthyGeneration({
      host: hostRun.process,
      hostExecutable,
      sidecarExecutable,
      excludedPids: killedPids,
      timeoutMs: 5_000,
    });
    generations.push(generation.pid);

    for (let restart = 1; restart <= 3; restart += 1) {
      killVerifiedSidecar({
        host: hostRun.process,
        hostExecutable,
        sidecarExecutable,
        expectedPid: generation.pid,
      });
      killedPids.add(generation.pid);
      generation = await waitForHealthyGeneration({
        host: hostRun.process,
        hostExecutable,
        sidecarExecutable,
        excludedPids: killedPids,
        timeoutMs: 4_000,
      });
      generations.push(generation.pid);
      console.log(
        `[tauri-supervisor-smoke] restart ${restart}/3：PID ${generations.at(
          -2
        )} → ${generation.pid}，本地服务已恢复`
      );
    }

    killVerifiedSidecar({
      host: hostRun.process,
      hostExecutable,
      sidecarExecutable,
      expectedPid: generation.pid,
    });
    killedPids.add(generation.pid);
    await waitForOutput(
      hostRun.observed,
      '后台服务已停止，自动重启失败。请重启应用。',
      2_000
    );

    const hostExitDeadline = Date.now() + 15_000;
    while (hostRun.process.exitCode === null && Date.now() < hostExitDeadline) {
      const owned = findOwnedSidecarProcesses(readProcessTable(), {
        hostPid: hostRun.process.pid,
        sidecarExecutable,
        requireDirectParent: false,
      });
      if (owned.length > 0 || (await localRuntimeReady())) {
        throw new Error('第四代 Sidecar 被强杀后服务意外恢复');
      }
      await sleep(100);
    }
    if (hostRun.process.exitCode === null) {
      throw new Error('restart budget 耗尽后 Tauri host 未在 15 秒内退出');
    }
    const exitCode = await waitForExit(hostRun, 2_000);
    activeHost = null;
    if (exitCode !== 0) {
      throw new Error(`restart-budget 场景 host 退出码为 ${exitCode}`);
    }
    await waitForNoOwnedSidecars(hostRun.process.pid, sidecarExecutable);
    await assertPortsStopped('restart-budget 场景结束后');
    console.log(
      `[tauri-supervisor-smoke] restart budget 耗尽：${generations.join(
        ' → '
      )}，第四代后本地服务未恢复`
    );
  } finally {
    await cleanupHost(hostRun, sidecarExecutable);
    activeHost = null;
  }
}

async function main() {
  if (globalThis.process.platform !== 'darwin') {
    throw new Error('真实 Tauri supervisor adverse smoke 目前只支持 macOS');
  }
  const hostExecutable = resolveTauriSmokeExecutable({
    executablePath: globalThis.process.env.YPM_TAURI_SMOKE_EXECUTABLE,
  });
  const sidecarExecutable = realpathSync(
    path.join(path.dirname(hostExecutable), 'yesplaymusic-sidecar')
  );

  await runConcurrentColdStartScenario(hostExecutable, sidecarExecutable);
  await runOccupiedPortScenario(hostExecutable, sidecarExecutable);
  await runRestartBudgetScenario(hostExecutable, sidecarExecutable);
  console.log(
    '[tauri-supervisor-smoke] 并发冷启动、端口占用、三次恢复、restart-budget exhaustion 与四端口回收全部通过'
  );
}

if (import.meta.main) {
  main().catch(async error => {
    if (activeHost) {
      try {
        await cleanupHost(activeHost.host, activeHost.sidecarExecutable);
      } catch (cleanupError) {
        console.error(
          `[tauri-supervisor-smoke] cleanup failed: ${cleanupError.message}`
        );
      }
    }
    console.error(`[tauri-supervisor-smoke] ${error.message}`);
    globalThis.process.exit(1);
  });
}
