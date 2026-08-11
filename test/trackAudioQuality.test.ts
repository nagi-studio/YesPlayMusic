import { beforeEach, describe, expect, mock, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import type { AxiosRequestConfig } from 'axios';
import type { Decoder } from '../src/api/decoders';

let musicQuality: number | 'flac' = 'flac';
const requests: AxiosRequestConfig[] = [];

interface AudioQualityCase {
  setting: number | 'flac';
  wire: string;
}

const qualityCases = (() => {
  const input: unknown = JSON.parse(
    readFileSync(
      new URL(
        '../src-tauri/sidecar/src/fixtures/audio-quality-cases.json',
        import.meta.url
      ),
      'utf8'
    )
  );
  if (
    !Array.isArray(input) ||
    !input.every(
      (entry): entry is AudioQualityCase =>
        typeof entry === 'object' &&
        entry !== null &&
        'setting' in entry &&
        (typeof entry.setting === 'number' || entry.setting === 'flac') &&
        'wire' in entry &&
        typeof entry.wire === 'string'
    )
  ) {
    throw new Error('音质 fixture 格式无效');
  }
  return input;
})();

const request = mock(
  async <TResponse>(
    config: AxiosRequestConfig,
    decoder: Decoder<TResponse>
  ): Promise<TResponse> => {
    requests.push(config);
    return decoder(
      {
        code: 200,
        data: [
          {
            id: 42,
            url: 'https://music.example/lossless.flac',
            type: 'flac',
            br: 999000,
          },
        ],
      },
      { url: config.url ?? '<unknown URL>' }
    );
  }
);

const getTestAppStore = () => ({ settings: { musicQuality } });

mock.module('@/stores/accessor', () => ({
  getAppStore: getTestAppStore,
  getAppStoreIfReady: getTestAppStore,
}));
mock.module('@/utils/request', () => ({ default: request }));

const { getMP3 } = await import('../src/api/track');

describe('音源质量契约', () => {
  beforeEach(() => {
    musicQuality = 'flac';
    requests.length = 0;
    request.mockClear();
  });

  test('五档设置都产生兼容的 wire bitrate', async () => {
    for (const qualityCase of qualityCases) {
      musicQuality = qualityCase.setting;
      requests.length = 0;

      const response = await getMP3('track-id');

      expect(requests[0]).toMatchObject({
        url: '/song/url',
        method: 'get',
        params: { id: 'track-id' },
      });
      expect(String(requests[0]?.params?.['br'])).toBe(qualityCase.wire);
      expect(response.data[0]).toMatchObject({
        id: 42,
        url: 'https://music.example/lossless.flac',
        type: 'flac',
        br: 999000,
      });
    }
  });
});
