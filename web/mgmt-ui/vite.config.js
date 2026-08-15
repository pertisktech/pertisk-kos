import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  define: {
    'import.meta.env.VITE_APP_VERSION': JSON.stringify(
      process.env.VITE_APP_VERSION || process.env.VERSION || '0.1.0'
    ),
  },
  server: {
    port: 5173,
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:8080',
        changeOrigin: true,
        // Long-lived SSE (`/api/events`) must not hit default proxy timeouts.
        timeout: 0,
        proxyTimeout: 0,
      },
    },
  },
  worker: {
    format: 'es',
  },
  optimizeDeps: {
    include: ['monaco-editor', 'monaco-yaml'],
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
})
