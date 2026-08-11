import { isDeepStrictEqual } from 'node:util';
import { createHash } from 'node:crypto';

const evidenceSchemaVersions = new Set([2, 3]);
const processRoles = new Set([
  'host',
  'sidecar',
  'webkit-content',
  'webkit-network',
  'webkit-gpu',
  'other',
]);

function parsePositiveNumber(value, flag) {
  const number = Number(value);
  if (!Number.isFinite(number) || number <= 0) {
    throw new Error(`${flag} 必须是正数`);
  }
  return number;
}

export function parseMetricsArgs(args) {
  const options = {
    pid: null,
    includePids: [],
    durationSeconds: 60,
    intervalSeconds: 1,
    label: 'unnamed',
    evidencePath: null,
    artifactPath: null,
    executablePath: null,
  };

  for (let index = 0; index < args.length; index += 1) {
    const flag = args[index];
    const value = args[index + 1];
    if (!value || value.startsWith('--')) throw new Error(`${flag} 缺少参数`);

    switch (flag) {
      case '--pid':
        options.pid = parsePositiveNumber(value, flag);
        if (!Number.isInteger(options.pid)) throw new Error('--pid 必须是整数');
        break;
      case '--duration':
        options.durationSeconds = parsePositiveNumber(value, flag);
        break;
      case '--include-pids':
        options.includePids = value.split(',').map(pid => {
          const parsed = parsePositiveNumber(pid, flag);
          if (!Number.isInteger(parsed)) {
            throw new Error(`${flag} 必须是整数列表`);
          }
          return parsed;
        });
        break;
      case '--interval':
        options.intervalSeconds = parsePositiveNumber(value, flag);
        break;
      case '--label':
        options.label = value;
        break;
      case '--evidence':
        options.evidencePath = value;
        break;
      case '--artifact':
        options.artifactPath = value;
        break;
      case '--executable':
        options.executablePath = value;
        break;
      default:
        throw new Error(`未知参数：${flag}`);
    }
    index += 1;
  }

  if (!options.pid) throw new Error('必须通过 --pid 指定根进程');
  if (
    options.evidencePath &&
    (!options.artifactPath || !options.executablePath)
  ) {
    throw new Error('--evidence 必须同时提供 --artifact 与 --executable');
  }
  if (new Set(options.includePids).size !== options.includePids.length) {
    throw new Error('--include-pids 不能包含重复 PID');
  }
  if (options.includePids.includes(options.pid)) {
    throw new Error('--include-pids 不能再包含根 PID');
  }
  return options;
}

export function parseProcessTable(text) {
  return text
    .split('\n')
    .map(line => {
      const match = line.match(
        /^\s*(\d+)\s+(\d+)\s+([A-Z][a-z]{2}\s+[A-Z][a-z]{2}\s+\d{1,2}\s+\d{2}:\d{2}:\d{2}\s+\d{4})\s+(\d+)\s+([\d.]+)\s+(.*)$/
      );
      if (match) {
        const startedAt = new Date(match[3]).toISOString();
        return {
          pid: Number(match[1]),
          ppid: Number(match[2]),
          startedAt,
          rssKiB: Number(match[4]),
          cpuPercent: Number(match[5]),
          command: match[6],
        };
      }
      const legacyMatch = line.match(
        /^\s*(\d+)\s+(\d+)\s+(\d+)\s+([\d.]+)\s+(.*)$/
      );
      if (!legacyMatch) return null;
      return {
        pid: Number(legacyMatch[1]),
        ppid: Number(legacyMatch[2]),
        rssKiB: Number(legacyMatch[3]),
        cpuPercent: Number(legacyMatch[4]),
        command: legacyMatch[5],
      };
    })
    .filter(Boolean);
}

export function collectProcessTree(processes, rootPid, includePids = []) {
  const selectedPids = new Set([rootPid, ...includePids]);
  let changed = true;

  while (changed) {
    changed = false;
    for (const process of processes) {
      if (selectedPids.has(process.ppid) && !selectedPids.has(process.pid)) {
        selectedPids.add(process.pid);
        changed = true;
      }
    }
  }

  return processes.filter(process => selectedPids.has(process.pid));
}

