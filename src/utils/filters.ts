import dayjs from 'dayjs';
import duration from 'dayjs/plugin/duration';
import relativeTime from 'dayjs/plugin/relativeTime';
import locale from '@/locale';
import { normalizePersistedLocale } from '@/locale/catalog';
import type { LocaleCode } from '@/locale/catalog';
import { buildArtworkURL } from '@/utils/artwork';

dayjs.extend(duration);
dayjs.extend(relativeTime);

const activeLocale = (): LocaleCode =>
  normalizePersistedLocale(locale.global.locale);

const DURATION_UNITS: Record<LocaleCode, { hour: string; minute: string }> = {
  en: { hour: 'hr', minute: 'min' },
  ja: { hour: '時間', minute: '分' },
  'zh-CN': { hour: '小时', minute: '分钟' },
  'zh-TW': { hour: '小時', minute: '分鐘' },
};

/** Locales that write dates as year-month-day with unit characters. */
const DATE_FORMATS: Partial<Record<LocaleCode, string>> = {
  ja: 'YYYY年M月D日',
  'zh-CN': 'YYYY年MM月DD日',
  'zh-TW': 'YYYY年MM月DD日',
};

/** Locales that group large numbers by 万 instead of by thousand. */
const MYRIAD_UNITS: Partial<
  Record<LocaleCode, { unit: string; squared: string }>
> = {
  ja: { unit: '万', squared: '億' },
  'zh-CN': { unit: '万', squared: '亿' },
  'zh-TW': { unit: '萬', squared: '億' },
};

export function formatTime(
  Milliseconds: number | null | undefined,
  format: 'HH:MM:SS' | 'Human' = 'HH:MM:SS'
): string {
  if (!Milliseconds) return '';

  let time = dayjs.duration(Milliseconds);
  let hours = time.hours().toString();
  let mins = time.minutes().toString();
  let seconds = time.seconds().toString().padStart(2, '0');

  if (format === 'HH:MM:SS') {
    return hours !== '0'
      ? `${hours}:${mins.padStart(2, '0')}:${seconds}`
      : `${mins}:${seconds}`;
  } else if (format === 'Human') {
    const { hour, minute } = DURATION_UNITS[activeLocale()];
    return hours !== '0'
      ? `${hours} ${hour} ${mins} ${minute}`
      : `${mins} ${minute}`;
  }
  return '';
}

export function formatDate(
  timestamp: dayjs.ConfigType,
  format = 'MMM D, YYYY'
): string {
  if (!timestamp) return '';
  return dayjs(timestamp).format(DATE_FORMATS[activeLocale()] ?? format);
}

export function formatAlbumType(
  type: string | null | undefined,
  album: { size?: number }
): string {
  if (!type) return '';
  if (type === 'EP/Single') {
    return album.size === 1 ? 'Single' : 'EP';
  } else if (type === 'Single') {
    return 'Single';
  } else if (type === '专辑') {
    return 'Album';
  } else {
    return type;
  }
}

export function resizeImage(imgUrl: unknown, size = 512): string {
  return buildArtworkURL(imgUrl, size);
}

export function formatPlayCount(
  count: number | null | undefined
): string | number {
  if (!count) return '';
  const myriad = MYRIAD_UNITS[activeLocale()];
  if (myriad) {
    // 万-based grouping: 2.32亿 / 232.1万 / 2.3万.
    if (count > 100000000) {
      return `${Math.floor((count / 100000000) * 100) / 100}${myriad.squared}`;
    }
    if (count > 100000) {
      return `${Math.floor((count / 10000) * 10) / 10}${myriad.unit}`;
    }
    if (count > 10000) {
      return `${Math.floor((count / 10000) * 100) / 100}${myriad.unit}`;
    }
    return count;
  }
  if (count > 10000000) return `${Math.floor((count / 1000000) * 10) / 10}M`;
  if (count > 1000000) return `${Math.floor((count / 1000000) * 100) / 100}M`;
  if (count > 1000) return `${Math.floor((count / 1000) * 100) / 100}K`;
  return count;
}

export function toHttps(url: string | null | undefined): string {
  if (!url) return '';
  return url.replace(/^http:/, 'https:');
}

export default {
  formatAlbumType,
  formatDate,
  formatPlayCount,
  formatTime,
  resizeImage,
  toHttps,
};
