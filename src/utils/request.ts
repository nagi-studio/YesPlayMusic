import router from '@/router';
import { doLogout, getCookie } from '@/utils/auth';
import axios from 'axios';
import type { AxiosError, AxiosRequestConfig } from 'axios';
import { isDesktopRuntime } from '@/utils/runtime';
import defaultStorageState from '@/stores/defaults';
import { decodeSettingsState, readStoredJson } from '@/utils/persistedState';
import { handleNcmSessionExpiry } from '@/utils/sessionExpiry';
import type { Decoder } from '@/api/decoders';

const baseURL = import.meta.env['VUE_APP_NETEASE_API_URL'] ?? '';

const service = axios.create({
  baseURL,
  withCredentials: true,
  timeout: 15000,
});

service.interceptors.request.use(function (config) {
  if (!config.params) config.params = {};
  if (baseURL.length) {
    if (
      baseURL[0] !== '/' &&
      !isDesktopRuntime &&
      getCookie('MUSIC_U') !== null
    ) {
      config.params.cookie = `MUSIC_U=${getCookie('MUSIC_U')};`;
    }
  } else {
    console.error("You must set up the baseURL in the service's config");
  }

  if (!isDesktopRuntime && !config.url?.includes('/login')) {
    config.params.realIP = '211.161.244.70';
  }

  // Apply the user-selected real IP at the API boundary.
  const settings = decodeSettingsState(
    readStoredJson(localStorage, 'settings'),
    defaultStorageState.settings
  );
  const enableRealIP = settings.enableRealIP;
  const realIP = settings.realIP;
  if (import.meta.env['VUE_APP_REAL_IP']) {
    config.params.realIP = import.meta.env['VUE_APP_REAL_IP'];
  } else if (enableRealIP) {
    config.params.realIP = realIP;
  }

  const proxy = settings.proxyConfig;
  if (proxy && ['HTTP', 'HTTPS'].includes(proxy.protocol)) {
    config.params.proxy = `${proxy.protocol}://${proxy.server}:${proxy.port}`;
  }

  return config;
});

service.interceptors.response.use(
  response => {
    const res = response.data;
    return res;
  },
  async (error: unknown) => {
    let data: unknown;
    if (error === 'TypeError: baseURL is undefined') {
      data = error;
      console.error("You must set up the baseURL in the service's config");
    } else if (axios.isAxiosError(error)) {
      data = (error as AxiosError).response?.data;
    }

    if (
      handleNcmSessionExpiry(data, {
        loginRoute: isDesktopRuntime ? 'loginAccount' : 'login',
        logout: doLogout,
        navigate: name => {
          void router.push({ name });
        },
      })
    ) {
      console.warn('Token has expired. Logout now!');
    }
    return Promise.reject(error);
  }
);

export default async function request<TResponse>(
  config: AxiosRequestConfig,
  decoder: Decoder<TResponse>
): Promise<TResponse> {
  // The interceptor only unwraps data; the endpoint decoder owns validation.
  const response: unknown = await service.request<unknown>(config);
  return decoder(response, { url: config.url ?? '<unknown URL>' });
}
