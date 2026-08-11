export type SidecarTargetTriple =
  | 'aarch64-apple-darwin'
  | 'x86_64-pc-windows-msvc'
  | 'x86_64-unknown-linux-gnu';

export type RustSidecarProfile = 'debug' | 'release';

export interface SidecarTarget {
  extension: string;
}

export interface HostTargetOptions {
  platform?: NodeJS.Platform;
  arch?: string;
}

export interface RustSidecarBuildOptions {
  targetTriple?: string;
  profile?: RustSidecarProfile;
}

export interface RustSidecarBuildPlan {
  targetTriple: SidecarTargetTriple;
  profile: RustSidecarProfile;
  outputName: string;
  outputPath: string;
  artifactPath: string;
  args: string[];
}

export interface InstallRustSidecarArtifactOptions {
  artifactPath: string;
  outputPath: string;
}

export const SIDECAR_TARGETS: Readonly<
  Record<SidecarTargetTriple, Readonly<SidecarTarget>>
>;

export const WINDOWS_GUI_SUBSYSTEM: 2;

export function parseWindowsPeSubsystem(bytes: Uint8Array): number;

export function assertWindowsGuiPe(bytes: Uint8Array): void;

export function shouldAssertWindowsGuiPe(options: {
  targetTriple: SidecarTargetTriple;
  profile: RustSidecarProfile;
}): boolean;

export function hostTargetTriple(
  options?: HostTargetOptions
): SidecarTargetTriple;

export function rustSidecarBuildPlan(
  options?: RustSidecarBuildOptions
): RustSidecarBuildPlan;

export function installRustSidecarArtifact(
  options: InstallRustSidecarArtifactOptions
): void;

export function buildRustSidecar(
  options?: RustSidecarBuildOptions
): RustSidecarBuildPlan;
