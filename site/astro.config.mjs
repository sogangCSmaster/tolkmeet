import { defineConfig } from 'astro/config'

// 소개 페이지(tolkmeet.com, GitHub Pages). 정적 HTML로 빌드한다. 첫 화면의 JS는 기능 카드의 인라인 로더(1KB 미만)뿐이고,
// 카드 장면 모듈(src/motion/)은 카드에 처음 다가갈 때 받는다.
export default defineConfig({
  site: 'https://tolkmeet.com',
  trailingSlash: 'always',
  build: { format: 'directory' },
  // 디자인 토큰을 레포 루트의 design/에서 가져온다.
  vite: { server: { fs: { allow: ['..'] } } },
})
