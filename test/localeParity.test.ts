import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { compileTemplate, parse as parseSfc } from '@vue/compiler-sfc';
import ts from 'typescript';
import {
  DEFAULT_LOCALE,
  LOCALE_CODES,
  LOCALE_OPTIONS,
  isLocaleCode,
  localeMessages,
  normalizePersistedLocale,
  resolveLocale,
} from '../src/locale/catalog';

// Legacy API mode ignores missingWarn, so a missing or misspelled key resolves to
// the raw key path on screen. These assertions are what makes that drift visible.
const BASELINE = DEFAULT_LOCALE;

type Messages = { [key: string]: string | Messages };

function flatten(messages: Messages, prefix = ''): [string, string][] {
  return Object.entries(messages).flatMap(([key, value]) =>
    typeof value === 'string'
      ? ([[`${prefix}${key}`, value]] as [string, string][])
      : flatten(value, `${prefix}${key}.`)
  );
}

function entriesOf(code: (typeof LOCALE_CODES)[number]): [string, string][] {
  return flatten(localeMessages[code] as Messages).sort(([left], [right]) =>
    left.localeCompare(right)
  );
}

function keysOf(code: (typeof LOCALE_CODES)[number]): string[] {
  return entriesOf(code).map(([key]) => key);
}

/** `{name}` style named interpolations, order-insensitive. */
function placeholdersOf(value: string): string[] {
  return [...value.matchAll(/\{\s*([A-Za-z_$][\w$]*)\s*\}/g)]
    .map(match => match[1] as string)
    .sort();
}

interface TranslationUsage {
  referenced: Set<string>;
  unresolved: string[];
}

interface TranslationScan extends TranslationUsage {
  receiverNames: Set<string>;
}

const MAX_STATIC_KEYS = 128;

function unwrapExpression(expression: ts.Expression): ts.Expression {
  let current = expression;
  while (
    ts.isParenthesizedExpression(current) ||
    ts.isAsExpression(current) ||
    ts.isTypeAssertionExpression(current) ||
    ts.isNonNullExpression(current) ||
    ts.isSatisfiesExpression(current) ||
    ts.isPartiallyEmittedExpression(current)
  ) {
    current = current.expression;
  }
  return current;
}

function propertyNameText(name: ts.PropertyName | undefined): string | null {
  if (!name) return null;
  if (
    ts.isIdentifier(name) ||
    ts.isStringLiteralLike(name) ||
    ts.isNumericLiteral(name)
  ) {
    return name.text;
  }
  return null;
}

function createTypeChecker(sourceFile: ts.SourceFile): ts.TypeChecker {
  const options: ts.CompilerOptions = {
    allowJs: true,
    module: ts.ModuleKind.ESNext,
    noLib: true,
    noResolve: true,
    target: ts.ScriptTarget.Latest,
  };
  const host: ts.CompilerHost = {
    fileExists: fileName => fileName === sourceFile.fileName,
    getCanonicalFileName: fileName => fileName,
    getCurrentDirectory: () => '.',
    getDefaultLibFileName: () => 'lib.d.ts',
    getDirectories: () => [],
    getNewLine: () => '\n',
    getSourceFile: fileName =>
      fileName === sourceFile.fileName ? sourceFile : undefined,
    readFile: fileName =>
      fileName === sourceFile.fileName ? sourceFile.text : undefined,
    useCaseSensitiveFileNames: () => true,
    writeFile: () => undefined,
  };
  return ts
    .createProgram([sourceFile.fileName], options, host)
    .getTypeChecker();
}

function constantInitializer(
  identifier: ts.Identifier,
  checker: ts.TypeChecker
): { initializer: ts.Expression; symbol: ts.Symbol } | null {
  const symbol = checker.getSymbolAtLocation(identifier);
  const declarations = symbol?.declarations;
  if (!symbol || declarations?.length !== 1) return null;
  const declaration = declarations[0];
  if (
    !declaration ||
    !ts.isVariableDeclaration(declaration) ||
    !declaration.initializer ||
    !ts.isVariableDeclarationList(declaration.parent) ||
    (declaration.parent.flags & ts.NodeFlags.Const) === 0
  ) {
    return null;
  }
  return { initializer: declaration.initializer, symbol };
}

