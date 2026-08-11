import { describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import {
  collectProcessTree,
  parseMetricsArgs,
  parseProcessTable,
  sanitizeSamples,
  summarizeSanitizedProcesses,
  summarizeProcesses,
  summarizeSamples,
  verifyPerformanceEvidence,
} from '../scripts/lib/processMetrics.mjs';

describe('性能采样参数', () => {
  test('只接受显式根 PID，避免误采样其他 YesPlayMusic', () => {
    expect(() => parseMetricsArgs([])).toThrow('必须通过 --pid 指定根进程');
    expect(parseMetricsArgs(['--pid', '42', '--duration', '5'])).toEqual({
      pid: 42,
      includePids: [],
      durationSeconds: 5,
      intervalSeconds: 1,
      label: 'unnamed',
      evidencePath: null,
      artifactPath: null,
      executablePath: null,
    });
  });

  test('可显式纳入由 launchd 托管的 WebKit XPC 进程', () => {
    expect(
      parseMetricsArgs(['--pid', '42', '--include-pids', '51,52']).includePids
    ).toEqual([51, 52]);
    expect(() =>
      parseMetricsArgs(['--pid', '42', '--include-pids', '51,51'])
    ).toThrow('--include-pids 不能包含重复 PID');
    expect(() =>
      parseMetricsArgs(['--pid', '42', '--include-pids', '42'])
    ).toThrow('--include-pids 不能再包含根 PID');
  });

  test('落盘证据时强制绑定分发物与 installed executable', () => {
    expect(() =>
      parseMetricsArgs(['--pid', '42', '--evidence', 'idle.json'])
    ).toThrow('--evidence 必须同时提供 --artifact 与 --executable');
  });
});

describe('进程树性能采样', () => {
  const table = `
  10     1 102400  1.5 /Applications/Example.app/main
  11    10  51200  2.0 helper --renderer
  12    11  25600  0.5 helper --gpu
  99     1 999999 80.0 unrelated
`;

  test('递归收集子进程，不混入无关应用', () => {
    const tree = collectProcessTree(parseProcessTable(table), 10);
    expect(tree.map(process => process.pid)).toEqual([10, 11, 12]);
  });

  test('解析 LC_ALL=C 的进程 birth token，保留带空格的 executable', () => {
    const [process] = parseProcessTable(
      '10 1 Tue Aug 11 00:00:00 2026 1024 0.1 /Applications/Example App/main'
    );
    expect(process?.startedAt).toBe(
      new Date('Tue Aug 11 00:00:00 2026').toISOString()
    );
    expect(process?.command).toBe('/Applications/Example App/main');
  });

  test('v3 允许同代进程 reparent，并按 birth token 区分 PID reuse', () => {
    const rootStartedAt = '2026-08-11T00:00:00.000Z';
    const firstChildStartedAt = '2026-08-11T00:00:01.000Z';
    const reusedChildStartedAt = '2026-08-11T00:00:02.000Z';
    const root = {
      pid: 10,
      ppid: 1,
      startedAt: rootStartedAt,
      rssKiB: 10240,
      cpuPercent: 1,
      command: '/Applications/Example.app/yesplaymusic-tauri',
    };
    const child = {
      pid: 20,
      ppid: 10,
      startedAt: firstChildStartedAt,
      rssKiB: 5120,
      cpuPercent: 2,
      command: '/Applications/Example.app/helper',
    };
    const rawSamples = sanitizeSamples([
      {
        at: '2026-08-11T00:00:00.000Z',
        rssMiB: 15,
        cpuPercent: 3,
        processes: [root, child],
      },
      {
        at: '2026-08-11T00:00:01.000Z',
        rssMiB: 15,
        cpuPercent: 3,
        processes: [root, { ...child, ppid: 1 }],
      },
    ]);
    const processSummaries = summarizeSanitizedProcesses(rawSamples);
    expect(processSummaries.find(process => process.pid === 20)?.ppids).toEqual(
      [1, 10]
    );
    const executable = {
      realpath: '/Applications/Example.app/yesplaymusic-tauri',
      sha256: 'fixture-sha256',
    };
    expect(
      verifyPerformanceEvidence({
        schemaVersion: 3,
        executable,
        measurement: {
          rootPid: 10,
          includedPids: [20],
          sampleCount: 2,
          durationSeconds: 2,
          intervalSeconds: 1,
          rootExecutableRealpath: executable.realpath,
          rootExecutableSha256: executable.sha256,
          rootProcessStartedAt: rootStartedAt,
        },
        summary: summarizeSamples(rawSamples),
        processSummaries,
        rawSamples,
      })
    ).toEqual(summarizeSamples(rawSamples));

    const reusedSamples = sanitizeSamples([
      {
        at: '2026-08-11T00:00:00.000Z',
        rssMiB: 15,
        cpuPercent: 3,
        processes: [root, child],
      },
      {
        at: '2026-08-11T00:00:01.000Z',
        rssMiB: 15,
        cpuPercent: 3,
        processes: [
          root,
          {
            ...child,
            startedAt: reusedChildStartedAt,
            command: '/Applications/Example.app/reused-helper',
          },
        ],
      },
    ]);
    const reusedProcessSummaries = summarizeSanitizedProcesses(reusedSamples);
    expect(
      reusedProcessSummaries.filter(process => process.pid === 20)
    ).toHaveLength(2);
    const reusedEvidence = {
      schemaVersion: 3,
      executable,
      measurement: {
        rootPid: 10,
        includedPids: [],
        sampleCount: 2,
        durationSeconds: 2,
        intervalSeconds: 1,
        rootExecutableRealpath: executable.realpath,
        rootExecutableSha256: executable.sha256,
        rootProcessStartedAt: rootStartedAt,
      },
      summary: summarizeSamples(reusedSamples),
      processSummaries: reusedProcessSummaries,
      rawSamples: reusedSamples,
    };
    expect(verifyPerformanceEvidence(reusedEvidence)).toEqual(
      summarizeSamples(reusedSamples)
    );
    expect(() =>
      verifyPerformanceEvidence({
        ...reusedEvidence,
        measurement: {
          ...reusedEvidence.measurement,
          includedPids: [20],
        },
      })
    ).toThrow('include PID birth token 发生变化');
  });

  test('v3 拒绝同一 birth token 中途 exec', () => {
    const startedAt = '2026-08-11T00:00:00.000Z';
    const first = {
      pid: 10,
      ppid: 1,
      startedAt,
      rssKiB: 10240,
      cpuPercent: 1,
      command: '/Applications/Example.app/yesplaymusic-tauri',
    };
    expect(() =>
      summarizeProcesses([
        { rssMiB: 10, cpuPercent: 1, processes: [first] },
        {
          rssMiB: 10,
          cpuPercent: 1,
          processes: [{ ...first, command: '/bin/other' }],
        },
      ])
    ).toThrow('进程身份在采样期间发生变化');
  });

  test('证据 verifier 同时复核 raw summary 与分发物 hash', async () => {
    const directory = mkdtempSync(path.join(tmpdir(), 'ypm-perf-evidence-'));
    try {
      const artifactPath = path.join(directory, 'fixture.dmg');
      const executablePath = path.join(directory, 'yesplaymusic-tauri');
      const evidencePath = path.join(directory, 'evidence.json');
      await Bun.write(artifactPath, 'artifact-a');
      await Bun.write(executablePath, 'executable-a');
      const identity = async (filePath: string) => {
        const file = Bun.file(filePath);
        const bytes = await file.arrayBuffer();
        return {
          realpath: filePath,
          bytes: file.size,
          sha256: new Bun.CryptoHasher('sha256').update(bytes).digest('hex'),
        };
      };
      const rawSamples = [
        {
          at: '2026-08-11T00:00:00.000Z',
          rssMiB: 10,
          cpuPercent: 1,
          processes: [
            {
              pid: 10,
              ppid: 1,
              rssKiB: 10240,
              cpuPercent: 1,
              role: 'host',
            },
          ],
        },
      ];
      const artifactIdentity = await identity(artifactPath);
      const executableIdentity = await identity(executablePath);
      await Bun.write(
        evidencePath,
        JSON.stringify({
          schemaVersion: 2,
          artifact: artifactIdentity,
          executable: executableIdentity,
          measurement: {
            rootPid: 10,
            includedPids: [],
            sampleCount: 1,
            durationSeconds: 1,
            intervalSeconds: 1,
            rootExecutableRealpath: executableIdentity.realpath,
            rootExecutableSha256: executableIdentity.sha256,
          },
          summary: summarizeSamples(rawSamples),
          processSummaries: summarizeSanitizedProcesses(rawSamples),
          rawSamples,
        })
      );

      const valid = Bun.spawnSync([
        process.execPath,
        'scripts/verify-performance-evidence.mjs',
        evidencePath,
      ]);
      expect(valid.exitCode).toBe(0);

      await Bun.write(artifactPath, 'artifact-b');
      const tampered = Bun.spawnSync([
        process.execPath,
        'scripts/verify-performance-evidence.mjs',
        evidencePath,
      ]);
      expect(tampered.exitCode).toBe(1);
      expect(new TextDecoder().decode(tampered.stderr)).toContain(
        'artifact SHA-256 不匹配'
      );
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });

  test('显式 WebKit PID 及其子进程会加入统计', () => {
    const tree = collectProcessTree(parseProcessTable(table), 10, [99]);
    expect(tree.map(process => process.pid)).toEqual([10, 11, 12, 99]);
  });

  test('统一输出均值、P95 和峰值', () => {
    expect(
      summarizeSamples([
        { rssMiB: 100, cpuPercent: 1 },
        { rssMiB: 120, cpuPercent: 2 },
        { rssMiB: 300, cpuPercent: 9 },
      ])
    ).toEqual({
      samples: 3,
      rssMiB: { mean: 173.33, p95: 300, max: 300 },
      cpuPercent: { mean: 4, p95: 9, max: 9 },
    });
  });

  test('分别汇总主进程与 Sidecar，不用最后一次采样代替趋势', () => {
    const first = parseProcessTable(table).slice(0, 2);
    const second = first.map(process => ({
      ...process,
      rssKiB: process.rssKiB * 2,
      cpuPercent: process.cpuPercent * 2,
    }));

    expect(
      summarizeProcesses([
        { rssMiB: 150, cpuPercent: 3.5, processes: first },
        { rssMiB: 300, cpuPercent: 7, processes: second },
      ]).map(({ pid, samples, rssMiB, cpuPercent }) => ({
        pid,
        samples,
        rssMiB,
        cpuPercent,
      }))
    ).toEqual([
      {
        pid: 10,
        samples: 2,
        rssMiB: { mean: 150, p95: 200, max: 200 },
        cpuPercent: { mean: 2.25, p95: 3, max: 3 },
      },
      {
        pid: 11,
        samples: 2,
        rssMiB: { mean: 75, p95: 100, max: 100 },
        cpuPercent: { mean: 3, p95: 4, max: 4 },
      },
    ]);
  });

  test('证据保留逐样本数据但不落盘完整命令行，并可重算摘要', () => {
    const raw = [
      {
        at: '2026-08-11T00:00:00.000Z',
        rssMiB: 10,
        cpuPercent: 1,
        processes: [
          {
            pid: 10,
            ppid: 1,
            rssKiB: 10240,
            cpuPercent: 1,
            command: '/private/tmp/Installed.app/yesplaymusic-tauri --secret',
          },
        ],
      },
    ];
    const rawSamples = sanitizeSamples(raw);
    expect(rawSamples[0]?.processes[0]).toEqual({
      pid: 10,
      ppid: 1,
      rssKiB: 10240,
      cpuPercent: 1,
      role: 'host',
    });
    expect(JSON.stringify(rawSamples)).not.toContain('--secret');

    const summary = summarizeSamples(rawSamples);
    expect(
      verifyPerformanceEvidence({
        schemaVersion: 2,
        executable: {
          realpath: '/tmp/yesplaymusic-tauri',
          sha256: 'fixture-sha256',
        },
        measurement: {
          rootPid: 10,
          includedPids: [],
          sampleCount: 1,
          durationSeconds: 1,
          intervalSeconds: 1,
          rootExecutableRealpath: '/tmp/yesplaymusic-tauri',
          rootExecutableSha256: 'fixture-sha256',
        },
        summary,
        processSummaries: summarizeSanitizedProcesses(rawSamples),
        rawSamples,
      })
    ).toEqual(summary);
    expect(() =>
      verifyPerformanceEvidence({
        schemaVersion: 2,
        executable: {
          realpath: '/tmp/yesplaymusic-tauri',
          sha256: 'fixture-sha256',
        },
        measurement: {
          rootPid: 10,
          includedPids: [],
          sampleCount: 1,
          durationSeconds: 1,
          intervalSeconds: 1,
          rootExecutableRealpath: '/tmp/yesplaymusic-tauri',
          rootExecutableSha256: 'fixture-sha256',
        },
        summary: { ...summary, samples: 2 },
        processSummaries: summarizeSanitizedProcesses(rawSamples),
        rawSamples,
      })
    ).toThrow('摘要与逐样本数据不一致');
  });

  test('拒绝缺少根进程、伪造逐进程摘要与空洞单样本', () => {
    const rawSamples = [
      {
        at: '2026-08-11T00:00:00.000Z',
        rssMiB: 10,
        cpuPercent: 1,
        processes: [
          {
            pid: 10,
            ppid: 1,
            rssKiB: 10240,
            cpuPercent: 1,
            role: 'host',
          },
        ],
      },
    ];
    const evidence = {
      schemaVersion: 2,
      executable: {
        realpath: '/tmp/yesplaymusic-tauri',
        sha256: 'fixture-sha256',
      },
      measurement: {
        rootPid: 10,
        includedPids: [],
        sampleCount: 1,
        durationSeconds: 1,
        intervalSeconds: 1,
        rootExecutableRealpath: '/tmp/yesplaymusic-tauri',
        rootExecutableSha256: 'fixture-sha256',
      },
      summary: summarizeSamples(rawSamples),
      processSummaries: summarizeSanitizedProcesses(rawSamples),
      rawSamples,
    };

    expect(() =>
      verifyPerformanceEvidence({
        ...evidence,
        measurement: { ...evidence.measurement, rootPid: 42 },
      })
    ).toThrow('未包含声明的 host 根进程');
    expect(() =>
      verifyPerformanceEvidence({
        ...evidence,
        processSummaries: [
          { ...evidence.processSummaries[0], role: 'sidecar' },
        ],
      })
    ).toThrow('逐进程摘要与逐样本数据不一致');
    expect(() =>
      verifyPerformanceEvidence({
        ...evidence,
        rawSamples: [
          {
            at: 'not-a-date',
            rssMiB: 0,
            cpuPercent: 0,
            processes: [],
          },
        ],
      })
    ).toThrow('时间序列无效');

    const rawSample = rawSamples[0];
    const rootProcess = rawSample?.processes[0];
    if (!rawSample || !rootProcess) throw new Error('测试 fixture 无效');
    const duplicatePidSamples = [
      {
        ...rawSample,
        rssMiB: 20,
        cpuPercent: 2,
        processes: [rootProcess, rootProcess],
      },
    ];
    expect(() =>
      verifyPerformanceEvidence({
        ...evidence,
        summary: summarizeSamples(duplicatePidSamples),
        processSummaries: summarizeSanitizedProcesses(duplicatePidSamples),
        rawSamples: duplicatePidSamples,
      })
    ).toThrow('逐进程结构无效');
    expect(() =>
      verifyPerformanceEvidence({
        ...evidence,
        summary: summarizeSamples([{ ...rawSample, rssMiB: Number.NaN }]),
        rawSamples: [{ ...rawSample, rssMiB: Number.NaN }],
      })
    ).toThrow('逐样本结构无效');
  });

  test('采样器生成的证据可由同一 verifier 复核', async () => {
    if (process.platform === 'win32') return;
    const directory = mkdtempSync(path.join(tmpdir(), 'ypm-perf-measure-'));
    const artifactPath = path.join(directory, 'fixture.dmg');
    const executablePath = path.join(directory, 'yesplaymusic-tauri');
    const evidencePath = path.join(directory, 'evidence.json');
    await Bun.write(artifactPath, 'artifact');
    symlinkSync('/bin/sleep', executablePath);
    const target = Bun.spawn([executablePath, '2'], {
      stdout: 'ignore',
      stderr: 'ignore',
    });

    try {
      const measured = Bun.spawnSync([
        process.execPath,
        'scripts/measure-process-tree.mjs',
        '--pid',
        String(target.pid),
        '--duration',
        '0.2',
        '--interval',
        '0.1',
        '--evidence',
        evidencePath,
        '--artifact',
        artifactPath,
        '--executable',
        executablePath,
      ]);
      expect(new TextDecoder().decode(measured.stderr)).toContain(
        '[performance] evidence:'
      );
      expect(measured.exitCode).toBe(0);

      const generatedEvidence = await Bun.file(evidencePath).json();
      expect(generatedEvidence.schemaVersion).toBe(4);
      expect(generatedEvidence.measurement.samplerVersion).toBe(4);
      expect(Object.keys(generatedEvidence.artifact).sort()).toEqual([
        'bytes',
        'sha256',
      ]);
      expect(Object.keys(generatedEvidence.executable).sort()).toEqual([
        'bytes',
        'sha256',
      ]);
      expect(generatedEvidence.measurement).not.toHaveProperty(
        'rootExecutableRealpath'
      );
      expect(JSON.stringify(generatedEvidence)).not.toContain(directory);
      expect(JSON.stringify(generatedEvidence)).not.toContain('realpath');
      expect(
        verifyPerformanceEvidence({
          ...generatedEvidence,
          measurement: {
            ...generatedEvidence.measurement,
            samplerVersion: 3,
          },
        })
      ).toEqual(generatedEvidence.summary);
      expect(() =>
        verifyPerformanceEvidence({
          ...generatedEvidence,
          executable: {
            ...generatedEvidence.executable,
            realpath: executablePath,
          },
        })
      ).toThrow('schema v4 性能证据不能包含本机路径');

      const missingOverrides = Bun.spawnSync([
        process.execPath,
        'scripts/verify-performance-evidence.mjs',
        evidencePath,
      ]);
      expect(missingOverrides.exitCode).toBe(1);
      expect(new TextDecoder().decode(missingOverrides.stderr)).toContain(
        'schema v4 必须通过 --artifact 与 --executable'
      );

      const verified = Bun.spawnSync([
        process.execPath,
        'scripts/verify-performance-evidence.mjs',
        evidencePath,
        '--artifact',
        artifactPath,
        '--executable',
        executablePath,
      ]);
      expect(verified.exitCode).toBe(0);
    } finally {
      target.kill();
      await target.exited;
      rmSync(directory, { recursive: true, force: true });
    }
  });

  test('拒绝用巨大末尾间隔补样伪造连续采样', () => {
    const rawSamples = [
      '2026-08-11T00:00:00.000Z',
      '2026-08-11T00:00:01.000Z',
      '2026-08-11T00:01:40.000Z',
    ].map(at => ({
      at,
      rssMiB: 10,
      cpuPercent: 1,
      processes: [
        {
          pid: 10,
          ppid: 1,
          rssKiB: 10240,
          cpuPercent: 1,
          role: 'host',
        },
      ],
    }));
    expect(() =>
      verifyPerformanceEvidence({
        schemaVersion: 2,
        executable: {
          realpath: '/tmp/yesplaymusic-tauri',
          sha256: 'fixture-sha256',
        },
        measurement: {
          rootPid: 10,
          includedPids: [],
          sampleCount: 3,
          durationSeconds: 3,
          intervalSeconds: 1,
          rootExecutableRealpath: '/tmp/yesplaymusic-tauri',
          rootExecutableSha256: 'fixture-sha256',
        },
        summary: summarizeSamples(rawSamples),
        processSummaries: summarizeSanitizedProcesses(rawSamples),
        rawSamples,
      })
    ).toThrow('采样间隔');
  });

  test('拒绝把未声明的无关进程混入进程树统计', () => {
    const rawSamples = [0, 1].map(seconds => ({
      at: `2026-08-11T00:00:0${seconds}.000Z`,
      rssMiB: 20,
      cpuPercent: 2,
      processes: [
        {
          pid: 10,
          ppid: 1,
          rssKiB: 10240,
          cpuPercent: 1,
          role: 'host',
        },
        {
          pid: 99,
          ppid: 1,
          rssKiB: 10240,
          cpuPercent: 1,
          role: 'other',
        },
      ],
    }));
    expect(() =>
      verifyPerformanceEvidence({
        schemaVersion: 2,
        executable: {
          realpath: '/tmp/yesplaymusic-tauri',
          sha256: 'fixture-sha256',
        },
        measurement: {
          rootPid: 10,
          includedPids: [],
          sampleCount: 2,
          durationSeconds: 2,
          intervalSeconds: 1,
          rootExecutableRealpath: '/tmp/yesplaymusic-tauri',
          rootExecutableSha256: 'fixture-sha256',
        },
        summary: summarizeSamples(rawSamples),
        processSummaries: summarizeSanitizedProcesses(rawSamples),
        rawSamples,
      })
    ).toThrow('未声明的进程');
  });

  test('在采样前绑定根进程 executable，拒绝中途 exec 替换', async () => {
    if (process.platform === 'win32') return;
    const directory = mkdtempSync(path.join(tmpdir(), 'ypm-perf-pid-bind-'));
    const artifactPath = path.join(directory, 'fixture.dmg');
    const executablePath = path.join(directory, 'yesplaymusic-tauri');
    const evidencePath = path.join(directory, 'evidence.json');
    await Bun.write(artifactPath, 'artifact');
    symlinkSync('/bin/sleep', executablePath);
    const wrapper = Bun.spawn(
      [
        '/bin/sh',
        '-c',
        'sleep 0.2; exec "$1" 5',
        'performance-wrapper',
        executablePath,
      ],
      { stdout: 'ignore', stderr: 'ignore' }
    );

    try {
      const measured = Bun.spawnSync([
        process.execPath,
        'scripts/measure-process-tree.mjs',
        '--pid',
        String(wrapper.pid),
        '--duration',
        '2',
        '--interval',
        '1',
        '--evidence',
        evidencePath,
        '--artifact',
        artifactPath,
        '--executable',
        executablePath,
      ]);
      expect(measured.exitCode).toBe(1);
      expect(new TextDecoder().decode(measured.stderr)).toContain(
        '根进程 executable 与 --executable 不一致'
      );
      expect(await Bun.file(evidencePath).exists()).toBe(false);
    } finally {
      wrapper.kill();
      await wrapper.exited;
      rmSync(directory, { recursive: true, force: true });
    }
  });

  test('采样前后都复核 executable，拒绝开始匹配但结束已替换', async () => {
    if (process.platform === 'win32') return;
    const directory = mkdtempSync(path.join(tmpdir(), 'ypm-perf-exec-swap-'));
    const artifactPath = path.join(directory, 'fixture.dmg');
    const executablePath = path.join(directory, 'yesplaymusic-tauri');
    const evidencePath = path.join(directory, 'evidence.json');
    await Bun.write(artifactPath, 'artifact');
    symlinkSync('/bin/bash', executablePath);
    const target = Bun.spawn(
      [
        executablePath,
        '-c',
        'sleep 0.2; exec /bin/sleep 5',
        'yesplaymusic-tauri',
      ],
      { stdout: 'ignore', stderr: 'ignore' }
    );

    try {
      const measured = Bun.spawnSync([
        process.execPath,
        'scripts/measure-process-tree.mjs',
        '--pid',
        String(target.pid),
        '--duration',
        '2',
        '--interval',
        '1',
        '--evidence',
        evidencePath,
        '--artifact',
        artifactPath,
        '--executable',
        executablePath,
      ]);
      expect(measured.exitCode).toBe(1);
      expect(new TextDecoder().decode(measured.stderr)).toContain(
        '采样期间根进程 executable 或 --executable 文件发生变化'
      );
      expect(await Bun.file(evidencePath).exists()).toBe(false);
    } finally {
      target.kill();
      await target.exited;
      rmSync(directory, { recursive: true, force: true });
    }
  });
});
