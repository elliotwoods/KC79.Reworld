import react from '@vitejs/plugin-react';
import { resolve } from 'node:path';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  /**
   * The framework's `brand/` directory, served as-is.
   *
   * Load-bearing and the single easiest thing to omit. The cue tones this bench uses to say
   * "pass", "fail" and "the soak stopped early" are `brand/system-sounds/*.wav`, served at
   * `/system-sounds/...` only because a `publicDir` points at them. The failure mode is silent
   * in the most literal sense: the page runs, the tests run, and nobody hears anything -- which
   * matters most for exactly the runs nobody is watching.
   */
  publicDir: resolve(__dirname, '../third_party/av-frameworks/brand'),
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    sourcemap: true,
  },
  server: {
    // `npm run dev` only. The built page is served by the app itself on the same origin.
    proxy: {
      '/bus': { target: 'ws://127.0.0.1:8770', ws: true },
      '/api': { target: 'http://127.0.0.1:8770' },
    },
  },
  plugins: [react()],
  resolve: {
    // npm represents the pinned `file:` dependency as a link, and here that path also runs
    // through a directory junction to PortalFlasher's framework checkout. Keeping the
    // package-facing location lets the framework's React peer imports resolve to this
    // application's single React copy instead of a second one.
    preserveSymlinks: true,
    dedupe: ['react', 'react-dom'],
  },
  test: {
    include: ['src/**/*.test.{ts,tsx}'],
  },
});