function objectLiteralFrom(
  expression: ts.Expression,
  checker: ts.TypeChecker,
  seen: ReadonlySet<ts.Symbol>,
  requireFrozen = false
): ts.ObjectLiteralExpression | null {
  const current = unwrapExpression(expression);
  if (ts.isObjectLiteralExpression(current)) {
    return requireFrozen ? null : current;
  }
  if (
    ts.isCallExpression(current) &&
    current.arguments.length === 1 &&
    ts.isPropertyAccessExpression(current.expression) &&
    ts.isIdentifier(current.expression.expression) &&
    current.expression.expression.text === 'Object' &&
    current.expression.name.text === 'freeze'
  ) {
    const argument = current.arguments[0];
    const frozen = argument ? unwrapExpression(argument) : null;
    return frozen && ts.isObjectLiteralExpression(frozen) ? frozen : null;
  }
  if (!ts.isIdentifier(current)) return null;
  const binding = constantInitializer(current, checker);
  if (!binding || seen.has(binding.symbol)) return null;
  return objectLiteralFrom(
    binding.initializer,
    checker,
    new Set([...seen, binding.symbol]),
    true
  );
}

function objectProperties(
  object: ts.ObjectLiteralExpression
): Map<string, ts.Expression> | null {
  const properties = new Map<string, ts.Expression>();
  for (const property of object.properties) {
    const name = propertyNameText(property.name);
    if (!name || properties.has(name)) return null;
    if (ts.isPropertyAssignment(property)) {
      properties.set(name, property.initializer);
    } else if (ts.isShorthandPropertyAssignment(property)) {
      properties.set(name, property.name);
    } else {
      // Spreads, accessors and computed/method properties are not safe to guess.
      return null;
    }
  }
  return properties;
}

function mergeStaticValues(
  values: ReadonlyArray<Set<string> | null>
): Set<string> | null {
  const merged = new Set<string>();
  for (const value of values) {
    if (!value) return null;
    for (const entry of value) {
      merged.add(entry);
      if (merged.size > MAX_STATIC_KEYS) return null;
    }
  }
  return merged;
}

function concatenateStaticValues(
  left: Set<string>,
  right: Set<string>
): Set<string> | null {
  const combined = new Set<string>();
  for (const leftValue of left) {
    for (const rightValue of right) {
      combined.add(`${leftValue}${rightValue}`);
      if (combined.size > MAX_STATIC_KEYS) return null;
    }
  }
  return combined;
}

