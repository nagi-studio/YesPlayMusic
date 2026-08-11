import type { ProcessTableEntry } from './lib/processMetrics.mjs';

export interface OwnedSidecarOptions {
  hostPid: number;
  sidecarExecutable: string;
  requireDirectParent?: boolean;
}

export function commandMatchesExecutable(
  command: string,
  executable: string
): boolean;

export function findOwnedSidecarProcesses(
  processes: readonly ProcessTableEntry[],
  options: OwnedSidecarOptions
): ProcessTableEntry[];

export function localRuntimeReady(baseUrl?: string): Promise<boolean>;
