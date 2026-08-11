import { afterEach, describe, expect, test } from 'bun:test';
import {
  checkForAppUpdate,
  checkForAppUpdateInBackground,
  clearPendingAppUpdate,
  installPendingAppUpdate,
} from '../src/services/appUpdater';
import type { AppUpdaterBindings } from '../src/services/appUpdater';

afterEach(async () => {
  await clearPendingAppUpdate();
});

describe('Tauri updater flow', () => {
  test('reports an unconfigured development build without checking the endpoint', async () => {
    let checked = false;
    const bindings: AppUpdaterBindings = {
      isConfigured: async () => false,
      check: async () => {
        checked = true;
        return null;
      },
      prepareForInstall: async () => false,
      relaunch: async () => {},
    };

    expect(await checkForAppUpdate(bindings)).toEqual({
      status: 'unconfigured',
    });
    expect(checked).toBe(false);
  });

  test('checks, downloads, installs, and relaunches a signed update', async () => {
    let relaunched = false;
    let closed = false;
    const installOrder: string[] = [];
    const bindings: AppUpdaterBindings = {
      isConfigured: async () => true,
      check: async () => ({
        version: '0.7.0',
        body: 'Release notes',
        date: '2026-08-10T00:00:00Z',
        async download(onEvent) {
          installOrder.push('download');
          onEvent?.({ event: 'Started', data: { contentLength: 10 } });
          onEvent?.({ event: 'Progress', data: { chunkLength: 4 } });
          onEvent?.({ event: 'Progress', data: { chunkLength: 6 } });
          onEvent?.({ event: 'Finished' });
        },
        async install() {
          installOrder.push('install');
        },
        async close() {
          closed = true;
        },
      }),
      prepareForInstall: async () => {
        installOrder.push('prepare');
        return true;
      },
      relaunch: async () => {
        installOrder.push('relaunch');
        relaunched = true;
      },
    };

    expect(await checkForAppUpdate(bindings)).toEqual({
      status: 'available',
      version: '0.7.0',
      notes: 'Release notes',
      date: '2026-08-10T00:00:00Z',
    });

    const progress: Array<number | null> = [];
    await installPendingAppUpdate(
      state => progress.push(state.percent),
      bindings
    );
    expect(progress).toEqual([0, 40, 100, 100]);
    expect(installOrder).toEqual([
      'download',
      'prepare',
      'install',
      'relaunch',
    ]);
    expect(relaunched).toBe(true);

    await clearPendingAppUpdate();
    expect(closed).toBe(true);
  });

  test('background checks suppress network errors', async () => {
    const errors: unknown[] = [];
    const result = await checkForAppUpdateInBackground(
      {
        isConfigured: async () => true,
        check: async () => {
          throw new Error('offline');
        },
        prepareForInstall: async () => false,
        relaunch: async () => {},
      },
      error => errors.push(error)
    );
    expect(result).toBeNull();
    expect(errors).toHaveLength(1);
  });

  test('startup and manual checks share one updater request', async () => {
    let checks = 0;
    let finishCheck: () => void = () => {};
    const waitForCheck = new Promise<void>(resolve => {
      finishCheck = resolve;
    });
    const bindings: AppUpdaterBindings = {
      isConfigured: async () => true,
      check: async () => {
        checks += 1;
        await waitForCheck;
        return null;
      },
      prepareForInstall: async () => false,
      relaunch: async () => {},
    };

    const startup = checkForAppUpdateInBackground(bindings);
    const manual = checkForAppUpdate(bindings);
    finishCheck();

    expect(await Promise.all([startup, manual])).toEqual([
      { status: 'up-to-date' },
      { status: 'up-to-date' },
    ]);
    expect(checks).toBe(1);
  });

  test('relaunches the current build if Windows installer startup fails after shutdown', async () => {
    let relaunched = false;
    const bindings: AppUpdaterBindings = {
      isConfigured: async () => true,
      check: async () => ({
        version: '0.7.2',
        async download() {},
        async install() {
          throw new Error('installer failed');
        },
        async close() {},
      }),
      prepareForInstall: async () => true,
      relaunch: async () => {
        relaunched = true;
      },
    };

    await checkForAppUpdate(bindings);
    await expect(installPendingAppUpdate(() => {}, bindings)).rejects.toThrow(
      'installer failed'
    );
    expect(relaunched).toBe(true);
  });

  test('relaunches the current build if Windows sidecar preparation rejects', async () => {
    let installed = false;
    let relaunched = false;
    const bindings: AppUpdaterBindings = {
      isConfigured: async () => true,
      check: async () => ({
        version: '0.7.2',
        async download() {},
        async install() {
          installed = true;
        },
        async close() {},
      }),
      prepareForInstall: async () => {
        throw new Error('sidecar termination was not confirmed');
      },
      relaunch: async () => {
        relaunched = true;
      },
    };

    await checkForAppUpdate(bindings);
    await expect(installPendingAppUpdate(() => {}, bindings)).rejects.toThrow(
      'sidecar termination was not confirmed'
    );
    expect(installed).toBe(false);
    expect(relaunched).toBe(true);
  });
});
