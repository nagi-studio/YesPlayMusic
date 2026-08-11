export interface TauriSmokeExecutableOptions {
  platform?: NodeJS.Platform;
  arch?: string;
  root?: string;
  executablePath?: string;
}

export function resolveTauriSmokeExecutable(
  options?: TauriSmokeExecutableOptions
): string;

export function tauriSmokeExitTimeoutMs(includeWebview: boolean): number;
