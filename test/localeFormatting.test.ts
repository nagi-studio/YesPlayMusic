import { afterEach, describe, expect, mock, test } from 'bun:test';
import { LOCALE_CODES } from '../src/locale/catalog';
import type { LocaleCode } from '../src/locale/catalog';

// src/locale/index.ts reads the Pinia store at module scope, which drags in the
// router and a real `window`. Only the active locale matters here, so stub that
// module instead of booting the app. src/locale/catalog.ts stays store-free so
// the parity test needs no stub at all.
const activeLocale = { locale: 'en' as LocaleCode };
mock.module('../src/locale', () => ({ default: { global: activeLocale } }));

const { formatDate, formatPlayCount, formatTime } = await import(
  '../src/utils/filters'
);

// formatTime/formatDate/formatPlayCount branch on the active locale. Adding a
// language without touching them silently leaves that language on English or,
// worse, on Chinese units.
function withLocale<T>(code: LocaleCode, run: () => T): T {
  activeLocale.locale = code;
  return run();
}

afterEach(() => {
  activeLocale.locale = 'en';
});

const HOUR_AND_A_HALF = 90 * 60 * 1000;
const RELEASE_DAY = new Date(2024, 4, 1, 12).getTime();

describe('按语言格式化', () => {
  test('时长单位跟随语言，日文不落回英文', () => {
    expect(withLocale('en', () => formatTime(HOUR_AND_A_HALF, 'Human'))).toBe(
      '1 hr 30 min'
    );
    expect(withLocale('ja', () => formatTime(HOUR_AND_A_HALF, 'Human'))).toBe(
      '1 時間 30 分'
    );
    expect(
      withLocale('zh-CN', () => formatTime(HOUR_AND_A_HALF, 'Human'))
    ).toBe('1 小时 30 分钟');
    expect(
      withLocale('zh-TW', () => formatTime(HOUR_AND_A_HALF, 'Human'))
    ).toBe('1 小時 30 分鐘');
  });

  test('日期格式跟随语言，日文用年月日', () => {
    expect(withLocale('ja', () => formatDate(RELEASE_DAY))).toBe(
      '2024年5月1日'
    );
    expect(withLocale('zh-CN', () => formatDate(RELEASE_DAY))).toBe(
      '2024年05月01日'
    );
    // An explicit format argument is still overridden for these locales.
    expect(
      withLocale('ja', () => formatDate(RELEASE_DAY, 'MMMM D, YYYY'))
    ).toBe('2024年5月1日');
    expect(withLocale('en', () => formatDate(RELEASE_DAY))).toBe('May 1, 2024');
  });

  test('播放数用万/億分组，日文不落回 K/M', () => {
    expect(withLocale('ja', () => formatPlayCount(23_200_000))).toBe('2320万');
    expect(withLocale('ja', () => formatPlayCount(250_000_000))).toBe('2.5億');
    expect(withLocale('zh-CN', () => formatPlayCount(250_000_000))).toBe(
      '2.5亿'
    );
    expect(withLocale('zh-TW', () => formatPlayCount(250_000_000))).toBe(
      '2.5億'
    );
    expect(withLocale('en', () => formatPlayCount(23_200_000))).toBe('23.2M');
    // Below the first grouping threshold every locale returns the raw number.
    expect(withLocale('ja', () => formatPlayCount(999))).toBe(999);
  });

  test('每种注册语言都有自己的时长单位，新增语言不会静默落回英文', () => {
    const english = withLocale('en', () =>
      formatTime(HOUR_AND_A_HALF, 'Human')
    );
    const fellBack = LOCALE_CODES.filter(
      code =>
        code !== 'en' &&
        withLocale(code, () => formatTime(HOUR_AND_A_HALF, 'Human')) === english
    );
    expect(fellBack).toEqual([]);
  });
});
