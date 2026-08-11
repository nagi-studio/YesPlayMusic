import { defineConfig, loadEnv } from 'vite';
import vue from '@vitejs/plugin-vue';
import { createSvgIconsPlugin } from 'vite-plugin-svg-icons';
import path from 'node:path';
import { rendererDependencyManifestPlugin } from './scripts/build-app-compliance.mjs';

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), '');
  const isTauri = process.env.IS_TAURI === 'true';

  return {
    envPrefix: ['VITE_', 'VUE_APP_', 'IS_'],
    resolve: {
      alias: [
        {
          find: /^~@\//,
          replacement: path.resolve(import.meta.dirname, 'src') + '/',
        },
        {
          find: /^@\//,
          replacement: path.resolve(import.meta.dirname, 'src') + '/',
        },
      ],
      extensions: ['.mjs', '.js', '.ts', '.tsx', '.json', '.vue'],
    },
    plugins: [
      vue(),
      createSvgIconsPlugin({
        iconDirs: [path.resolve(import.meta.dirname, 'src/assets/icons')],
        symbolId: 'icon-[name]',
      }),
      ...(isTauri ? [rendererDependencyManifestPlugin()] : []),
    ],
    define: isTauri
      ? {
          'import.meta.env.VUE_APP_NETEASE_API_URL': JSON.stringify('/api'),
        }
      : undefined,
    server: {
      host: isTauri ? '127.0.0.1' : undefined,
      port: isTauri ? 1420 : Number(env.DEV_SERVER_PORT) || 8080,
      strictPort: isTauri,
      proxy: {
        '^/api': {
          target: isTauri ? 'http://127.0.0.1:12754' : 'http://localhost:3000',
          changeOrigin: true,
          rewrite: p => p.replace(/^\/api/, ''),
        },
      },
    },
    build: {
      sourcemap: false,
      outDir: isTauri ? 'dist-tauri' : 'dist',
      rollupOptions: {
        output: {
          manualChunks: {
            'audio-vendor': ['howler', 'vue-slider-component'],
            'data-vendor': ['axios', 'dexie'],
            'vue-vendor': ['vue', 'vue-i18n', 'vue-router', 'pinia'],
          },
        },
      },
    },
  };
});
