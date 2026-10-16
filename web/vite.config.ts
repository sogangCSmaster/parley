import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  server: {
    // design/tokens.css를 디자인 시스템 원본 그대로 import하기 위해 상위 폴더 접근 허용
    fs: { allow: ['..'] },
    proxy: {
      '/api': 'http://127.0.0.1:8080',
      '/ws': { target: 'ws://127.0.0.1:8080', ws: true },
    },
  },
})
