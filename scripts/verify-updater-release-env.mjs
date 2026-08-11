import { validateUpdaterPublicKey } from './verify-updater-signature.mjs';

export const REQUIRED_UPDATER_RELEASE_ENV = [
  'TAURI_SIGNING_PRIVATE_KEY',
  'TAURI_SIGNING_PRIVATE_KEY_PASSWORD',
  'TAURI_UPDATER_PUBKEY',
];

export function verifyUpdaterReleaseEnvironment(environment = process.env) {
  const missing = REQUIRED_UPDATER_RELEASE_ENV.filter(
    name => typeof environment[name] !== 'string' || !environment[name].trim()
  );
  if (missing.length) {
    throw new Error(
      `Missing updater release configuration: ${missing.join(', ')}`
    );
  }
  validateUpdaterPublicKey(environment.TAURI_UPDATER_PUBKEY);
  return true;
}

if (import.meta.main) {
  try {
    verifyUpdaterReleaseEnvironment();
    console.log('[tauri-updater] signing configuration verified');
  } catch (error) {
    console.error(`[tauri-updater] ${error.message}`);
    process.exit(1);
  }
}
