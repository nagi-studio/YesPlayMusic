export interface CargoPackageMetadata {
  id: string;
  name: string;
  version: string;
  license: string | null;
  license_file?: string | null;
  authors?: string[];
  repository?: string | null;
  manifest_path: string;
  rust_version?: string | null;
  source?: string | null;
}

export interface CargoMetadata {
  packages: CargoPackageMetadata[];
  resolve: {
    nodes: Array<{
      id: string;
      deps: Array<{
        pkg: string;
        dep_kinds?: Array<{ kind: 'dev' | 'build' | null }>;
      }>;
    }>;
  };
}

export interface ComplianceBuildOptions {
  projectRoot?: string;
  outputDirectory?: string;
  completeSourceDirectory?: string;
  metadata?: CargoMetadata;
  binaryProvenance?: {
    targetTriple: string;
    fileName: string;
    sha256: string;
    rustMarker: 'YPM_RUST_SIDECAR_V1';
    machOUuid: string | null;
  };
  skipOfflineRebuild?: boolean;
  noticesOnly?: boolean;
}

export const defaultProjectRoot: string;
export const defaultComplianceOutput: string;
export const defaultCompleteSourceOutput: string;
export const EXPECTED_UNM_CRATES: readonly string[];

export function sidecarSourceArchiveName(version: string): string;
export function sidecarSourceOfferName(version: string): string;

export function buildSidecarCompliance(
  options?: ComplianceBuildOptions
): Promise<{
  outputDirectory: string;
  completeSourceDirectory: string | null;
  dependencyCount: number;
  copyleftSourceCount: number;
}>;
