import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// The Rust binary serves `dist/` as-is, so asset URLs have to be root-relative
// and the output has to land somewhere the Dockerfile can copy wholesale.
//
// `server.proxy` is what makes `npm run dev` useful: the API stays on the Rust
// process (port 8080) while Vite serves the UI with hot reload, so you develop
// against real crawled data rather than fixtures. `/events` is in there too —
// it is an SSE stream, and Vite proxies it without buffering.
export default defineConfig({
  plugins: [react()],
  base: '/',
  build: { outDir: 'dist', emptyOutDir: true, sourcemap: false },
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://127.0.0.1:8080',
      '/events': 'http://127.0.0.1:8080',
      '/settings/resume': 'http://127.0.0.1:8080',
    },
  },
})
