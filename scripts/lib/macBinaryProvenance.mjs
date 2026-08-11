const MH_MAGIC_64 = 0xfeedfacf;
const CPU_TYPE_ARM64 = 0x0100000c;
const LC_UUID = 0x1b;
const MACH_HEADER_64_SIZE = 32;

export const RUST_SIDECAR_MARKER = 'YPM_RUST_SIDECAR_V1';

function containsAscii(bytes, text) {
  const needle = new TextEncoder().encode(text);
  outer: for (
    let offset = 0;
    offset <= bytes.length - needle.length;
    offset++
  ) {
    for (let index = 0; index < needle.length; index++) {
      if (bytes[offset + index] !== needle[index]) continue outer;
    }
    return true;
  }
  return false;
}

export function assertRustSidecarMarker(bytes) {
  if (!containsAscii(bytes, RUST_SIDECAR_MARKER)) {
    throw new Error(
      `Sidecar 缺少 Rust production marker：${RUST_SIDECAR_MARKER}`
    );
  }
  return RUST_SIDECAR_MARKER;
}

function requireBytes(view, offset, size, label) {
  if (offset < 0 || size < 0 || offset + size > view.byteLength) {
    throw new Error(`macOS Sidecar 缺少 ${label}`);
  }
}

function formatUuid(bytes) {
  const hex = [...bytes]
    .map(value => value.toString(16).padStart(2, '0'))
    .join('')
    .toUpperCase();
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(
    12,
    16
  )}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

/**
 * Read the LC_UUID from the thin arm64 Mach-O shipped as the macOS Sidecar.
 * Code signing changes the signature blob but preserves this build UUID.
 * @param {Uint8Array} bytes
 */
export function readArm64MachOUuid(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  requireBytes(view, 0, MACH_HEADER_64_SIZE, '64-bit Mach-O header');
  if (view.getUint32(0, true) !== MH_MAGIC_64) {
    throw new Error('macOS Sidecar 必须是 thin 64-bit Mach-O');
  }
  if (view.getUint32(4, true) !== CPU_TYPE_ARM64) {
    throw new Error('macOS Sidecar 必须是 arm64 Mach-O');
  }

  const commandCount = view.getUint32(16, true);
  const commandBytes = view.getUint32(20, true);
  requireBytes(view, MACH_HEADER_64_SIZE, commandBytes, 'Mach-O load commands');

  let offset = MACH_HEADER_64_SIZE;
  for (let index = 0; index < commandCount; index += 1) {
    requireBytes(view, offset, 8, 'Mach-O load command');
    const command = view.getUint32(offset, true);
    const commandSize = view.getUint32(offset + 4, true);
    if (commandSize < 8) {
      throw new Error('macOS Sidecar 的 Mach-O load command 长度无效');
    }
    requireBytes(view, offset, commandSize, 'Mach-O load command payload');
    if (command === LC_UUID) {
      if (commandSize < 24) {
        throw new Error('macOS Sidecar 的 LC_UUID 长度无效');
      }
      return formatUuid(bytes.subarray(offset + 8, offset + 24));
    }
    offset += commandSize;
  }

  throw new Error('macOS Sidecar 缺少 LC_UUID provenance');
}