function percentile(values, ratio) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.max(0, Math.ceil(sorted.length * ratio) - 1)];
}

function round(value) {
  return Math.round(value * 100) / 100;
}

export function summarizeSamples(samples) {
  if (samples.length === 0) throw new Error('没有可汇总的采样');

  const rssValues = samples.map(sample => sample.rssMiB);
  const cpuValues = samples.map(sample => sample.cpuPercent);
  const average = values =>
    values.reduce((total, value) => total + value, 0) / values.length;

  return {
    samples: samples.length,
    rssMiB: {
      mean: round(average(rssValues)),
      p95: round(percentile(rssValues, 0.95)),
      max: round(Math.max(...rssValues)),
    },
    cpuPercent: {
      mean: round(average(cpuValues)),
      p95: round(percentile(cpuValues, 0.95)),
      max: round(Math.max(...cpuValues)),
    },
  };
}

export function summarizeProcesses(samples) {
  const byPid = new Map();
  for (const sample of samples) {
    for (const process of sample.processes ?? []) {
      const identity = process.startedAt
        ? `${process.pid}\0${process.startedAt}`
        : process.pid;
      const metrics = byPid.get(identity) ?? {
        pid: process.pid,
        ppid: process.ppid,
        command: process.command,
        ...(process.startedAt ? { startedAt: process.startedAt } : {}),
        ppids: new Set(),
        samples: [],
      };
      if (
        (process.startedAt && metrics.command !== process.command) ||
        (!process.startedAt &&
          (metrics.ppid !== process.ppid ||
            metrics.command !== process.command))
      ) {
        throw new Error(`PID ${process.pid} 的进程身份在采样期间发生变化`);
      }
      metrics.ppids.add(process.ppid);
      metrics.samples.push({
        rssMiB: process.rssKiB / 1024,
        cpuPercent: process.cpuPercent,
      });
      byPid.set(identity, metrics);
    }
  }

  return [...byPid.values()]
    .sort((left, right) => left.pid - right.pid)
    .map(process => ({
      pid: process.pid,
      ...(process.startedAt
        ? {
            startedAt: process.startedAt,
            ppids: [...process.ppids].sort((left, right) => left - right),
          }
        : { ppid: process.ppid }),
      command: process.command,
      ...summarizeSamples(process.samples),
    }));
}

function execTokenHash(command) {
  return createHash('sha256').update(command).digest('hex');
}

export function summarizeSanitizedProcesses(samples) {
  const byPid = new Map();
  for (const sample of samples) {
    for (const process of sample.processes ?? []) {
      const identity = process.startedAt
        ? `${process.pid}\0${process.startedAt}`
        : process.pid;
      const metrics = byPid.get(identity) ?? {
        pid: process.pid,
        ppid: process.ppid,
        role: process.role,
        ...(process.startedAt ? { startedAt: process.startedAt } : {}),
        ...(process.execTokenHash
          ? { execTokenHash: process.execTokenHash }
          : {}),
        ppids: new Set(),
        samples: [],
      };
      if (
        (process.startedAt &&
          (metrics.role !== process.role ||
            metrics.execTokenHash !== process.execTokenHash)) ||
        (!process.startedAt &&
          (metrics.ppid !== process.ppid || metrics.role !== process.role))
      ) {
        throw new Error(`PID ${process.pid} 的进程身份在采样期间发生变化`);
      }
      metrics.ppids.add(process.ppid);
      metrics.samples.push({
        rssMiB: process.rssKiB / 1024,
        cpuPercent: process.cpuPercent,
      });
      byPid.set(identity, metrics);
    }
  }

  return [...byPid.values()]
    .sort((left, right) => left.pid - right.pid)
    .map(process => ({
      pid: process.pid,
      ...(process.startedAt
        ? {
            startedAt: process.startedAt,
            execTokenHash: process.execTokenHash,
            ppids: [...process.ppids].sort((left, right) => left - right),
          }
        : { ppid: process.ppid }),
      role: process.role,
      ...summarizeSamples(process.samples),
    }));
}

