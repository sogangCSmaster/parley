import { defineConfig } from 'astro/config'

// 소개 페이지(tolkmeet.com, GitHub Pages). JS 없는 정적 HTML로 빌드한다.
export default defineConfig({
  site: 'https://tolkmeet.com',
  trailingSlash: 'always',
  build: { format: 'directory' },
  // 디자인 토큰을 레포 루트의 design/에서 가져온다.
  vite: { server: { fs: { allow: ['..'] } } },
})
