import { isAccountLoggedIn } from './auth';
import { refreshCookie } from '@/api/auth';
import dayjs from 'dayjs';
import { getAppStore } from '@/stores/accessor';
import { normalizePersistedLocale } from '@/locale/catalog';
import type { LocaleCode } from '@/locale/catalog';
import type { Track, TrackPrivilege } from '@/types/domain';

interface ThemeColors {
  primary: string;
  primaryBg: string;
  primaryBgForTransparent: string;
  primaryGradient: string;
}

type Appearance = 'light' | 'dark';

const themeColorPresets: Record<string, Record<Appearance, ThemeColors>> = {
  default: {
    light: {
      primary: '#335eea',
      primaryBg: '#eaeffd',
      primaryBgForTransparent: 'rgba(189, 207, 255, 0.28)',
      primaryGradient: '#335eea',
    },
    dark: {
      primary: '#335eea',
      primaryBg: '#bbcdff',
      primaryBgForTransparent: 'rgba(255, 255, 255, 0.12)',
      primaryGradient: '#335eea',
    },
  },
  sunset: {
    light: {
      primary: '#dd2476',
      primaryBg: '#ffe1ed',
      primaryBgForTransparent: 'rgba(221, 36, 118, 0.2)',
      primaryGradient: 'linear-gradient(135deg, #ff512f 0%, #dd2476 100%)',
    },
    dark: {
      primary: '#ff6fa5',
      primaryBg: '#3d182b',
      primaryBgForTransparent: 'rgba(255, 111, 165, 0.24)',
      primaryGradient: 'linear-gradient(135deg, #ff512f 0%, #dd2476 100%)',
    },
  },
  ocean: {
    light: {
      primary: '#0072ff',
      primaryBg: '#e0f2ff',
      primaryBgForTransparent: 'rgba(0, 114, 255, 0.18)',
      primaryGradient: 'linear-gradient(135deg, #00c6ff 0%, #0072ff 100%)',
    },
    dark: {
      primary: '#5aa2ff',
      primaryBg: '#15233a',
      primaryBgForTransparent: 'rgba(90, 162, 255, 0.24)',
      primaryGradient: 'linear-gradient(135deg, #00c6ff 0%, #0072ff 100%)',
    },
  },
  forest: {
    light: {
      primary: '#56ab2f',
      primaryBg: '#e6f7da',
      primaryBgForTransparent: 'rgba(86, 171, 47, 0.18)',
      primaryGradient: 'linear-gradient(135deg, #56ab2f 0%, #a8e063 100%)',
    },
    dark: {
      primary: '#7cd957',
      primaryBg: '#1b3020',
      primaryBgForTransparent: 'rgba(124, 217, 87, 0.24)',
      primaryGradient: 'linear-gradient(135deg, #56ab2f 0%, #a8e063 100%)',
    },
  },
};

export function changeThemeColor(
  themeColor: string | null | undefined,
  appearance?: string | null
): void {
  if (typeof document === 'undefined') return;
  const resolvedAppearance =
    appearance || document.body?.getAttribute('data-theme') || 'light';
  const resolvedTheme =
    themeColor && themeColorPresets[themeColor] ? themeColor : 'default';
  const preset =
    themeColorPresets[resolvedTheme] ?? themeColorPresets['default'];
  if (!preset) return;
  const theme =
    preset[resolvedAppearance === 'dark' ? 'dark' : 'light'] ?? preset.light;
  if (!theme) return;
  const target = document.body || document.documentElement;
  target.style.setProperty('--color-primary', theme.primary);
  target.style.setProperty('--color-primary-bg', theme.primaryBg);
  target.style.setProperty(
    '--color-primary-bg-for-transparent',
    theme.primaryBgForTransparent
  );
  target.style.setProperty(
    '--color-primary-gradient',
    theme.primaryGradient || theme.primary
  );
}

export function isTrackPlayable(track: Track): {
  playable: boolean;
  reason: string;
} {
  const result = {
    playable: true,
    reason: '',
  };
  if ((track.privilege?.pl ?? 0) > 0) {
    return result;
  }
  // cloud storage judgement logic
  if (isAccountLoggedIn() && track?.privilege?.cs) {
    return result;
  }
  if (track.fee === 1 || track.privilege?.fee === 1) {
    if (isAccountLoggedIn() && getAppStore().data.user.vipType === 11) {
      result.playable = true;
    } else {
      result.playable = false;
      result.reason = 'VIP Only';
    }
  } else if (track.fee === 4 || track.privilege?.fee === 4) {
    result.playable = false;
    result.reason = '付费专辑';
  } else if (
    track.noCopyrightRcmd !== null &&
    track.noCopyrightRcmd !== undefined
  ) {
    result.playable = false;
    result.reason = '无版权';
  } else if ((track.privilege?.st ?? 0) < 0 && isAccountLoggedIn()) {
    result.playable = false;
    result.reason = '已下架';
  }
  return result;
}

export function mapTrackPlayableStatus<T extends Track>(
  tracks: T[],
  privileges: TrackPrivilege[] = []
): T[] {
  return tracks.map(t => {
    const privilege = privileges.find(item => item.id === t.id) ?? {};
    if (t.privilege) {
      Object.assign(t.privilege, privilege);
    } else {
      t.privilege = privilege;
    }
    const result = isTrackPlayable(t);
    t.playable = result.playable;
    t.reason = result.reason;
    return t;
  });
}

export function randomNum(minNum: number, maxNum?: number): number {
  switch (arguments.length) {
    case 1:
      return Math.trunc(Math.random() * minNum + 1);
    case 2:
      return Math.trunc(
        Math.random() * ((maxNum ?? minNum) - minNum + 1) + minNum
      );
    default:
      return 0;
  }
}