export function processRole(command) {
  if (command.includes('yesplaymusic-sidecar')) return 'sidecar';
  if (command.includes('yesplaymusic-tauri')) return 'host';
  if (command.includes('com.apple.WebKit.WebContent')) return 'webkit-content';
  if (command.includes('com.apple.WebKit.Networking')) return 'webkit-network';
  if (command.includes('com.apple.WebKit.GPU')) return 'webkit-gpu';
  return 'other';
}

export function sanitizeSamples(samples) {
  return samples.map(sample => ({
    at: sample.at,
    rssMiB: sample.rssMiB,
    cpuPercent: sample.cpuPercent,
    processes: (sample.processes ?? []).map(process => ({
      pid: process.pid,
      ppid: process.ppid,
      ...(process.startedAt ? { startedAt: process.startedAt } : {}),
      ...(process.startedAt
        ? { execTokenHash: execTokenHash(process.command) }
        : {}),
      rssKiB: process.rssKiB,
      cpuPercent: process.cpuPercent,
      role: processRole(process.command),
    })),
  }));
}

function isNonEmptyString(value) {
  return typeof value === 'string' && value.trim().length > 0;
}

function parseCanonicalTimestamp(value) {
  if (!isNonEmptyString(value)) {
    throw new Error('性能证据时间序列无效');
  }
  const timestamp = Date.parse(value);
  if (
    !Number.isFinite(timestamp) ||
    new Date(timestamp).toISOString() !== value
  ) {
    throw new Error('性能证据时间序列无效');
  }
  return timestamp;
}

function validateIncludedPids(includedPids, rootPid) {
  if (!Array.isArray(includedPids)) {
    throw new Error('性能证据 include PID 元数据无效');
  }
  const unique = new Set();
  for (const pid of includedPids) {
    if (
      !Number.isSafeInteger(pid) ||
      pid <= 0 ||
      pid === rootPid ||
      unique.has(pid)
    ) {
      throw new Error('性能证据 include PID 元数据无效');
    }
    unique.add(pid);
  }
  return unique;
}

function validateProcessTree(processes, rootPid, includedPids) {
  const selectedPids = new Set([rootPid, ...includedPids]);
  let changed = true;
  while (changed) {
    changed = false;
    for (const process of processes) {
      if (selectedPids.has(process.ppid) && !selectedPids.has(process.pid)) {
        selectedPids.add(process.pid);
        changed = true;
      }
    }
  }
  if (processes.some(process => !selectedPids.has(process.pid))) {
    throw new Error('性能证据混入了未声明的进程');
  }
}

