export interface UpdaterBuildPlan {
  targetTriple: string;
  args: string[];
  afterBuild: string[][];
}

export const UPDATER_BUILD_PLANS: Readonly<Record<string, UpdaterBuildPlan>>;
export const CANARY_UPDATER_ENDPOINT: string;

export function createUpdaterBuildConfig(
  publicKey: string,
  version?: string
): Promise<Record<string, unknown>>;

export function updaterBuildArgs(
  target: string,
  options?: { developerId?: boolean }
): string[];

export function buildTauriUpdater(
  target: string,
  options?: { developerId?: boolean }
): Promise<UpdaterBuildPlan>;
