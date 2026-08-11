import { invoke } from '@tauri-apps/api/core';
import type { DownloadEvent } from '@tauri-apps/plugin-updater';
import { isDesktopRuntime } from '@/utils/runtime';

export type AppUpdateCheckResult =
  | { status: 'unconfigured' }
  | { status: 'up-to-date' }
  | {
      status: 'available';
      version: string;
      notes: string;
      date: string | null;
    };

export interface AppUpdateProgress {
  downloadedBytes: number;
  totalBytes: number | null;
  percent: number | null;
}

interface UpdateCandidate {
  version: string;
  body?: string;
  date?: string;
  download(onEvent?: (event: DownloadEvent) => void): Promise<void>;
  install(): Promise<void>;
  close(): Promise<void>;
}

export interface AppUpdaterBindings {
  isConfigured(): Promise<boolean>;
  check(): Promise<UpdateCandidate | null>;
  prepareForInstall(): Promise<boolean>;
  relaunch(): Promise<void>;
}

let pendingUpdate: UpdateCandidate | null = null;
let checkInFlight: Promise<AppUpdateCheckResult> | null = null;

async function defaultBindings(): Promise<AppUpdaterBindings> {
  const [{ check }, { relaunch }] = await Promise.all([
    import('@tauri-apps/plugin-updater'),
    import('@tauri-apps/plugin-process'),
  ]);
  return {
    isConfigured: () => invoke<boolean>('updater_configured'),
    check,
    prepareForInstall: () => invoke<boolean>('prepare_for_update'),
    relaunch,
  };
}

async function releasePendingUpdate(): Promise<void> {
  const update = pendingUpdate;
  pendingUpdate = null;
  if (update) await update.close();
}

async function performAppUpdateCheck(
  bindings?: AppUpdaterBindings
): Promise<AppUpdateCheckResult> {
  if (!isDesktopRuntime && !bindings) return { status: 'unconfigured' };
  const runtime = bindings ?? (await defaultBindings());
  await releasePendingUpdate();
  if (!(await runtime.isConfigured())) return { status: 'unconfigured' };

  const update = await runtime.check();
  if (!update) return { status: 'up-to-date' };
  pendingUpdate = update;
  return {
    status: 'available',
    version: update.version,
    notes: update.body?.trim() ?? '',
    date: update.date ?? null,
  };
}

export async function checkForAppUpdate(
  bindings?: AppUpdaterBindings
): Promise<AppUpdateCheckResult> {
  if (checkInFlight) return checkInFlight;
  const check = performAppUpdateCheck(bindings);
  checkInFlight = check;
  try {
    return await check;
  } finally {
    if (checkInFlight === check) checkInFlight = null;
  }
}

export async function checkForAppUpdateInBackground(
  bindings?: AppUpdaterBindings,
  reportError: (error: unknown) => void = error => {
    console.warn('[updater] background update check failed', error);
  }
): Promise<AppUpdateCheckResult | null> {
  try {
    return await checkForAppUpdate(bindings);
  } catch (error) {
    // Background checks must not interrupt startup or show an error toast.
    reportError(error);
    return null;
  }
}

export async function installPendingAppUpdate(
  onProgress: (progress: AppUpdateProgress) => void,
  bindings?: AppUpdaterBindings
): Promise<void> {
  const update = pendingUpdate;
  if (!update) throw new Error('No update is ready to install');
  const runtime = bindings ?? (await defaultBindings());
  let downloadedBytes = 0;
  let totalBytes: number | null = null;

  await update.download(event => {
    if (event.event === 'Started') {
      totalBytes = event.data.contentLength ?? null;
      downloadedBytes = 0;
    } else if (event.event === 'Progress') {
      downloadedBytes += event.data.chunkLength;
    }
    onProgress({
      downloadedBytes,
      totalBytes,
      percent:
        totalBytes && totalBytes > 0
          ? Math.min(100, Math.round((downloadedBytes / totalBytes) * 100))
          : null,
    });
  });

  let prepareCompleted = false;
  let sidecarStopped = false;
  try {
    sidecarStopped = await runtime.prepareForInstall();
    prepareCompleted = true;
    await update.install();
  } catch (error) {
    // A rejected preparation may mean Windows stopped the Sidecar but could not
    // confirm it. Relaunch in that uncertain state as well as after install errors.
    if (!prepareCompleted || sidecarStopped) await runtime.relaunch();
    throw error;
  }
  await runtime.relaunch();
}

export async function clearPendingAppUpdate(): Promise<void> {
  await releasePendingUpdate();
}
