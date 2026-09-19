import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import vueJsx from '@vitejs/plugin-vue-jsx'
import UnoCSS from 'unocss/vite'

// 开发态把 /api 代理到本地 axum 服务端，生产态由 axum 直接托管 dist/，
// 因此前端代码里所有请求都可以用同源相对路径（BASE_URL = ""）。
//
// 端口可用环境变量覆盖，方便和已有服务错开。
// @ts-expect-error process is a nodejs global
const apiTarget = process.env.WNACG_API_TARGET ?? 'http://127.0.0.1:3030'
// @ts-expect-error process is a nodejs global
const devPort = Number(process.env.WNACG_DEV_PORT ?? 5173)

export default defineConfig({
  plugins: [vue(), vueJsx(), UnoCSS()],

  build: {
    // 产物给 axum 的 ServeDir 托管
    outDir: 'dist',
    emptyOutDir: true,
  },

  server: {
    port: devPort,
    strictPort: true,
    host: true,
    proxy: {
      // HTTP API + WebSocket 升级（/api/ws）
      '/api': {
        target: apiTarget,
        changeOrigin: true,
        ws: true,
      },
    },
  },
})