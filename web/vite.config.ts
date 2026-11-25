import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

// The production bundle is written into the Python package so FastAPI (and the Tauri
// shell via frontendDist) can serve it; see ADR 0003 / 0010.
export default defineConfig({
  base: '/',
  plugins: [react()],
  build: {
    outDir: '../app/web_dist',
    emptyOutDir: true,
    // three.js is loaded lazily as its own chunk; it is larger than Vite's default hint.
    chunkSizeWarningLimit: 1024,
  },
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      '/api': { target: 'http://127.0.0.1:8000', changeOrigin: false },
      '/docs': { target: 'http://127.0.0.1:8000', changeOrigin: false },
      '/openapi.json': { target: 'http://127.0.0.1:8000', changeOrigin: false },
    },
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
    restoreMocks: true,
  },
});
