import { createI18n } from 'vue-i18n';
import { getAppStore } from '@/stores/accessor';
import {
  DEFAULT_LOCALE,
  localeMessages,
  normalizePersistedLocale,
} from './catalog';
import type { MessageKey } from './catalog';

const i18n = createI18n({
  legacy: true,
  locale: normalizePersistedLocale(getAppStore().settings.lang),
  fallbackLocale: DEFAULT_LOCALE,
  messages: localeMessages,
  // Legacy API mode ignores missingWarn/fallbackWarn; silentTranslationWarn and
  // silentFallbackWarn are the options it actually reads. Keep the warnings in
  // development so a missing or misspelled key is loud, and silence them in
  // production so a single bad key cannot spam the user's console.
  silentTranslationWarn: import.meta.env.PROD,
  silentFallbackWarn: import.meta.env.PROD,
});

// Keep vue-i18n mode details behind a typed adapter.
const locale = Object.assign(i18n, {
  t(key: MessageKey): string {
    return String(i18n.global.t(key));
  },
});

export default locale;