function resolveStaticValues(
  expression: ts.Expression,
  checker: ts.TypeChecker,
  seen: ReadonlySet<ts.Symbol> = new Set()
): Set<string> | null {
  const current = unwrapExpression(expression);
  if (ts.isStringLiteralLike(current)) return new Set([current.text]);

  if (ts.isConditionalExpression(current)) {
    return mergeStaticValues([
      resolveStaticValues(current.whenTrue, checker, seen),
      resolveStaticValues(current.whenFalse, checker, seen),
    ]);
  }

  if (
    ts.isBinaryExpression(current) &&
    current.operatorToken.kind === ts.SyntaxKind.PlusToken
  ) {
    const left = resolveStaticValues(current.left, checker, seen);
    const right = resolveStaticValues(current.right, checker, seen);
    return left && right ? concatenateStaticValues(left, right) : null;
  }

  if (ts.isTemplateExpression(current)) {
    let values = new Set([current.head.text]);
    for (const span of current.templateSpans) {
      const substitution = resolveStaticValues(span.expression, checker, seen);
      if (!substitution) return null;
      const withSubstitution = concatenateStaticValues(values, substitution);
      if (!withSubstitution) return null;
      const withLiteral = concatenateStaticValues(
        withSubstitution,
        new Set([span.literal.text])
      );
      if (!withLiteral) return null;
      values = withLiteral;
    }
    return values;
  }

  if (ts.isIdentifier(current)) {
    const binding = constantInitializer(current, checker);
    if (!binding || seen.has(binding.symbol)) return null;
    return resolveStaticValues(
      binding.initializer,
      checker,
      new Set([...seen, binding.symbol])
    );
  }

  if (ts.isArrayLiteralExpression(current)) {
    return mergeStaticValues(
      current.elements.map(element =>
        ts.isSpreadElement(element)
          ? null
          : resolveStaticValues(element, checker, seen)
      )
    );
  }

  if (ts.isObjectLiteralExpression(current)) {
    const properties = objectProperties(current);
    return properties
      ? mergeStaticValues(
          [...properties.values()].map(value =>
            resolveStaticValues(value, checker, seen)
          )
        )
      : null;
  }

  if (ts.isPropertyAccessExpression(current)) {
    const object = objectLiteralFrom(current.expression, checker, seen);
    const properties = object ? objectProperties(object) : null;
    const value = properties?.get(current.name.text);
    return value ? resolveStaticValues(value, checker, seen) : null;
  }

  if (ts.isElementAccessExpression(current)) {
    const object = objectLiteralFrom(current.expression, checker, seen);
    const properties = object ? objectProperties(object) : null;
    if (!properties) return null;
    const argument = current.argumentExpression
      ? unwrapExpression(current.argumentExpression)
      : null;
    if (argument && ts.isStringLiteralLike(argument)) {
      const value = properties.get(argument.text);
      return value ? resolveStaticValues(value, checker, seen) : null;
    }
    // Runtime indexing is safe only when every possible value is checked.
    return mergeStaticValues(
      [...properties.values()].map(value =>
        resolveStaticValues(value, checker, seen)
      )
    );
  }

  return null;
}

function rootIdentifier(expression: ts.Expression): ts.Identifier | null {
  const current = unwrapExpression(expression);
  if (ts.isIdentifier(current)) return current;
  if (
    ts.isPropertyAccessExpression(current) ||
    ts.isElementAccessExpression(current)
  ) {
    return rootIdentifier(current.expression);
  }
  return null;
}

interface TranslationBindings {
  checker: ts.TypeChecker;
  functionSymbols: Set<ts.Symbol>;
  receiverNames: Set<string>;
  receiverSymbols: Set<ts.Symbol>;
  templateReceiverNames: ReadonlySet<string>;
}

function isLocaleModule(value: string): boolean {
  return /(?:^|\/)locale(?:\/index)?$/.test(value);
}

function isDynamicLocaleImport(expression: ts.Expression): boolean {
  const current = unwrapExpression(expression);
  if (ts.isAwaitExpression(current)) {
    return isDynamicLocaleImport(current.expression);
  }
  if (
    ts.isPropertyAccessExpression(current) &&
    current.name.text === 'default'
  ) {
    return isDynamicLocaleImport(current.expression);
  }
  if (
    !ts.isCallExpression(current) ||
    current.expression.kind !== ts.SyntaxKind.ImportKeyword
  ) {
    return false;
  }
  const moduleName = current.arguments[0];
  return Boolean(
    moduleName &&
      ts.isStringLiteralLike(moduleName) &&
      isLocaleModule(moduleName.text)
  );
}

function contextMemberName(expression: ts.Expression): string | null {
  const current = unwrapExpression(expression);
  if (!ts.isPropertyAccessExpression(current)) return null;
  const receiver = unwrapExpression(current.expression);
  if (ts.isIdentifier(receiver) && receiver.text === '_ctx') {
    return current.name.text;
  }
  return contextMemberName(current.expression);
}