export function shuffleAList(
  list: Array<{ id: number; sort?: number }>
): Record<number, number | undefined> {
  const sortsList = list.map(track => track.sort);
  for (let i = 1; i < sortsList.length; i++) {
    const random = Math.floor(Math.random() * (i + 1));
    [sortsList[i], sortsList[random]] = [sortsList[random], sortsList[i]];
  }
  const newSorts: Record<number, number | undefined> = {};
  list.forEach(track => {
    newSorts[track.id] = sortsList.pop();
  });
  return newSorts;
}

export function throttle<TThis, TArgs extends unknown[]>(
  fn: (this: TThis, ...args: TArgs) => void,
  time: number
): (this: TThis, ...args: TArgs) => void {
  let isRun = false;
  return function (this: TThis, ...args: TArgs) {
    if (isRun) return;
    isRun = true;
    fn.apply(this, args);
    setTimeout(() => {
      isRun = false;
    }, time);
  };
}

export function updateHttps(url: string | null | undefined): string {
  if (!url) return '';
  return url.replace(/^http:/, 'https:');
}

export function dailyTask(): void {
  const appStore = getAppStore();
  let lastDate = appStore.data.lastRefreshCookieDate;
  if (
    isAccountLoggedIn() &&
    (lastDate === undefined || lastDate !== dayjs().date())
  ) {
    console.debug('[debug][common.js] execute dailyTask');
    refreshCookie().then(() => {
      console.debug('[debug][common.js] 刷新cookie');
      appStore.updateData({
        key: 'lastRefreshCookieDate',
        value: dayjs().date(),
      });
    });
  }
}

export function changeAppearance(appearance?: string | null): void {
  if (appearance === 'auto' || appearance === undefined) {
    appearance = window.matchMedia('(prefers-color-scheme: dark)').matches
      ? 'dark'
      : 'light';
  }
  const resolvedAppearance = appearance ?? 'light';
  document.body.setAttribute('data-theme', resolvedAppearance);
  document
    .querySelector('meta[name="theme-color"]')
    ?.setAttribute('content', resolvedAppearance === 'dark' ? '#222' : '#fff');
}

export function splitSoundtrackAlbumTitle(title: string): {
  title: string;
  subtitle: string;
} {
  const keywords = [
    'Music from the Original Motion Picture Score',
    'The Original Motion Picture Soundtrack',
    'Original MGM Motion Picture Soundtrack',
    'Complete Original Motion Picture Score',
    'Original Music From The Motion Picture',
    'Music From The Disney+ Original Movie',
    'Original Music From The Netflix Film',
    'Original Score to the Motion Picture',
    'Original Motion Picture Soundtrack',
    'Soundtrack from the Motion Picture',
    'Original Television Soundtrack',
    'Original Motion Picture Score',
    'Music From the Motion Picture',
    'Music From The Motion Picture',
    'Complete Motion Picture Score',
    'Music from the Motion Picture',
    'Original Videogame Soundtrack',
    'La Bande Originale du Film',
    'Music from the Miniseries',
    'Bande Originale du Film',
    'Die Original Filmmusik',
    'Original Soundtrack',
    'Complete Score',
    'Original Score',
  ];
  for (let keyword of keywords) {
    if (title.includes(keyword) === false) continue;
    return {
      title: title
        .replace(`(${keyword})`, '')
        .replace(`: ${keyword}`, '')
        .replace(`[${keyword}]`, '')
        .replace(`- ${keyword}`, '')
        .replace(`${keyword}`, ''),
      subtitle: keyword,
    };
  }
  return {
    title: title,
    subtitle: '',
  };
}

export function splitAlbumTitle(title: string): {
  title: string;
  subtitle: string;
} {
  const keywords = [
    'Bonus Tracks Edition',
    'Complete Edition',
    'Deluxe Edition',
    'Deluxe Version',
    'Tour Edition',
  ];
  for (let keyword of keywords) {
    if (title.includes(keyword) === false) continue;
    return {
      title: title
        .replace(`(${keyword})`, '')
        .replace(`: ${keyword}`, '')
        .replace(`[${keyword}]`, '')
        .replace(`- ${keyword}`, '')
        .replace(`${keyword}`, ''),
      subtitle: keyword,
    };
  }
  return {
    title: title,
    subtitle: '',
  };
}

const BYTE_UNITS: Record<LocaleCode, string> = {
  en: ' Bytes',
  ja: ' バイト',
  'zh-CN': '字节',
  'zh-TW': '位元組',
};

export function bytesToSize(bytes: number): string {
  const marker = 1024; // Change to 1000 if required
  const decimal = 2; // Change as required
  const kiloBytes = marker;
  const megaBytes = marker * marker;
  const gigaBytes = marker * marker * marker;

  // The unit below one kilobyte is the only localised one; KB/MB/GB are universal.
  const byteUnit =
    BYTE_UNITS[normalizePersistedLocale(getAppStore().settings.lang)];

  if (bytes < kiloBytes) return bytes + byteUnit;
  else if (bytes < megaBytes)
    return (bytes / kiloBytes).toFixed(decimal) + ' KB';
  else if (bytes < gigaBytes)
    return (bytes / megaBytes).toFixed(decimal) + ' MB';
  else return (bytes / gigaBytes).toFixed(decimal) + ' GB';
}

export function formatTrackTime(value: number | null | undefined): string {
  if (!value) return '';
  const min = ~~(value / 60);
  const sec = (~~(value % 60)).toString().padStart(2, '0');
  return `${min}:${sec}`;
}
