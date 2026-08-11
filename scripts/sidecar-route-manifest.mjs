import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { isDeepStrictEqual } from 'node:util';
import ts from 'typescript';

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..'
);

const REQUIRED_MANIFEST_FIELDS = [
  'id',
  'method',
  'path',
  'requestBuilder',
  'nodeAdapter',
  'rustAdapter',
  'decoder',
  'comparator',
  'idempotent',
];

function normalizeRelativePath(rootDir, fileName) {
  return path.relative(rootDir, fileName).split(path.sep).join('/');
}

function parseSource(fileName, scriptKind = ts.ScriptKind.TS) {
  return ts.createSourceFile(
    fileName,
    readFileSync(fileName, 'utf8'),
    ts.ScriptTarget.Latest,
    true,
    scriptKind
  );
}

function collectTypeScriptFiles(directory) {
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const fileName = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...collectTypeScriptFiles(fileName));
    } else if (
      entry.isFile() &&
      entry.name.endsWith('.ts') &&
      !entry.name.endsWith('.d.ts')
    ) {
      files.push(fileName);
    }
  }
  return files.sort();
}

function unwrapExpression(expression) {
  let current = expression;
  while (
    ts.isParenthesizedExpression(current) ||
    ts.isAsExpression(current) ||
    ts.isTypeAssertionExpression(current) ||
    ts.isNonNullExpression(current) ||
    ts.isSatisfiesExpression(current)
  ) {
    current = current.expression;
  }
  return current;
}

function propertyNameText(name) {
  if (!name) return undefined;
  if (ts.isIdentifier(name) || ts.isStringLiteralLike(name)) return name.text;
  return undefined;
}

function objectPropertyExpression(object, propertyName) {
  for (const property of object.properties) {
    if (propertyNameText(property.name) !== propertyName) continue;
    if (ts.isPropertyAssignment(property)) return property.initializer;
    if (ts.isShorthandPropertyAssignment(property)) return property.name;
  }
  return undefined;
}

function staticString(expression, substitutions = new Map()) {
  if (!expression) return undefined;
  const current = unwrapExpression(expression);
  if (ts.isStringLiteralLike(current)) return current.text;
  if (ts.isIdentifier(current) && substitutions.has(current.text)) {
    return staticString(substitutions.get(current.text), substitutions);
  }
  return undefined;
}

function resolveModuleFile(rootDir, containingFile, moduleName) {
  let candidate;
  if (moduleName.startsWith('@/')) {
    candidate = path.join(rootDir, 'src', moduleName.slice(2));
  } else if (moduleName.startsWith('.')) {
    candidate = path.resolve(path.dirname(containingFile), moduleName);
  } else {
    return undefined;
  }

  return path.extname(candidate) ? candidate : `${candidate}.ts`;
}

function collectImports(rootDir, sourceFile) {
  const imports = new Map();
  for (const statement of sourceFile.statements) {
    if (
      !ts.isImportDeclaration(statement) ||
      !ts.isStringLiteral(statement.moduleSpecifier) ||
      !statement.importClause
    ) {
      continue;
    }
    const moduleName = statement.moduleSpecifier.text;
    const sourcePath = resolveModuleFile(
      rootDir,
      sourceFile.fileName,
      moduleName
    );
    if (statement.importClause.name) {
      imports.set(statement.importClause.name.text, {
        kind: 'default',
        importedName: 'default',
        moduleName,
        sourcePath,
      });
    }
    const bindings = statement.importClause.namedBindings;
    if (bindings && ts.isNamedImports(bindings)) {
      for (const element of bindings.elements) {
        imports.set(element.name.text, {
          kind: 'named',
          importedName: element.propertyName?.text ?? element.name.text,
          moduleName,
          sourcePath,
        });
      }
    }
  }
  return imports;
}

function findFunctionReturnObject(sourceFile, functionName) {
  let declaration;
  for (const statement of sourceFile.statements) {
    if (
      ts.isFunctionDeclaration(statement) &&
      statement.name?.text === functionName
    ) {
      declaration = statement;
      break;
    }
  }
  if (!declaration?.body) {
    throw new Error(
      `无法解析 request helper ${functionName}（${sourceFile.fileName}）`
    );
  }

  let returned;
  function visit(node) {
    if (returned) return;
    if (ts.isReturnStatement(node) && node.expression) {
      const expression = unwrapExpression(node.expression);
      if (ts.isObjectLiteralExpression(expression)) returned = expression;
      return;
    }
    ts.forEachChild(node, visit);
  }
  visit(declaration.body);
  if (!returned) {
    throw new Error(
      `request helper ${functionName} 未返回对象（${sourceFile.fileName}）`
    );
  }
  return { declaration, returned };
}

