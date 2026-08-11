import { describe, expect, test } from 'bun:test';
import {
  comparableManifestRoute,
  extractProductionNcmRoutes,
  loadSidecarRouteManifest,
  validateSidecarRouteManifest,
  verifySidecarRouteManifest,
} from '../scripts/sidecar-route-manifest.mjs';

describe('Rust Sidecar NCM route manifest', () => {
  test('与生产 request AST 和 Node adapter 集合完全相等', () => {
    const { manifest, actual } = verifySidecarRouteManifest();
    const declared = manifest
      .map(comparableManifestRoute)
      .sort((left, right) => left.path.localeCompare(right.path));

    expect(actual).toHaveLength(57);
    expect(manifest).toHaveLength(57);
    expect(declared).toEqual(actual);
    expect(new Set(actual.map(route => route.path)).size).toBe(57);
  });

  test('helper 生成的两条密码登录调用也由 AST 推导', () => {
    const loginRoutes = extractProductionNcmRoutes()
      .filter(
        route => route.path === '/login' || route.path === '/login/cellphone'
      )
      .map(route => ({
        method: route.method,
        path: route.path,
        requestBuilder: route.requestBuilder,
      }));

    expect(loginRoutes).toEqual([
      {
        method: 'POST',
        path: '/login',
        requestBuilder: 'src/api/auth.ts#loginWithEmail',
      },
      {
        method: 'POST',
        path: '/login/cellphone',
        requestBuilder: 'src/api/auth.ts#loginWithPhone',
      },
    ]);
  });

  test('POST 集合来自生产调用，且全部禁止跨后端自动重放', () => {
    const actualPosts = extractProductionNcmRoutes()
      .filter(route => route.method === 'POST')
      .map(route => route.path);
    const manifest = validateSidecarRouteManifest(loadSidecarRouteManifest());
    const declaredPosts = manifest
      .filter(route => route.method === 'POST')
      .map(route => route.path);

    expect(actualPosts).toHaveLength(15);
    expect(declaredPosts).toEqual(actualPosts);
    expect(
      manifest
        .filter(route => route.method === 'POST')
        .every(route => route.idempotent === false)
    ).toBe(true);
  });

  test('/api 只放行生产 getCloudLyric 使用的 eapi builder', () => {
    const actualApi = extractProductionNcmRoutes().find(
      route => route.path === '/api'
    );
    const manifestApi = validateSidecarRouteManifest(
      loadSidecarRouteManifest()
    ).find(route => route.path === '/api');
    if (!actualApi || !manifestApi) throw new Error('缺少 /api route');

    expect(actualApi.apiForward).toEqual({
      allowedPaths: ['/api/cloud/lyric/get'],
      crypto: 'eapi',
    });
    expect(manifestApi.apiForward).toEqual(actualApi.apiForward);
    expect(
      extractProductionNcmRoutes().filter(
        route => route.apiForward !== undefined
      )
    ).toHaveLength(1);
  });

  test('每条 comparator 都是非空稳定字段，不允许全量 JSON', () => {
    const manifest = validateSidecarRouteManifest(loadSidecarRouteManifest());

    expect(manifest.every(route => route.comparator.length > 0)).toBe(true);
    expect(
      manifest.every(route =>
        route.comparator.every(
          field => !['raw', 'json', 'response', 'body', 'data'].includes(field)
        )
      )
    ).toBe(true);

    const invalid = structuredClone(manifest);
    const firstRoute = invalid[0];
    if (!firstRoute) throw new Error('manifest 不能为空');
    firstRoute.comparator = ['data'];
    expect(() => validateSidecarRouteManifest(invalid)).toThrow('全量 JSON');
  });
});
