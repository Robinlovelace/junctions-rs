import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import wasm from 'vite-plugin-wasm';

export default defineConfig({
  base: '/junctions-rs/',
  plugins: [svelte(), wasm()],
  build: {
    rollupOptions: {
      output: {
        manualChunks: { maplibre: ['maplibre-gl'] }
      }
    }
  }
});
