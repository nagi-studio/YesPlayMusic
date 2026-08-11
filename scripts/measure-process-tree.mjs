#!/usr/bin/env bun
import { realpathSync } from 'node:fs';
import { mkdir, writeFile } from 'node:fs/promises';
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
} from './lib/processMetrics.mjs';

const sleep = milliseconds =>
  new Promise(resolve => setTimeout(resolve, milliseconds));

function takeSample(options) {
  const result = Bun.spawnSync(
    ['ps', '-axo', 'pid=,ppid=,lstart=,rss=,%cpu=,comm='],
    {
      env: { ...process.env, LC_ALL: 'C' },
    }
  );
  if (result.exitCode !== 0) {
    throw new Error(new TextDecoder().decode(result.stderr).trim());
  }

  const processes = parseProcessTable(new TextDecoder().decode(result.stdout));
  const tree = collectProcessTree(processes, options.pid, options.includePids);
  if (!tree.some(process => process.pid === options.pid)) {
    throw new Error(`根进程 ${options.pid} 不存在`);
  }

  return {
    at: new Date().toISOString(),
    rssMiB: tree.reduce((total, process) => total + process.rssKiB, 0) / 1024,
    cpuPercent: tree.reduce((total, process) => total + process.cpuPercent, 0),
    processes: tree,
  };
}

async function fileIdentity(filePath) {
  const realpath = realpathSync(filePath);
  const file = Bun.file(realpath);
  const bytes = await file.arrayBuffer();
  return {
    realpath,
    bytes: file.size,
    sha256: new Bun.CryptoHasher('sha256').update(bytes).digest('hex'),
  };
}

function osVersion() {
  const result = Bun.spawnSync(
    process.platform === 'darwin'
      ? ['sw_vers', '-productVersion']
      : ['uname', '-sr']
  );
  return result.exitCode === 0
    ? new TextDecoder().decode(result.stdout).trim()
    : 'unknown';
}

function resolveRootExecutable(pid) {
  if (process.platform === 'linux') {
    return realpathSync(`/proc/${pid}/exe`);
  }
  if (process.platform === 'darwin') {
    const result = Bun.spawnSync([
      'lsof',
      '-a',
      '-p',
      String(pid),
      '-d',
      'txt',
      '-Fn',
    ]);
    if (result.exitCode !== 0) {
      throw new Error(`无法解析根进程 ${pid} 的 executable`);
    }
    const executable = new TextDecoder()
      .decode(result.stdout)
      .split('\n')
      .find(line => line.startsWith('n'))
      ?.slice(1);
    if (!executable) throw new Error(`根进程 ${pid} 没有 executable 记录`);
    return realpathSync(executable);
  }
  throw new Error(`当前平台不支持绑定根进程 executable：${process.platform}`);
}

async function main() {
  const options = parseMetricsArgs(process.argv.slice(2));
  let executable = null;
  let rootExecutableRealpath = null;
  if (options.evidencePath) {
    executable = await fileIdentity(options.executablePath);
    rootExecutableRealpath = resolveRootExecutable(options.pid);
    if (rootExecutableRealpath !== executable.realpath) {
      throw new Error(
        `根进程 executable 与 --executable 不一致：${rootExecutableRealpath}`
      );
    }
  }
  const startedAt = new Date().toISOString();
  const sampleCount = Math.max(
    1,
    Math.ceil(options.durationSeconds / options.intervalSeconds)
  );
  const samples = [];

  for (let index = 0; index < sampleCount; index += 1) {
    samples.push(takeSample(options));
    if (index + 1 < sampleCount) {
      await sleep(options.intervalSeconds * 1000);
    }
  }
  const finishedAt = new Date().toISOString();
  if (options.evidencePath) {
    const finalRootExecutableRealpath = resolveRootExecutable(options.pid);
    const finalExecutable = await fileIdentity(options.executablePath);
    if (
      finalRootExecutableRealpath !== rootExecutableRealpath ||
      finalExecutable.realpath !== executable.realpath ||
      finalExecutable.bytes !== executable.bytes ||
      finalExecutable.sha256 !== executable.sha256
    ) {
      throw new Error('采样期间根进程 executable 或 --executable 文件发生变化');
    }
  }

  const lastProcesses = samples.at(-1).processes.map(process => ({
    pid: process.pid,
    ppid: process.ppid,
    rssMiB: Math.round((process.rssKiB / 1024) * 100) / 100,
    cpuPercent: process.cpuPercent,
    command: process.command,
  }));

  const summary = summarizeSamples(samples);
  const rootProcessStartedAt = samples[0].processes.find(
    process => process.pid === options.pid
  )?.startedAt;
  if (!rootProcessStartedAt) {
    throw new Error(`根进程 ${options.pid} 缺少 birth token`);
  }
  const result = {
    label: options.label,
    rootPid: options.pid,
    includedPids: options.includePids,
    durationSeconds: options.durationSeconds,
    intervalSeconds: options.intervalSeconds,
    ...summary,
    processSummaries: summarizeProcesses(samples),
    lastProcesses,
  };
  console.log(JSON.stringify(result, null, 2));

  if (options.evidencePath) {
    const evidence = {
      schemaVersion: 3,
      artifact: await fileIdentity(options.artifactPath),
      executable,
      measurement: {
        label: options.label,
        rootPid: options.pid,
        includedPids: options.includePids,
        durationSeconds: options.durationSeconds,
        intervalSeconds: options.intervalSeconds,
        sampleCount: samples.length,
        startedAt,
        finishedAt,
        platform: process.platform,
        arch: process.arch,
        osVersion: osVersion(),
        bunVersion: Bun.version,
        samplerVersion: 3,
        rootExecutableRealpath,
        rootExecutableSha256: executable.sha256,
        rootProcessStartedAt,
      },
      summary,
      processSummaries: summarizeSanitizedProcesses(sanitizeSamples(samples)),
      rawSamples: sanitizeSamples(samples),
    };
    verifyPerformanceEvidence(evidence);
    await mkdir(path.dirname(options.evidencePath), { recursive: true });
    await writeFile(
      options.evidencePath,
      `${JSON.stringify(evidence, null, 2)}\n`,
      'utf8'
    );
    console.error(`[performance] evidence: ${options.evidencePath}`);
  }
}

main().catch(error => {
  console.error(error.message);
  process.exit(1);
});
