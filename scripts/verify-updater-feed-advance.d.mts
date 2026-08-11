export interface CanaryUpdaterManifestVersion {
  version: string;
  [key: string]: unknown;
}

export function compareCanaryUpdaterVersions(
  left: string,
  right: string
): -1 | 0 | 1;

export function verifyCanaryUpdaterFeedAdvance(
  current: CanaryUpdaterManifestVersion,
  next: CanaryUpdaterManifestVersion
): true;