function requestConfigFromHelper(rootDir, call, imports) {
  const helperName = ts.isIdentifier(call.expression)
    ? call.expression.text
    : undefined;
  const imported = helperName ? imports.get(helperName) : undefined;
  if (!helperName || imported?.kind !== 'named' || !imported.sourcePath) {
    throw new Error(`request 第一个参数不是可解析的对象或具名 helper`);
  }

  const helperSource = parseSource(imported.sourcePath);
  const { declaration, returned } = findFunctionReturnObject(
    helperSource,
    imported.importedName
  );
  const substitutions = new Map();
  declaration.parameters.forEach((parameter, index) => {
    if (ts.isIdentifier(parameter.name) && call.arguments[index]) {
      substitutions.set(parameter.name.text, call.arguments[index]);
    }
  });

  return { object: returned, substitutions };
}

function findExportedRequestBuilder(node) {
  let current = node.parent;
  while (current) {
    if (
      ts.isFunctionDeclaration(current) &&
      current.name &&
      current.modifiers?.some(
        modifier => modifier.kind === ts.SyntaxKind.ExportKeyword
      )
    ) {
      return current.name.text;
    }
    current = current.parent;
  }
  return undefined;
}

function decoderReference(rootDir, sourceFile, expression, imports) {
  const decoder = unwrapExpression(expression);
  if (!ts.isIdentifier(decoder)) {
    throw new Error(
      `decoder 必须是具名标识符（${normalizeRelativePath(
        rootDir,
        sourceFile.fileName
      )}:${
        sourceFile.getLineAndCharacterOfPosition(decoder.getStart()).line + 1
      }）`
    );
  }

  const imported = imports.get(decoder.text);
  const fileName = imported?.sourcePath ?? sourceFile.fileName;
  const symbol = imported?.importedName ?? decoder.text;
  return `${normalizeRelativePath(rootDir, fileName)}#${symbol}`;
}

function extractApiForward(configObject, substitutions) {
  const paramsExpression = objectPropertyExpression(configObject, 'params');
  if (!paramsExpression) return undefined;
  const params = unwrapExpression(paramsExpression);
  if (!ts.isObjectLiteralExpression(params)) return undefined;
  const upstreamPath = staticString(
    objectPropertyExpression(params, 'uri'),
    substitutions
  );
  if (!upstreamPath) return undefined;
  const crypto = staticString(
    objectPropertyExpression(params, 'crypto'),
    substitutions
  );
  return {
    allowedPaths: [upstreamPath],
    ...(crypto === undefined ? {} : { crypto }),
  };
}

function extractRequestCalls(rootDir) {
  const apiDirectory = path.join(rootDir, 'src/api');
  const calls = [];

  for (const fileName of collectTypeScriptFiles(apiDirectory)) {
    const sourceFile = parseSource(fileName);
    const imports = collectImports(rootDir, sourceFile);
    const requestBindings = new Set(
      [...imports.entries()]
        .filter(
          ([, imported]) =>
            imported.kind === 'default' &&
            imported.moduleName === '@/utils/request'
        )
        .map(([localName]) => localName)
    );
    if (requestBindings.size === 0) continue;

    function visit(node) {
      if (
        ts.isCallExpression(node) &&
        ts.isIdentifier(node.expression) &&
        requestBindings.has(node.expression.text)
      ) {
        const firstArgument = node.arguments[0]
          ? unwrapExpression(node.arguments[0])
          : undefined;
        let config;
        if (firstArgument && ts.isObjectLiteralExpression(firstArgument)) {
          config = { object: firstArgument, substitutions: new Map() };
        } else if (firstArgument && ts.isCallExpression(firstArgument)) {
          config = requestConfigFromHelper(rootDir, firstArgument, imports);
        } else {
          throw new Error(
            `request 缺少静态配置（${normalizeRelativePath(
              rootDir,
              fileName
            )}:${
              sourceFile.getLineAndCharacterOfPosition(node.getStart()).line + 1
            }）`
          );
        }

        const routePath = staticString(
          objectPropertyExpression(config.object, 'url'),
          config.substitutions
        );
        const method = staticString(
          objectPropertyExpression(config.object, 'method'),
          config.substitutions
        )?.toUpperCase();
        const requestBuilder = findExportedRequestBuilder(node);
        const decoder = node.arguments[1]
          ? decoderReference(rootDir, sourceFile, node.arguments[1], imports)
          : undefined;
        if (!routePath || !method || !requestBuilder || !decoder) {
          throw new Error(
            `生产 request 必须具有静态 path、method、builder 与 decoder（${normalizeRelativePath(
              rootDir,
              fileName
            )}:${
              sourceFile.getLineAndCharacterOfPosition(node.getStart()).line + 1
            }）`
          );
        }

        const apiForward = extractApiForward(
          config.object,
          config.substitutions
        );
        calls.push({
          method,
          path: routePath,
          requestBuilder: `${normalizeRelativePath(
            rootDir,
            fileName
          )}#${requestBuilder}`,
          decoder,
          ...(apiForward === undefined ? {} : { apiForward }),
        });
      }
      ts.forEachChild(node, visit);
    }
    visit(sourceFile);
  }

  return calls;
}

