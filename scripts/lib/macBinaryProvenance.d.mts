export const RUST_SIDECAR_MARKER: 'YPM_RUST_SIDECAR_V1';
export function assertRustSidecarMarker(bytes: Uint8Array): string;
export function readArm64MachOUuid(bytes: Uint8Array): string;
