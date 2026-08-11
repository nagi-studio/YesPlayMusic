export interface UpdaterArtifactSpec {
  targetTriple: string;
  bundleDir: string;
  suffix: string;
}

export const UPDATER_ARTIFACT_SPECS: Readonly<
  Record<string, UpdaterArtifactSpec>
>;

export function collectUpdaterArtifacts(
  target: string,
  outputDir: string,
  root?: string,
  publicKey?: string
): Promise<{
  artifactName: string;
  artifactPath: string;
  signaturePath: string;
}>;