function extractNodeAdapters(rootDir) {
  const fileName = path.join(rootDir, 'src/ncmModDef.cjs');
  const sourceFile = parseSource(fileName, ts.ScriptKind.JS);
  const adapters = new Map();

  function visit(node) {
    if (ts.isObjectLiteralExpression(node)) {
      const route = staticString(objectPropertyExpression(node, 'route'));
      const identifier = staticString(
        objectPropertyExpression(node, 'identifier')
      );
      const moduleExpression = objectPropertyExpression(node, 'module');
      const moduleCall = moduleExpression
        ? unwrapExpression(moduleExpression)
        : undefined;
      const adapter =
        moduleCall &&
        ts.isCallExpression(moduleCall) &&
        ts.isIdentifier(moduleCall.expression) &&
        moduleCall.expression.text === 'require'
          ? staticString(moduleCall.arguments[0])
          : undefined;
      if (route && identifier && adapter) {
        if (adapters.has(route)) {
          throw new Error(`Node route 定义重复：${route}`);
        }
        adapters.set(route, { id: identifier, nodeAdapter: adapter });
      }
    }
    ts.forEachChild(node, visit);
  }
  visit(sourceFile);
  return adapters;
}

export function extractProductionNcmRoutes({ rootDir = projectRoot } = {}) {
  const calls = extractRequestCalls(rootDir);
  const nodeAdapters = extractNodeAdapters(rootDir);
  const seenPaths = new Set();
  const routes = calls.map(call => {
    if (seenPaths.has(call.path)) {
      throw new Error(`生产 request path 重复：${call.path}`);
    }
    seenPaths.add(call.path);
    const adapter = nodeAdapters.get(call.path);
    if (!adapter) {
      throw new Error(`生产 request 没有 Node adapter：${call.path}`);
    }
    return { ...adapter, ...call };
  });
  return routes.sort((left, right) => left.path.localeCompare(right.path));
}

export function loadSidecarRouteManifest({ rootDir = projectRoot } = {}) {
  return JSON.parse(
    readFileSync(path.join(rootDir, 'src/sidecar-route-manifest.json'), 'utf8')
  );
}

function describeIndex(index, route) {
  return `manifest[${index}]${
    route && typeof route.path === 'string' ? ` (${route.path})` : ''
  }`;
}

