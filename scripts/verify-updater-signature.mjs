import {
  createHash,
  createPublicKey,
  verify as verifyEd25519,
} from 'node:crypto';
import { createReadStream } from 'node:fs';
import { readFile } from 'node:fs/promises';

const ED25519_SPKI_PREFIX = Buffer.from('302a300506032b6570032100', 'hex');
const TRUSTED_COMMENT_PREFIX = 'trusted comment: ';

function decodeBase64(value, label) {
  if (typeof value !== 'string') throw new Error(`Missing ${label}`);
  const normalized = value.trim();
  if (
    !normalized ||
    normalized.length % 4 !== 0 ||
    !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(
      normalized
    )
  ) {
    throw new Error(`Invalid ${label}: expected Base64`);
  }
  const decoded = Buffer.from(normalized, 'base64');
  if (decoded.toString('base64') !== normalized) {
    throw new Error(`Invalid ${label}: non-canonical Base64`);
  }
  return decoded;
}

function decodeUtf8(value, label) {
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(value);
  } catch {
    throw new Error(`Invalid ${label}: expected UTF-8`);
  }
}

function parsePublicKey(encodedPublicKey) {
  const text = decodeUtf8(
    decodeBase64(encodedPublicKey, 'updater public key'),
    'updater public key'
  );
  const lines = text.trimEnd().split(/\r?\n/);
  if (lines.length !== 2 || !lines[0]?.startsWith('untrusted comment:')) {
    throw new Error('Invalid updater public key: expected Minisign key');
  }
  const payload = decodeBase64(lines[1], 'Minisign public key');
  if (payload.length !== 42) {
    throw new Error('Invalid updater public key: expected 42-byte payload');
  }
  const algorithm = payload.subarray(0, 2).toString('ascii');
  if (algorithm !== 'Ed' && algorithm !== 'ED') {
    throw new Error('Invalid updater public key: unsupported algorithm');
  }
  return {
    keyId: payload.subarray(2, 10),
    key: createPublicKey({
      key: Buffer.concat([ED25519_SPKI_PREFIX, payload.subarray(10, 42)]),
      format: 'der',
      type: 'spki',
    }),
  };
}

function parseSignature(encodedSignature) {
  const text = decodeUtf8(
    decodeBase64(encodedSignature, 'updater signature'),
    'updater signature'
  );
  const lines = text.trimEnd().split(/\r?\n/);
  if (
    lines.length !== 4 ||
    !lines[0]?.startsWith('untrusted comment:') ||
    !lines[2]?.startsWith(TRUSTED_COMMENT_PREFIX)
  ) {
    throw new Error('Invalid updater signature: expected Minisign signature');
  }
  const signaturePayload = decodeBase64(lines[1], 'Minisign signature payload');
  const globalSignature = decodeBase64(lines[3], 'Minisign global signature');
  if (signaturePayload.length !== 74 || globalSignature.length !== 64) {
    throw new Error('Invalid updater signature: unexpected payload size');
  }
  const algorithm = signaturePayload.subarray(0, 2).toString('ascii');
  if (algorithm !== 'Ed' && algorithm !== 'ED') {
    throw new Error('Invalid updater signature: unsupported algorithm');
  }
  return {
    isPrehashed: algorithm === 'ED',
    keyId: signaturePayload.subarray(2, 10),
    signature: signaturePayload.subarray(10, 74),
    trustedComment: lines[2].slice(TRUSTED_COMMENT_PREFIX.length),
    globalSignature,
  };
}

async function hashArtifact(artifactPath) {
  const hash = createHash('blake2b512');
  for await (const chunk of createReadStream(artifactPath)) hash.update(chunk);
  return hash.digest();
}

export function validateUpdaterPublicKey(publicKey) {
  parsePublicKey(publicKey);
  return true;
}

export async function verifyUpdaterArtifactSignature(
  artifactPath,
  signaturePath,
  publicKey
) {
  const parsedPublicKey = parsePublicKey(publicKey);
  const parsedSignature = parseSignature(await readFile(signaturePath, 'utf8'));
  if (!parsedPublicKey.keyId.equals(parsedSignature.keyId)) {
    throw new Error(
      `Updater signature key ID does not match public key: ${artifactPath}`
    );
  }

  const artifact = parsedSignature.isPrehashed
    ? await hashArtifact(artifactPath)
    : await readFile(artifactPath);
  if (
    !verifyEd25519(
      null,
      artifact,
      parsedPublicKey.key,
      parsedSignature.signature
    )
  ) {
    throw new Error(
      `Updater artifact signature verification failed: ${artifactPath}`
    );
  }

  const globalPayload = Buffer.concat([
    parsedSignature.signature,
    Buffer.from(parsedSignature.trustedComment, 'utf8'),
  ]);
  if (
    !verifyEd25519(
      null,
      globalPayload,
      parsedPublicKey.key,
      parsedSignature.globalSignature
    )
  ) {
    throw new Error(
      `Updater global signature verification failed: ${artifactPath}`
    );
  }
  return true;
}
