import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// During `npm run dev`, API calls go to a running Hearthgate. Point it
// elsewhere with HEARTHGATE_URL=https://192.168.1.10:7380.
const target = process.env.HEARTHGATE_URL ?? 'https://127.0.0.1:7380';

export default defineConfig({
  plugins: [svelte()],
  build: {
    // Hearthgate's CSP allows no inline scripts or styles, so keep every
    // asset a separate file.
    assetsInlineLimit: 0,
    sourcemap: false,
  },
  server: {
    proxy: {
      '/api': { target, changeOrigin: true, secure: false },
      '/metrics': { target, changeOrigin: true, secure: false },
    },
  },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.ts'],
    setupFiles: ['src/test-setup.ts'],
  },
  resolve: process.env.VITEST ? { conditions: ['browser'] } : undefined,
});