export function verifyPerformanceEvidence(evidence) {
  if (!evidence || !evidenceSchemaVersions.has(evidence.schemaVersion)) {
    throw new Error('不支持的性能证据格式');
  }
  if (!Array.isArray(evidence.rawSamples) || evidence.rawSamples.length === 0) {
    throw new Error('性能证据没有逐样本数据');
  }
  const measurement = evidence.measurement;
  if (
    !measurement ||
    !Number.isSafeInteger(measurement.rootPid) ||
    measurement.rootPid <= 0 ||
    !Number.isSafeInteger(measurement.sampleCount) ||
    measurement.sampleCount <= 0 ||
    !Number.isFinite(measurement.durationSeconds) ||
    measurement.durationSeconds <= 0 ||
    !Number.isFinite(measurement.intervalSeconds) ||
    measurement.intervalSeconds <= 0
  ) {
    throw new Error('性能证据测量元数据无效');
  }
  const includedPids = validateIncludedPids(
    measurement.includedPids,
    measurement.rootPid
  );
  const usesProcessBirthTokens = evidence.schemaVersion >= 3;
  const rootProcessStartedAt = usesProcessBirthTokens
    ? parseCanonicalTimestamp(measurement.rootProcessStartedAt)
    : null;
  if (
    measurement.rootExecutableRealpath !== evidence.executable.realpath ||
    measurement.rootExecutableSha256 !== evidence.executable.sha256
  ) {
    throw new Error('性能证据根进程与 installed executable 未绑定');
  }
  if (evidence.rawSamples.length !== measurement.sampleCount) {
    throw new Error('性能证据样本数与测量元数据不一致');
  }
  const expectedSamples = Math.max(
    1,
    Math.ceil(measurement.durationSeconds / measurement.intervalSeconds)
  );
  if (expectedSamples !== evidence.rawSamples.length) {
    throw new Error('性能证据采样次数与 duration/interval 不一致');
  }

  const sampleTimestamps = [];
  for (let index = 0; index < evidence.rawSamples.length; index += 1) {
    const sample = evidence.rawSamples[index];
    const current = parseCanonicalTimestamp(sample?.at);
    sampleTimestamps.push(current);
    if (index > 0) {
      const previous = sampleTimestamps[index - 1];
      const minimumGap = measurement.intervalSeconds * 1_000 * 0.9;
      const maximumGap = measurement.intervalSeconds * 1_000 * 2.5;
      if (current - previous < minimumGap || current - previous > maximumGap) {
        throw new Error('性能证据采样间隔无效');
      }
    }
  }
  const observedIncludedPids = new Set();
  const includedProcessBirthTokens = new Map();
  for (const sample of evidence.rawSamples) {
    if (
      !Number.isFinite(sample.rssMiB) ||
      sample.rssMiB <= 0 ||
      !Number.isFinite(sample.cpuPercent) ||
      sample.cpuPercent < 0 ||
      !Array.isArray(sample.processes) ||
      sample.processes.length === 0
    ) {
      throw new Error('性能证据逐样本结构无效');
    }
    const seenPids = new Set();
    for (const process of sample.processes) {
      if (
        !Number.isSafeInteger(process.pid) ||
        process.pid <= 0 ||
        !Number.isSafeInteger(process.ppid) ||
        process.ppid < 0 ||
        !Number.isSafeInteger(process.rssKiB) ||
        process.rssKiB <= 0 ||
        !Number.isFinite(process.cpuPercent) ||
        process.cpuPercent < 0 ||
        !processRoles.has(process.role) ||
        (usesProcessBirthTokens &&
          (parseCanonicalTimestamp(process.startedAt) <= 0 ||
            !/^[a-f0-9]{64}$/.test(process.execTokenHash))) ||
        seenPids.has(process.pid)
      ) {
        throw new Error('性能证据逐进程结构无效');
      }
      seenPids.add(process.pid);
      if (includedPids.has(process.pid)) {
        observedIncludedPids.add(process.pid);
        if (usesProcessBirthTokens) {
          const births =
            includedProcessBirthTokens.get(process.pid) ?? new Set();
          births.add(process.startedAt);
          includedProcessBirthTokens.set(process.pid, births);
        }
      }
    }
    const root = sample.processes.find(
      process => process.pid === measurement.rootPid
    );
    if (!root || root.role !== 'host') {
      throw new Error('性能证据未包含声明的 host 根进程');
    }
    if (
      usesProcessBirthTokens &&
      Date.parse(root.startedAt) !== rootProcessStartedAt
    ) {
      throw new Error('性能证据根进程 birth token 发生变化');
    }
    validateProcessTree(sample.processes, measurement.rootPid, includedPids);
    const processRssMiB = sample.processes.reduce(
      (total, process) => total + process.rssKiB / 1024,
      0
    );
    const processCpu = sample.processes.reduce(
      (total, process) => total + process.cpuPercent,
      0
    );
    if (processRssMiB !== sample.rssMiB || processCpu !== sample.cpuPercent) {
      throw new Error('性能证据总量与逐进程数据不一致');
    }
  }
  if ([...includedPids].some(pid => !observedIncludedPids.has(pid))) {
    throw new Error('性能证据没有采样到声明的 include PID');
  }
  if (
    usesProcessBirthTokens &&
    [...includedProcessBirthTokens.values()].some(births => births.size !== 1)
  ) {
    throw new Error('性能证据 include PID birth token 发生变化');
  }

  const recalculated = summarizeSamples(evidence.rawSamples);
  if (!isDeepStrictEqual(recalculated, evidence.summary)) {
    throw new Error('性能摘要与逐样本数据不一致');
  }
  const processSummaries = summarizeSanitizedProcesses(evidence.rawSamples);
  if (!isDeepStrictEqual(processSummaries, evidence.processSummaries)) {
    throw new Error('逐进程摘要与逐样本数据不一致');
  }
  return recalculated;
}
