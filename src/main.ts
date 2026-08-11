import { createApp } from 'vue';
import '@/assets/css/global.scss';
import NProgress from 'nprogress';
import '@/assets/css/nprogress.css';
import { migrateLegacyDesktopSettings } from '@/services/legacyDataMigration';
import {
  migrateLegacyRendererData,
  type LegacyMigrationNotice,
} from '@/services/legacyRendererMigration';
import { isDesktopRuntime } from '@/utils/runtime';
import { purgeLegacyDesktopAuthStorage } from '@/utils/authStorage';
import { shouldOpenLibraryOnStartup } from '@/services/startupNavigation';
import type { MessageKey } from '@/locale/catalog';

window.resetApp = () => {
  localStorage.clear();
  indexedDB.deleteDatabase('yesplaymusic');
  document.cookie.split(';').forEach(function (c) {
    document.cookie = c
      .replace(/^ +/, '')
      .replace(/=.*/, '=;expires=' + new Date().toUTCString() + ';path=/');
  });
  return '已重置应用，请刷新页面（按Ctrl/Command + R）';
};
console.log(
  '如出现问题，可尝试在本页输入 %cresetApp()%c 然后按回车重置应用。',
  'background: #eaeffd;color:#335eea;padding: 4px 6px;border-radius:3px;',
  'background:unset;color:unset;'
);

NProgress.configure({ showSpinner: false, trickleSpeed: 100 });

function showLegacyMigrationRetry(): void {
  const root = document.querySelector<HTMLElement>('#app');
  if (!root) return;
  const isChinese = navigator.language.toLowerCase().startsWith('zh');
  const title = document.createElement('h1');
  const message = document.createElement('p');
  const retry = document.createElement('button');
  const skip = document.createElement('button');
  title.textContent = isChinese
    ? '旧版数据暂时无法读取'
    : 'Unable to read Electron data';
  message.textContent = isChinese
    ? '请退出旧版 YesPlayMusic 后重试。继续会跳过旧账号和播放状态导入。'
    : 'Quit the Electron version and retry. Continuing skips its account and playback state.';
  retry.textContent = isChinese ? '重试' : 'Retry';
  skip.textContent = isChinese ? '跳过导入' : 'Skip import';
  retry.addEventListener('click', () => window.location.reload());
  skip.addEventListener('click', () => {
    localStorage.setItem('legacyElectronRendererImportedV1', 'skipped-by-user');
    window.location.reload();
  });
  root.replaceChildren(title, message, retry, skip);
  root.style.cssText =
    'max-width:520px;margin:15vh auto;padding:32px;font:16px/1.6 system-ui;text-align:center';
  for (const button of [retry, skip]) {
    button.style.cssText = 'margin:8px;padding:8px 16px;cursor:pointer';
  }
}

async function bootstrap() {
  const rendererMigration = await migrateLegacyRendererData();
  if (rendererMigration?.status === 'retry-required') {
    showLegacyMigrationRetry();
    return;
  }
  await migrateLegacyDesktopSettings();
  purgeLegacyDesktopAuthStorage(localStorage, isDesktopRuntime);
  const { appStore, default: pinia } = await import('./stores');
  const [
    { default: App },
    { default: router },
    { default: i18n },
    { default: SvgIcons },
    { default: filters },
    { copyText },
    { dailyTask },
  ] = await Promise.all([
    import('./App.vue'),
    import('./router'),
    import('@/locale'),
    import('@/assets/icons'),
    import('@/utils/filters'),
    import('@/utils/clipboard'),
    import('@/utils/common'),
  ]);

  dailyTask();
  const app = createApp(App);
  app.config.globalProperties.$filters = filters;
  app.config.globalProperties.$copyText = copyText;
  app.use(i18n);
  app.use(pinia);
  app.use(router);
  app.use(SvgIcons);
  await router.isReady();
  if (
    shouldOpenLibraryOnStartup(
      appStore.settings.showLibraryDefault,
      router.currentRoute.value.name
    )
  ) {
    await router.replace({ name: 'library' });
  }
  app.mount('#app');
  if (rendererMigration?.status === 'completed' && rendererMigration.notice) {
    const noticeKeys = Object.freeze({
      complete: 'toast.legacyMigrationComplete',
      'partial-import': 'toast.legacyMigrationPartial',
      'cache-not-migrated': 'toast.legacyMigrationCache',
      'login-required': 'toast.legacyMigrationLogin',
      'login-and-cache': 'toast.legacyMigrationLoginAndCache',
    } as const satisfies Record<LegacyMigrationNotice, MessageKey>);
    appStore.showToast(i18n.t(noticeKeys[rendererMigration.notice]));
  }
}

void bootstrap();
