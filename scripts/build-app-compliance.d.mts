export interface CargoPackageMetadata {
  id: string;
  name: string;
  version?: string;
  license?: string | null;
  license_file?: string | null;
  authors?: string[];
  repository?: string | null;
  manifest_path?: string;
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

export const defaultAppComplianceOutput: string;
export const defaultRendererManifest: string;

export function collectCargoRuntimePackages(
  metadata: CargoMetadata,
  rootName: string,
  allowedCoordinates?: Set<string>
): CargoPackageMetadata[];

export function collectRendererPackageJsonPaths(
  moduleIds: string[],
  projectRoot?: string
): string[];

export function rendererDependencyManifestPlugin(options?: {
  projectRoot?: string;
  outputPath?: string;
}): {
  name: string;
  generateBundle(
    outputOptions: unknown,
    bundle: Record<string, { type: string; modules?: Record<string, unknown> }>
  ): void;
};

export function buildAppCompliance(options?: {
  projectRoot?: string;
  outputDirectory?: string;
  rendererManifestPath?: string;
  cargoMetadata?: CargoMetadata;
  cargoTreeCoordinates?: Set<string>;
  targetTriple?: string;
}): Promise<{ hostDependencyCount: number; rendererPackageCount: number }>;

export function verifyAppComplianceDirectory(
  directory: string,
  options?: {
    targetTriple?: string;
    requiredHostPackages?: string[];
    requiredRendererPackages?: string[];
  }
): Promise<unknown>;

export function assertAppComplianceMatchesDirectory(
  actual: string,
  expected: string
): Promise<void>;
