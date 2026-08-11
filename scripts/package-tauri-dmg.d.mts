export interface PackageTauriDmgOptions {
  appPath?: string;
  outputDir?: string;
  completeSourceDirectory?: string;
}

export interface CollectTauriReleaseDmgOptions extends PackageTauriDmgOptions {
  sourcePath?: string;
}

export interface TauriDmgResult {
  dmgPath: string;
  checksumPath: string;
  appAllocatedBytes: number;
  sourceArchivePath: string;
  sourceChecksumPath: string;
  sourceArchiveBytes: number;
  sourceDependencyCount: number;
  sourceOfferPath: string;
  sourceOfferChecksumPath: string;
}

export const defaultTauriAppPath: string;
export const defaultBuiltSidecarPath: string;
export const RUST_SIDECAR_APP_SIZE_LIMIT_BYTES: number;

export function assertTauriAppSize(
  allocatedBytes: number,
  limitBytes?: number
): number;

export function tauriDmgName(version: string): string;

export function tauriBundledDmgPath(version: string): string;

export function assertMacBundleProvenance(
  appPath: string,
  expectedVersion: string,
  builtSidecarPath?: string,
  expectedAppComplianceDirectory?: string
): Promise<{
  sidecarUuid: string;
  builtSidecarSha256: string;
  sourceArchiveName: string;
  dependencyCount: number;
}>;

export function packageTauriDmg(
  options?: PackageTauriDmgOptions
): Promise<TauriDmgResult>;

export function collectTauriReleaseDmg(
  options?: CollectTauriReleaseDmgOptions
): Promise<TauriDmgResult>;
