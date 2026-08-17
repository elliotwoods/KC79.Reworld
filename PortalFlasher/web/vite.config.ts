import react from '@vitejs/plugin-react';
import { resolve } from 'node:path';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  /**
   * The framework's `brand/` directory, served as-is.
   *
   * This is not decoration and it is the single easiest thing to omit: the pass and fail tones
   * this rig depends on are `brand/system-sounds/*.wav`, served at `/system-sounds/…` only
   * because a `publicDir` points at them. `av-frameworks`' own vite config does this; a
   * downstream app copied from `av-laserworkbench-app` does not, and the failure is silent in
   * the most literal sense — the page runs, the rig flashes boards, and nobody hears anything.
   */
  publicDir: resolve(__dirname, '../third_party/av-frameworks/brand'),
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    sourcemap: true,
  },
  server: {
    proxy: {
      '/bus': { target: 'ws://127.0.0.1:8761', ws: true },
    },
  },
  plugins: [react()],
  resolve: {
    // npm represents the pinned file dependency as a link. Keeping that package-facing location
    // lets its React peer imports resolve to this application's one React copy.
    preserveSymlinks: true,
    dedupe: ['react', 'react-dom'],
  },
  test: {
    include: ['src/**/*.test.{ts,tsx}'],
  },
});
