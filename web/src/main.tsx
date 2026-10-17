import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import '../../design/tokens.css'
import './index.css'
import App from './App.tsx'
import './lib/theme'

// 새로 배포하면 예전 조각 파일이 사라져 지연 로딩이 실패한다. 새 index.html을 받도록 한 번 새로 고친다.
// 1분 안에 다시 실패하면 되풀이하지 않고 오류 화면(LoadBoundary)을 보인다.
addEventListener('vite:preloadError', () => {
  // 오프라인이면 새로 고쳐도 브라우저의 "인터넷 없음" 화면으로 바뀔 뿐이다. 오류 화면을 두어 연결되면 다시 시도하게 한다.
  if (!navigator.onLine) return
  const KEY = 'tolkmeet.chunkReload'
  try {
    if (Date.now() - Number(sessionStorage.getItem(KEY) ?? 0) < 60_000) return
    sessionStorage.setItem(KEY, String(Date.now()))
  } catch {
    // 저장소가 막혀 있으면 되풀이를 막을 수 없으니 새로 고치지 않는다.
    return
  }
  location.reload()
})

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