function collectTranslationBindings(
  sourceFile: ts.SourceFile,
  checker: ts.TypeChecker,
  templateReceiverNames: ReadonlySet<string>
): TranslationBindings {
  const receiverSymbols = new Set<ts.Symbol>();
  const functionSymbols = new Set<ts.Symbol>();
  const receiverNames = new Set<string>();

  const addReceiver = (identifier: ts.Identifier): boolean => {
    const symbol = checker.getSymbolAtLocation(identifier);
    if (!symbol || receiverSymbols.has(symbol)) return false;
    receiverSymbols.add(symbol);
    receiverNames.add(identifier.text);
    return true;
  };
  const addFunction = (identifier: ts.Identifier): boolean => {
    const symbol = checker.getSymbolAtLocation(identifier);
    if (!symbol || functionSymbols.has(symbol)) return false;
    functionSymbols.add(symbol);
    return true;
  };
  const isReceiverExpression = (expression: ts.Expression): boolean => {
    const root = rootIdentifier(expression);
    const symbol = root ? checker.getSymbolAtLocation(root) : undefined;
    return Boolean(symbol && receiverSymbols.has(symbol));
  };
  const addDefaultBinding = (name: ts.BindingName): void => {
    if (!ts.isObjectBindingPattern(name)) return;
    for (const element of name.elements) {
      const localName = ts.isIdentifier(element.name)
        ? element.name.text
        : null;
      if (
        (propertyNameText(element.propertyName) ?? localName) === 'default' &&
        ts.isIdentifier(element.name)
      ) {
        addReceiver(element.name);
      }
    }
  };

  for (const statement of sourceFile.statements) {
    if (
      ts.isImportDeclaration(statement) &&
      ts.isStringLiteral(statement.moduleSpecifier) &&
      isLocaleModule(statement.moduleSpecifier.text) &&
      statement.importClause?.name
    ) {
      addReceiver(statement.importClause.name);
    }
  }

  const collectDynamicImports = (node: ts.Node): void => {
    if (ts.isVariableDeclaration(node) && node.initializer) {
      if (isDynamicLocaleImport(node.initializer)) {
        if (ts.isIdentifier(node.name)) addReceiver(node.name);
        else addDefaultBinding(node.name);
      }

      if (ts.isArrayBindingPattern(node.name)) {
        const bindings = node.name;
        const awaited = unwrapExpression(node.initializer);
        const promiseAll = ts.isAwaitExpression(awaited)
          ? unwrapExpression(awaited.expression)
          : awaited;
        if (
          ts.isCallExpression(promiseAll) &&
          ts.isPropertyAccessExpression(promiseAll.expression) &&
          ts.isIdentifier(promiseAll.expression.expression) &&
          promiseAll.expression.expression.text === 'Promise' &&
          promiseAll.expression.name.text === 'all'
        ) {
          const imports = promiseAll.arguments[0];
          if (imports && ts.isArrayLiteralExpression(imports)) {
            imports.elements.forEach((entry, index) => {
              if (ts.isSpreadElement(entry) || !isDynamicLocaleImport(entry)) {
                return;
              }
              const binding = bindings.elements[index];
              if (binding && ts.isBindingElement(binding)) {
                addDefaultBinding(binding.name);
              }
            });
          }
        }
      }
    }
    ts.forEachChild(node, collectDynamicImports);
  };
  collectDynamicImports(sourceFile);

  let changed = true;
  while (changed) {
    changed = false;
    const collectAliases = (node: ts.Node): void => {
      if (
        ts.isVariableDeclaration(node) &&
        node.initializer &&
        ts.isVariableDeclarationList(node.parent) &&
        (node.parent.flags & ts.NodeFlags.Const) !== 0
      ) {
        const initializer = unwrapExpression(node.initializer);
        if (ts.isIdentifier(node.name)) {
          if (
            (ts.isPropertyAccessExpression(initializer) &&
              initializer.name.text === 't' &&
              isReceiverExpression(initializer.expression)) ||
            (ts.isPropertyAccessExpression(initializer) &&
              initializer.name.text === '$t' &&
              initializer.expression.kind === ts.SyntaxKind.ThisKeyword) ||
            (ts.isIdentifier(initializer) &&
              initializer.text === '$t' &&
              !checker.getSymbolAtLocation(initializer))
          ) {
            changed = addFunction(node.name) || changed;
          } else if (isReceiverExpression(initializer)) {
            changed = addReceiver(node.name) || changed;
          }
        } else if (
          ts.isObjectBindingPattern(node.name) &&
          isReceiverExpression(initializer)
        ) {
          for (const element of node.name.elements) {
            const localName = ts.isIdentifier(element.name)
              ? element.name.text
              : null;
            if (
              (propertyNameText(element.propertyName) ?? localName) === 't' &&
              ts.isIdentifier(element.name)
            ) {
              changed = addFunction(element.name) || changed;
            }
          }
        }
      }
      ts.forEachChild(node, collectAliases);
    };
    collectAliases(sourceFile);
  }

  return {
    checker,
    functionSymbols,
    receiverNames,
    receiverSymbols,
    templateReceiverNames,
  };
}

