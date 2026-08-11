export interface UpdaterManifestOptions {
  artifactsDir: string;
  version: string;
  repository?: string;
  tag?: string;
  publishedAt?: string;
  publicKey?: string;
  artifactVersionReader?: (
    target: string,
    artifactPath: string
  ) => Promise<string>;
}

export interface UpdaterManifest {
  version: string;
  notes: string;
  pub_date: string;
  platforms: Record<string, { signature: string; url: string }>;
}

export const UPDATER_MANIFEST_SUFFIXES: Readonly<Record<string, string>>;

export function createUpdaterManifest(
  options: UpdaterManifestOptions
): Promise<UpdaterManifest>;

export function writeUpdaterManifest(
  options: UpdaterManifestOptions,
  outputPath: string
): Promise<UpdaterManifest>;
