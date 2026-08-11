export interface ProcessMetricsOptions {
  pid: number;
  includePids: number[];
  durationSeconds: number;
  intervalSeconds: number;
  label: string;
  evidencePath: string | null;
  artifactPath: string | null;
  executablePath: string | null;
}

export interface ProcessTableEntry {
  pid: number;
  ppid: number;
  startedAt?: string;
  rssKiB: number;
  cpuPercent: number;
  command: string;
}

export interface ProcessSample {
  rssMiB: number;
  cpuPercent: number;
}

export interface ProcessTreeSample extends ProcessSample {
  processes: readonly ProcessTableEntry[];
}

export interface MetricSummary {
  mean: number;
  p95: number;
  max: number;
}

export interface ProcessSamplesSummary {
  samples: number;
  rssMiB: MetricSummary;
  cpuPercent: MetricSummary;
}

export interface PerProcessSummary extends ProcessSamplesSummary {
  pid: number;
  ppid?: number;
  ppids?: number[];
  startedAt?: string;
  command: string;
}

export interface SanitizedPerProcessSummary extends ProcessSamplesSummary {
  pid: number;
  ppid?: number;
  ppids?: number[];
  startedAt?: string;
  execTokenHash?: string;
  role: string;
}

export function parseMetricsArgs(
  args: readonly string[]
): ProcessMetricsOptions;

export function parseProcessTable(text: string): ProcessTableEntry[];

export function collectProcessTree(
  processes: readonly ProcessTableEntry[],
  rootPid: number,
  includePids?: readonly number[]
): ProcessTableEntry[];

export function summarizeSamples(
  samples: readonly ProcessSample[]
): ProcessSamplesSummary;

export function summarizeProcesses(
  samples: readonly ProcessTreeSample[]
): PerProcessSummary[];

export function processRole(command: string): string;

export interface SanitizedProcessEntry {
  pid: number;
  ppid: number;
  startedAt?: string;
  execTokenHash?: string;
  rssKiB: number;
  cpuPercent: number;
  role: string;
}

export interface SanitizedProcessTreeSample extends ProcessSample {
  at: string;
  processes: readonly SanitizedProcessEntry[];
}

export function sanitizeSamples(
  samples: readonly (ProcessTreeSample & { at: string })[]
): SanitizedProcessTreeSample[];

export function summarizeSanitizedProcesses(
  samples: readonly SanitizedProcessTreeSample[]
): SanitizedPerProcessSummary[];

export function verifyPerformanceEvidence(
  evidence: unknown
): ProcessSamplesSummary;