function isTranslationCall(
  call: ts.CallExpression,
  bindings: TranslationBindings
): boolean {
  const callee = unwrapExpression(call.expression);
  if (ts.isIdentifier(callee)) {
    const symbol = bindings.checker.getSymbolAtLocation(callee);
    return symbol ? bindings.functionSymbols.has(symbol) : callee.text === '$t';
  }
  if (ts.isPropertyAccessExpression(callee)) {
    if (callee.name.text === '$t') {
      const receiver = unwrapExpression(callee.expression);
      return (
        receiver.kind === ts.SyntaxKind.ThisKeyword ||
        (ts.isIdentifier(receiver) && receiver.text === '_ctx')
      );
    }
    if (callee.name.text !== 't') return false;
    const root = rootIdentifier(callee.expression);
    const symbol = root
      ? bindings.checker.getSymbolAtLocation(root)
      : undefined;
    return Boolean(
      (symbol && bindings.receiverSymbols.has(symbol)) ||
        bindings.templateReceiverNames.has(
          contextMemberName(callee.expression) ?? ''
        )
    );
  }
  if (ts.isElementAccessExpression(callee)) {
    const argument = callee.argumentExpression;
    if (!argument || !ts.isStringLiteralLike(argument)) return false;
    const receiver = unwrapExpression(callee.expression);
    if (argument.text === '$t') {
      return (
        receiver.kind === ts.SyntaxKind.ThisKeyword ||
        (ts.isIdentifier(receiver) && receiver.text === '_ctx')
      );
    }
    if (argument.text !== 't') return false;
    const root = rootIdentifier(callee.expression);
    const symbol = root
      ? bindings.checker.getSymbolAtLocation(root)
      : undefined;
    return Boolean(symbol && bindings.receiverSymbols.has(symbol));
  }
  return false;
}

function mergeUsage(target: TranslationUsage, source: TranslationUsage): void {
  for (const key of source.referenced) target.referenced.add(key);
  target.unresolved.push(...source.unresolved);
}

