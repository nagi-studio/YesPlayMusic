import en from './lang/en';
import ja from './lang/ja';
import zhCN from './lang/zh-CN';
import zhTW from './lang/zh-TW';

// Single source of truth for the supported locale set. Keep this module free of
// store, router and vue-i18n imports so tests and the navigator probe can use it
// without booting the app.
export const localeMessages = {
  en,
  ja,
  'zh-CN': zhCN,
  'zh-TW': zhTW,
} as const;

export type LocaleCode = keyof typeof localeMessages;

type LeafMessagePaths<T> = {
  [Key in keyof T & string]: T[Key] extends string
    ? Key
    : T[Key] extends Record<string, unknown>
    ? `${Key}.${LeafMessagePaths<T[Key]>}`
    : never;
}[keyof T & string];

export type MessageKey = LeafMessagePaths<typeof en>;

export const DEFAULT_LOCALE: LocaleCode = 'en';

export const LOCALE_CODES = Object.keys(localeMessages) as LocaleCode[];

/**
 * Ordered entries for the settings language picker. Labels are the language's
 * own endonym with no flag: a flag names a place, and these are languages and
 * scripts. Simplified and Traditional Chinese in particular are writing systems
 * rather than regions.
 */
export const LOCALE_OPTIONS: ReadonlyArray<{
  code: LocaleCode;
  label: string;
}> = [
  { code: 'en', label: 'English' },
  { code: 'ja', label: '日本語' },
  { code: 'zh-CN', label: '简体中文' },
  { code: 'zh-TW', label: '繁體中文' },
];

export function isLocaleCode(value: unknown): value is LocaleCode {
  return (
    typeof value === 'string' && (LOCALE_CODES as string[]).includes(value)
  );
}

/**
 * Sanitise a persisted `settings.lang`. Retired locales (Turkish shipped up to
 * 0.7.0) and hand-edited values fall back to the default instead of leaving
 * vue-i18n on a locale with no messages.
 */
export function normalizePersistedLocale(value: unknown): LocaleCode {
  return isLocaleCode(value) ? value : DEFAULT_LOCALE;
}

const TRADITIONAL_CHINESE_REGIONS = new Set(['tw', 'hk', 'mo']);

function resolveChineseLocale(subtags: readonly string[]): LocaleCode {
  // An explicit script is more precise than a region when both are present.
  if (subtags.includes('hant')) return 'zh-TW';
  if (subtags.includes('hans')) return 'zh-CN';
  return subtags.some(subtag => TRADITIONAL_CHINESE_REGIONS.has(subtag))
    ? 'zh-TW'
    : 'zh-CN';
}

/** Pick a supported locale for a `navigator.language` style tag. */
export function resolveLocale(language: string | undefined | null): LocaleCode {
  if (!language) return DEFAULT_LOCALE;
  const tag = language.trim();
  if (isLocaleCode(tag)) return tag;

  // POSIX locales can add an encoding/modifier and use `_` as a separator.
  const normalized = tag
    .toLowerCase()
    .replace(/[.@].*$/, '')
    .replace(/_/g, '-');
  const subtags = normalized.split('-').filter(Boolean);
  if (subtags[0] === 'zh') {
    // Ignore extension/private-use values when deciding the Chinese script.
    const extensionIndex = subtags.findIndex(
      (subtag, index) => index > 0 && subtag.length === 1
    );
    const coreSubtags = subtags.slice(
      1,
      extensionIndex === -1 ? undefined : extensionIndex
    );
    return resolveChineseLocale(coreSubtags);
  }

  const base = normalized.slice(0, 2);
  const match = LOCALE_CODES.find(code => code.toLowerCase() === base);
  return match ?? DEFAULT_LOCALE;
}
