export const UPDATER_ARTIFACT_VERSION_RESOURCE: string;

export function findSquashfsOffset(buffer: Buffer): number;

export function readUpdaterArtifactVersion(
  target: string,
  artifactPath: string,
  execute?: (command: string, args: string[]) => void
): Promise<string>;

export function withExtractedUpdaterArtifact<T>(
  target: string,
  artifactPath: string,
  inspect: (directory: string) => Promise<T>,
  execute?: (command: string, args: string[]) => void,
  options?: { expandWindowsNested?: boolean }
): Promise<T>;
