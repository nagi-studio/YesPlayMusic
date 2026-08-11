import { expect, test } from 'bun:test';
import type { ProcessTableEntry } from '../scripts/lib/processMetrics.mjs';
import {
  commandMatchesExecutable,
  findOwnedSidecarProcesses,
  localRuntimeReady,
} from '../scripts/smoke-tauri-supervisor.mjs';
import { tauriSmokeExitTimeoutMs } from '../scripts/smoke-tauri-local.mjs';

const sidecarExecutable =
  '/tmp/Installed Copy/YesPlayMusic.app/Contents/MacOS/yesplaymusic-sidecar';

test('WebView smoke runner outlives the packaged host exit timer', () => {
  expect(tauriSmokeExitTimeoutMs(true)).toBeGreaterThan(25_000);
  expect(tauriSmokeExitTimeoutMs(false)).toBeGreaterThan(12_000);
});

test('supervisor readiness stays hermetic when the external login route is unavailable', async () => {
  const server = Bun.serve({
    port: 0,
    fetch(request) {
      const path = new URL(request.url).pathname;
      if (path === '/__yesplaymusic/health') {
        return Response.json(
          { service: 'yesplaymusic-sidecar', protocol: 1 },
          { headers: { 'X-YesPlayMusic-Backend': 'rust' } }
        );
      }
      if (path === '/player') {
        return Response.json({ currentTrack: null, progress: 0 });
      }
      return Response.json({ code: 504 }, { status: 504 });
    },
  });
  try {
    expect(await localRuntimeReady(`http://127.0.0.1:${server.port}`)).toBe(
      true
    );
  } finally {
    server.stop(true);
  }
});

function processEntry(
  pid: number,
  ppid: number,
  command: string
): ProcessTableEntry {
  return { pid, ppid, command, rssKiB: 1, cpuPercent: 0 };
}

test('executable identity uses an exact command boundary even when the path has spaces', () => {
  expect(commandMatchesExecutable(sidecarExecutable, sidecarExecutable)).toBe(
    true
  );
  expect(
    commandMatchesExecutable(
      `${sidecarExecutable} --api-port 12754`,
      sidecarExecutable
    )
  ).toBe(true);
  expect(
    commandMatchesExecutable(
      `${sidecarExecutable}.old --api-port 12754`,
      sidecarExecutable
    )
  ).toBe(false);
});

test('SIGKILL candidate requires exact executable, direct parent, and parent-pid marker', () => {
  const processes = [
    processEntry(
      201,
      100,
      `${sidecarExecutable} --api-port 12754 --parent-pid 100`
    ),
    processEntry(
      202,
      999,
      `${sidecarExecutable} --api-port 12754 --parent-pid 100`
    ),
    processEntry(
      203,
      100,
      `${sidecarExecutable}.old --api-port 12754 --parent-pid 100`
    ),
    processEntry(
      204,
      100,
      `${sidecarExecutable} --api-port 12754 --parent-pid 101`
    ),
    processEntry(
      205,
      100,
      `${sidecarExecutable} --api-port 12754 --parent-pid 1000`
    ),
  ];

  expect(
    findOwnedSidecarProcesses(processes, {
      hostPid: 100,
      sidecarExecutable,
    }).map(process => process.pid)
  ).toEqual([201]);
  expect(
    findOwnedSidecarProcesses(processes, {
      hostPid: 100,
      sidecarExecutable,
      requireDirectParent: false,
    }).map(process => process.pid)
  ).toEqual([201, 202]);
});
