export function validateUpdaterPublicKey(publicKey: string): true;

export function verifyUpdaterArtifactSignature(
  artifactPath: string,
  signaturePath: string,
  publicKey: string
): Promise<true>;