export function validateSidecarRouteManifest(manifest) {
  if (!Array.isArray(manifest)) throw new Error('route manifest 必须是数组');

  const errors = [];
  const ids = new Set();
  const paths = new Set();
  let apiForwardCount = 0;

  manifest.forEach((route, index) => {
    const label = describeIndex(index, route);
    if (!route || typeof route !== 'object' || Array.isArray(route)) {
      errors.push(`${label} 必须是对象`);
      return;
    }
    for (const field of REQUIRED_MANIFEST_FIELDS) {
      if (!(field in route)) errors.push(`${label} 缺少 ${field}`);
    }
    if (typeof route.id !== 'string' || !/^[a-z0-9_]+$/.test(route.id)) {
      errors.push(`${label}.id 必须是 snake_case`);
    } else if (ids.has(route.id)) {
      errors.push(`${label}.id 重复：${route.id}`);
    } else {
      ids.add(route.id);
    }
    if (route.method !== 'GET' && route.method !== 'POST') {
      errors.push(`${label}.method 只能是 GET 或 POST`);
    }
    if (typeof route.path !== 'string' || !route.path.startsWith('/')) {
      errors.push(`${label}.path 必须是绝对 API path`);
    } else if (paths.has(route.path)) {
      errors.push(`${label}.path 重复：${route.path}`);
    } else {
      paths.add(route.path);
    }
    if (
      typeof route.requestBuilder !== 'string' ||
      !/^src\/api\/[\w/-]+\.ts#[A-Za-z_$][\w$]*$/.test(route.requestBuilder)
    ) {
      errors.push(`${label}.requestBuilder 必须指向生产 API 导出函数`);
    }
    if (
      typeof route.nodeAdapter !== 'string' ||
      !route.nodeAdapter.startsWith('@neteaseapireborn/api/module/')
    ) {
      errors.push(`${label}.nodeAdapter 必须指向 NCM Node module`);
    }
    if (route.rustAdapter !== `ncm::${route.id}`) {
      errors.push(`${label}.rustAdapter 必须稳定映射为 ncm::${route.id}`);
    }
    if (
      typeof route.decoder !== 'string' ||
      !/^src\/api\/[\w/-]+\.ts#[A-Za-z_$][\w$]*$/.test(route.decoder)
    ) {
      errors.push(`${label}.decoder 必须指向具名生产 decoder`);
    }
    if (!Array.isArray(route.comparator) || route.comparator.length === 0) {
      errors.push(`${label}.comparator 不能为空`);
    } else {
      const comparatorFields = new Set();
      for (const field of route.comparator) {
        if (
          typeof field !== 'string' ||
          !/^[A-Za-z_][A-Za-z0-9_]*(?:\[\])?(?:\.[A-Za-z_][A-Za-z0-9_]*(?:\[\])?)*$/.test(
            field
          )
        ) {
          errors.push(`${label}.comparator 含无效稳定字段：${String(field)}`);
        } else if (/^(?:raw|json|response|body|data)$/.test(field)) {
          errors.push(`${label}.comparator 不得比较全量 JSON：${field}`);
        } else if (comparatorFields.has(field)) {
          errors.push(`${label}.comparator 字段重复：${field}`);
        } else {
          comparatorFields.add(field);
        }
      }
    }
    if (typeof route.idempotent !== 'boolean') {
      errors.push(`${label}.idempotent 必须是 boolean`);
    }
    if (route.method === 'POST' && route.idempotent !== false) {
      errors.push(`${label} POST 不得标成可自动重试`);
    }

    if ('apiForward' in route) {
      apiForwardCount += 1;
      if (route.path !== '/api') {
        errors.push(`${label}.apiForward 只允许用于 /api`);
      }
      if (
        !route.apiForward ||
        typeof route.apiForward !== 'object' ||
        Array.isArray(route.apiForward) ||
        !Array.isArray(route.apiForward.allowedPaths) ||
        route.apiForward.allowedPaths.length !== 1 ||
        route.apiForward.allowedPaths[0] !== '/api/cloud/lyric/get' ||
        route.apiForward.crypto !== 'eapi'
      ) {
        errors.push(
          `${label}.apiForward 必须只允许 /api/cloud/lyric/get 的 eapi builder`
        );
      }
    } else if (route.path === '/api') {
      errors.push(`${label} 缺少 /api 的显式 apiForward allowlist`);
    }
  });

  if (apiForwardCount !== 1) {
    errors.push(
      `/api 的 apiForward builder 必须恰好出现一次，实际 ${apiForwardCount}`
    );
  }
  if (errors.length > 0) {
    throw new Error(errors.join('\n'));
  }
  return manifest;
}

export function comparableManifestRoute(route) {
  return {
    id: route.id,
    method: route.method,
    path: route.path,
    requestBuilder: route.requestBuilder,
    nodeAdapter: route.nodeAdapter,
    decoder: route.decoder,
    ...('apiForward' in route ? { apiForward: route.apiForward } : {}),
  };
}

export function verifySidecarRouteManifest({ rootDir = projectRoot } = {}) {
  const manifest = validateSidecarRouteManifest(
    loadSidecarRouteManifest({ rootDir })
  );
  const actual = extractProductionNcmRoutes({ rootDir });
  const declared = manifest
    .map(comparableManifestRoute)
    .sort((left, right) => left.path.localeCompare(right.path));
  if (!isDeepStrictEqual(declared, actual)) {
    throw new Error(
      'route manifest 与 src/api 生产 request AST 或 src/ncmModDef.cjs 不一致'
    );
  }
  return { manifest, actual };
}

if (import.meta.main) {
  const { manifest } = verifySidecarRouteManifest();
  const postCount = manifest.filter(route => route.method === 'POST').length;
  console.log(
    `[sidecar-route-manifest] ${manifest.length} routes (${postCount} POST)`
  );
}
