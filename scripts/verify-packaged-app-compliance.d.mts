export function verifyPackagedAppCompliance(
  target: string,
  artifactPath: string,
  expectedDirectory?: string,
  execute?: (command: string, args: string[]) => void
): Promise<string>;
