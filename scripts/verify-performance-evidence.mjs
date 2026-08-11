#!/usr/bin/env bun
import { existsSync, realpathSync } from 'node:fs';
import { verifyPerformanceEvidence } from './lib/processMetrics.mjs';

async function sha256(filePath) {
  const bytes = await Bun.file(filePath).arrayBuffer();
  return new Bun.CryptoHasher('sha256').update(bytes).digest('hex');
}

function parseArgs(args) {
  const evidencePath = args[0];
  if (!evidencePath || evidencePath.startsWith('--')) {
    throw new Error(
      '用法：verify-performance-evidence.mjs <evidence.json> [--artifact <path>] [--executable <path>]'
    );
  }
  const options = { evidencePath, artifactPath: null, executablePath: null };
  for (let index = 1; index < args.length; index += 2) {
    const flag = args[index];
    const value = args[index + 1];
    if (!value) throw new Error(`${flag} 缺少参数`);
    if (flag === '--artifact') options.artifactPath = value;
    else if (flag === '--executable') options.executablePath = value;
    else throw new Error(`未知参数：${flag}`);
  }
  return options;
}

async function verifyFileIdentity(label, identity, overridePath) {
  const candidate = overridePath ?? identity?.realpath;
  if (!candidate || !existsSync(candidate)) {
    throw new Error(
      `${label} 文件不存在，无法复核 hash：${candidate ?? 'missing'}`
    );
  }
  const actualPath = realpathSync(candidate);
  const actualFile = Bun.file(actualPath);
  if (actualFile.size !== identity.bytes) {
    throw new Error(`${label} 字节数不匹配`);
  }
  if ((await sha256(actualPath)) !== identity.sha256) {
    throw new Error(`${label} SHA-256 不匹配`);
  }
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const evidence = await Bun.file(options.evidencePath).json();
  const summary = verifyPerformanceEvidence(evidence);
  if (
    evidence.schemaVersion >= 4 &&
    (!options.artifactPath || !options.executablePath)
  ) {
    throw new Error(
      'schema v4 必须通过 --artifact 与 --executable 提供待复核文件'
    );
  }
  await verifyFileIdentity('artifact', evidence.artifact, options.artifactPath);
  await verifyFileIdentity(
    'executable',
    evidence.executable,
    options.executablePath
  );
  console.log(
    `[performance] verified ${summary.samples} samples, artifact=${evidence.artifact.sha256}`
  );
}

main().catch(error => {
  console.error(`[performance] ${error.message}`);
  process.exit(1);
});
