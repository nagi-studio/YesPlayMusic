import { createPinia, setActivePinia } from 'pinia';
import { watch } from 'vue';
import { useAppStore } from './app';
import { registerAppStore } from './accessor';
import { changeAppearance, changeThemeColor } from '@/utils/common';
import { mountPlayerState } from '@/utils/playerState';
import { isDesktopRuntime } from '@/utils/runtime';
import { sendDesktop } from '@/services/desktopTransport';
import { isLastfmCallbackLocation } from '@/services/lastfmAuth';
import { syncDesktopSettings } from '@/services/desktopSettings';
import { normalizePersistedLocale, resolveLocale } from '@/locale/catalog';

const isLastfmCallback = isLastfmCallbackLocation(window.location);

export const pinia = createPinia();
setActivePinia(pinia);

export const appStore = useAppStore(pinia);
registerAppStore(appStore);

watch(
  () => [appStore.settings, appStore.data],
  () => {
    localStorage.setItem('settings', JSON.stringify(appStore.settings));
    localStorage.setItem('data', JSON.stringify(appStore.data));
  },
  { deep: true, flush: 'sync' }
);

if (appStore.settings.lang === null) {
  appStore.settings.lang = resolveLocale(navigator.language);
} else {
  // Drop a locale that no longer ships instead of running with empty messages.
  appStore.settings.lang = normalizePersistedLocale(appStore.settings.lang);
}

appStore.$onAction(({ name, after }) => {
  if (!isDesktopRuntime || isLastfmCallback || name !== 'updateSettings')
    return;
  after(() => {
    void syncDesktopSettings(appStore.settings);
  });
});

changeAppearance(appStore.settings.appearance);
changeThemeColor(appStore.settings.themeColor);

window
  .matchMedia('(prefers-color-scheme: dark)')
  .addEventListener('change', () => {
    if (appStore.settings.appearance === 'auto') {
      changeAppearance(appStore.settings.appearance);
      changeThemeColor(appStore.settings.themeColor);
    }
  });

if (!isLastfmCallback) {
  window.yesplaymusic ??= {};
  mountPlayerState(appStore, appStore.player, window.yesplaymusic);
}

if (isDesktopRuntime && !isLastfmCallback) {
  watch(
    () => ({
      playing: appStore.player.playing,
      likedCurrentTrack: appStore.player.isCurrentTrackLiked,
      positionSeconds: appStore.player.progress,
      repeatMode: appStore.player.repeatMode,
      shuffle: appStore.player.shuffle,
    }),
    state => {
      void sendDesktop('mediaState', state);
    },
    { immediate: true }
  );
}

export default pinia;