function scanTranslationSource(
  source: string,
  fileName: string,
  scriptKind: ts.ScriptKind = ts.ScriptKind.TS,
  templateReceiverNames: ReadonlySet<string> = new Set()
): TranslationScan {
  const sourceFile = ts.createSourceFile(
    fileName,
    source,
    ts.ScriptTarget.Latest,
    true,
    scriptKind
  );
  const checker = createTypeChecker(sourceFile);
  const bindings = collectTranslationBindings(
    sourceFile,
    checker,
    templateReceiverNames
  );
  const usage: TranslationScan = {
    referenced: new Set(),
    receiverNames: bindings.receiverNames,
    unresolved: [],
  };

  const visit = (node: ts.Node): void => {
    if (ts.isCallExpression(node) && isTranslationCall(node, bindings)) {
      const argument = node.arguments[0];
      const keys = argument ? resolveStaticValues(argument, checker) : null;
      if (!keys || keys.size === 0) {
        const { line, character } = sourceFile.getLineAndCharacterOfPosition(
          node.getStart(sourceFile)
        );
        usage.unresolved.push(
          `${fileName}:${line + 1}:${character + 1} ${node.expression.getText(
            sourceFile
          )}(${argument?.getText(sourceFile) ?? ''})`
        );
      } else {
        for (const key of keys) usage.referenced.add(key);
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(sourceFile);
  return usage;
}

function scanVueTranslationSource(
  source: string,
  fileName: string
): TranslationUsage {
  const usage: TranslationUsage = { referenced: new Set(), unresolved: [] };
  const receiverNames = new Set<string>();
  const parsed = parseSfc(source, { filename: fileName });
  if (parsed.errors.length > 0) {
    throw new Error(`${fileName}: ${parsed.errors.map(String).join('; ')}`);
  }
  for (const [index, block] of [
    parsed.descriptor.script,
    parsed.descriptor.scriptSetup,
  ].entries()) {
    if (block) {
      const scriptUsage = scanTranslationSource(
        block.content,
        `${fileName}.script-${index}.ts`
      );
      mergeUsage(usage, scriptUsage);
      for (const name of scriptUsage.receiverNames) receiverNames.add(name);
    }
  }
  const template = parsed.descriptor.template;
  if (template) {
    const compiled = compileTemplate({
      id: fileName,
      filename: fileName,
      source: template.content,
    });
    if (compiled.errors.length > 0) {
      throw new Error(`${fileName}: ${compiled.errors.map(String).join('; ')}`);
    }
    mergeUsage(
      usage,
      scanTranslationSource(
        compiled.code,
        `${fileName}.template.js`,
        ts.ScriptKind.JS,
        receiverNames
      )
    );
  }
  return usage;
}

function scanProjectTranslationUsage(): TranslationUsage {
  const usage: TranslationUsage = { referenced: new Set(), unresolved: [] };
  const files = [...new Bun.Glob('src/**/*.{ts,vue}').scanSync('.')].sort();

  for (const file of files) {
    if (file.startsWith('src/locale/lang/')) continue;
    const source = readFileSync(file, 'utf8');
    mergeUsage(
      usage,
      file.endsWith('.ts')
        ? scanTranslationSource(source, file)
        : scanVueTranslationSource(source, file)
    );
  }
  return usage;
}

const translationUsage = scanProjectTranslationUsage();

describe('locale 目录', () => {
  test('catalog 是唯一语言清单，选项与 messages 一一对应', () => {
    expect(LOCALE_CODES.length).toBeGreaterThan(0);
    expect(LOCALE_OPTIONS.map(option => option.code).sort()).toEqual(
      [...LOCALE_CODES].sort()
    );
    expect(new Set(LOCALE_OPTIONS.map(option => option.label)).size).toBe(
      LOCALE_OPTIONS.length
    );
    expect(LOCALE_CODES).toContain(DEFAULT_LOCALE);
    // A flag names a place; these entries name languages and scripts.
    const flagged = LOCALE_OPTIONS.filter(option =>
      /[\u{1F1E6}-\u{1F1FF}]/u.test(option.label)
    ).map(option => option.code);
    expect(flagged).toEqual([]);
    // Every registered code must actually carry messages.
    for (const code of LOCALE_CODES) {
      expect(keysOf(code).length).toBeGreaterThan(0);
    }
  });

  test('语言选择器由 catalog 驱动，没有第二份硬编码清单', () => {
    const settingsView = readFileSync('src/views/settings.vue', 'utf8');
    const picker = settingsView.match(
      /<Select\s+v-model="lang"[^>]*:options="langOptions"[^>]*>/
    )?.[0];
    expect(picker).toBeString();
    // langOptions 由 localeOptions computed 生成，后者来自 catalog
    expect(settingsView).toContain('langOptions()');
    expect(settingsView).toContain('localeOptions.map');
    // A literal <option value="xx"> inside this picker is a second source of truth.
    const hardcoded = [...(settingsView.matchAll(/<option\s+value="/g))].length;
    expect({ picker: 'lang', hardcodedOptions: hardcoded }).toEqual({
      picker: 'lang',
      hardcodedOptions: 0,
    });
  });

  test('退役 locale 与非法值安全回退到默认语言', () => {
    expect(normalizePersistedLocale('tr')).toBe(DEFAULT_LOCALE);
    expect(normalizePersistedLocale(null)).toBe(DEFAULT_LOCALE);
    expect(normalizePersistedLocale('')).toBe(DEFAULT_LOCALE);
    expect(normalizePersistedLocale('zh-CN')).toBe('zh-CN');
    expect(isLocaleCode('tr')).toBe(false);
  });

  test('繁体地区不会被识别成简体', () => {
    for (const tag of [
      'zh-TW',
      'zh-HK',
      'zh-MO',
      'zh-Hant',
      'zh-Hant-HK',
      'zh-cmn-Hant-TW',
      'zh-yue-Hant-HK',
    ]) {
      expect(resolveLocale(tag)).toBe('zh-TW');
    }
    for (const tag of ['zh', 'zh-CN', 'zh-Hans', 'zh-Hans-CN', 'zh-SG']) {
      expect(resolveLocale(tag)).toBe('zh-CN');
    }
    // Some platforms report POSIX tags; `_` must not defeat the script match.
    for (const tag of [
      'zh_Hant_HK',
      'zh_Hant_TW',
      'zh_MO',
      'zh_TW.UTF-8',
      'zh_HK.UTF-8',
      'zh_MO@traditional',
    ]) {
      expect(resolveLocale(tag)).toBe('zh-TW');
    }
    // An unlisted region or a BCP-47 extension is still Chinese, not English.
    for (const tag of ['zh_Hans_SG', 'zh-u-nu-hanidec', 'zh-Hans-MY']) {
      expect(resolveLocale(tag)).toBe('zh-CN');
    }
    expect(resolveLocale('en-US')).toBe('en');
    expect(resolveLocale('ja-JP')).toBe('ja');
    expect(resolveLocale('ja')).toBe('ja');
    // A retired or unsupported system language must not select empty messages.
    expect(resolveLocale('tr-TR')).toBe(DEFAULT_LOCALE);
    expect(resolveLocale(undefined)).toBe(DEFAULT_LOCALE);
  });
});

describe('locale 文件一致性', () => {
  test('翻译调用扫描器按 AST 精确解析，并对动态值 fail closed', () => {
    const staticUsage = scanTranslationSource(
      `
        import i18n from './locale';
        const mapped = Object.freeze({
          first: 'mapped.one',
          second: "mapped.two",
        } as const);
        $t("double.key");
        this.$t?.(flag ? \`template.key\` : 'single.key');
        i18n.t(mapped[kind]);
        // $t('comment.only')
      `,
      'locale-scanner-static.ts'
    );
    expect([...staticUsage.referenced].sort()).toEqual([
      'double.key',
      'mapped.one',
      'mapped.two',
      'single.key',
      'template.key',
    ]);
    expect(staticUsage.unresolved).toEqual([]);

    const dynamicUsage = scanTranslationSource(
      'const key = getRuntimeKey(); $t(key);',
      'locale-scanner-dynamic.ts'
    );
    expect([...dynamicUsage.referenced]).toEqual([]);
    expect(dynamicUsage.unresolved).toHaveLength(1);

    const mutableMapUsage = scanTranslationSource(
      `
        const keys = { current: 'stale.key' };
        keys.current = getRuntimeKey();
        this.$t(keys.current);
      `,
      'locale-scanner-mutable-map.ts'
    );
    expect([...mutableMapUsage.referenced]).toEqual([]);
    expect(mutableMapUsage.unresolved).toHaveLength(1);

    const shadowedUsage = scanTranslationSource(
      `
        const key = 'false.positive';
        function render(key: string) { return this.$t(key); }
        function $t(value: string) { return value; }
        $t('not.a.translation');
        import locale from './locale';
        function shadow(locale: { t(value: string): string }) {
          return locale.t('also.not.translation');
        }
      `,
      'locale-scanner-shadowed.ts'
    );
    expect([...shadowedUsage.referenced]).toEqual([]);
    expect(shadowedUsage.unresolved).toHaveLength(1);

    const dynamicImportUsage = scanTranslationSource(
      `
        import locale from './locale';
        const { default: first } = await import('./locale');
        const second = (await import('./locale')).default;
        const adapterTranslate = locale.t;
        const translate = this.$t;
        first.t('dynamic.first');
        second.t('dynamic.second');
        adapterTranslate('aliased.adapter');
        translate('aliased.global');
      `,
      'locale-scanner-imports.ts'
    );
    expect([...dynamicImportUsage.referenced].sort()).toEqual([
      'aliased.adapter',
      'aliased.global',
      'dynamic.first',
      'dynamic.second',
    ]);
    expect(dynamicImportUsage.unresolved).toEqual([]);

    const vueUsage = scanVueTranslationSource(
      `
        <template>
          <div>{{ locale.t('template.adapter') }}</div>
          <p>{{ $t('template.global') }}</p>
        </template>
        <script lang="ts">import locale from './locale';</script>
      `,
      'locale-scanner.vue'
    );
    expect([...vueUsage.referenced].sort()).toEqual([
      'template.adapter',
      'template.global',
    ]);
    expect(vueUsage.unresolved).toEqual([]);
  });

  test('每个 locale 的 key 集合与基准完全相等', () => {
    const baseline = keysOf(BASELINE);
    expect(baseline.length).toBeGreaterThan(0);

    for (const code of LOCALE_CODES) {
      if (code === BASELINE) continue;
      const actual = keysOf(code);
      const missing = baseline.filter(key => !actual.includes(key));
      const extra = actual.filter(key => !baseline.includes(key));
      expect({ locale: code, missing, extra }).toEqual({
        locale: code,
        missing: [],
        extra: [],
      });
    }
  });

  test('插值 placeholder 集合逐 key 对齐', () => {
    const baseline = new Map(
      entriesOf(BASELINE).map(([key, value]) => [key, placeholdersOf(value)])
    );

    for (const code of LOCALE_CODES) {
      if (code === BASELINE) continue;
      const mismatched = entriesOf(code)
        .map(([key, value]) => ({
          key,
          expected: baseline.get(key) ?? [],
          actual: placeholdersOf(value),
        }))
        .filter(entry => entry.expected.join(',') !== entry.actual.join(','));
      // A dropped {name} renders the literal placeholder or silently loses data.
      expect({ locale: code, mismatched }).toEqual({
        locale: code,
        mismatched: [],
      });
    }
  });

  test('没有重复 key 路径，也没有空值', () => {
    for (const code of LOCALE_CODES) {
      const keys = keysOf(code);
      expect({
        locale: code,
        duplicates: keys.length - new Set(keys).size,
      }).toEqual({ locale: code, duplicates: 0 });

      const blank = entriesOf(code)
        .filter(([, value]) => value.trim() === '')
        .map(([key]) => key);
      expect({ locale: code, blank }).toEqual({ locale: code, blank: [] });
    }
  });

  test('每个 locale key 都被代码引用', () => {
    expect(translationUsage.unresolved).toEqual([]);
    const orphans = keysOf(BASELINE).filter(
      key => !translationUsage.referenced.has(key)
    );
    expect(orphans).toEqual([]);
  });

  test('代码里的静态 $t/locale.t key 都存在于基准 locale', () => {
    expect(translationUsage.unresolved).toEqual([]);
    const baseline = new Set(keysOf(BASELINE));
    expect(translationUsage.referenced.size).toBeGreaterThan(100);
    const unknown = [...translationUsage.referenced].filter(
      key => !baseline.has(key)
    );
    expect(unknown).toEqual([]);
  });
});
