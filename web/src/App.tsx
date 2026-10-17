import { lazy, Suspense, useEffect, useState, type ComponentType } from 'react'
import { InAppBrowserPage, LoadBoundary, LoadFailedPage, NotFoundPage } from './components/ErrorPages'
import { Home } from './components/Home'
import { InPerson } from './components/InPerson'
import { LiveCaptions } from './components/LiveCaptions'
import { Meeting } from './components/Meeting'
import { useI18n, type MessageKey } from './lib/i18n'
import { isInAppBrowser } from './lib/inapp'
import { parseRoomCode, usePath } from './lib/router'

/** 받지 못한 화면들을 다음에 다시 받게 바꾸는 함수 */
const failedLoads = new Set<() => void>()

/**
 * React.lazy는 조각 파일을 받지 못한 실패도 기억해 다시 받지 않는다. 그래서 실패한 화면은 LoadBoundary가 오류 화면을 보인 뒤
 * 새 lazy로 바꿔 두고, 화면을 옮겨(key가 바뀌어) 다시 그릴 때 한 번 더 받는다. 오류 화면 전에 바꾸면 Suspense가 곧바로 다시 받아 실패를 되풀이한다.
 */
function lazyRetry(load: () => Promise<{ default: ComponentType }>): ComponentType {
  const make = () =>
    lazy(() =>
      load().catch((e: unknown) => {
        failedLoads.add(reset)
        throw e
      }),
    )
  const reset = () => (current = make())
  let current = make()
  return function Retry() {
    const Page = current
    return <Page />
  }
}

function retryFailedLoads() {
  failedLoads.forEach((reset) => reset())
  failedLoads.clear()
}

// 회의와 상관없는 화면(로그인·계정·회의 목록·결제)은 필요할 때 받는다. 링크로 회의에 들어오는 손님은 받지 않는다.
const Login = lazyRetry(() => import('./components/AuthPages').then((m) => ({ default: m.Login })))
const Signup = lazyRetry(() => import('./components/AuthPages').then((m) => ({ default: m.Signup })))
const Verify = lazyRetry(() => import('./components/AuthPages').then((m) => ({ default: m.Verify })))
const Forgot = lazyRetry(() => import('./components/AuthPages').then((m) => ({ default: m.Forgot })))
const Reset = lazyRetry(() => import('./components/AuthPages').then((m) => ({ default: m.Reset })))
const Account = lazyRetry(() => import('./components/AccountPages').then((m) => ({ default: m.Account })))
const Security = lazyRetry(() => import('./components/AccountPages').then((m) => ({ default: m.Security })))
const Meetings = lazyRetry(() => import('./components/MeetingsPage').then((m) => ({ default: m.MeetingsPage })))
const Checkout = lazyRetry(() => import('./components/BillingPages').then((m) => ({ default: m.Checkout })))
const PaidDone = lazyRetry(() => import('./components/BillingPages').then((m) => ({ default: m.PaidDone })))

const PAGES: Record<string, React.ComponentType> = {
  '/login': Login,
  '/signup': Signup,
  '/verify': Verify,
  '/forgot': Forgot,
  '/reset': Reset,
  '/account': Account,
  '/account/security': Security,
  '/meetings': Meetings,
  '/checkout': Checkout,
  '/billing/done': PaidDone,
}

/** 탭 제목(화면 이름 · Tolkmeet). 지연 로딩 전에 정하므로 문구는 첫 번들(i18n.ts)에 둔다. */
const TITLES: Record<string, MessageKey> = {
  '/login': 'auth.hostLogin',
  '/signup': 'auth.createAccount',
  '/verify': 'title.verify',
  '/forgot': 'title.forgot',
  '/reset': 'title.forgot',
  '/account': 'nav.account',
  '/account/security': 'title.security',
  '/meetings': 'nav.meetings',
  '/checkout': 'title.checkout',
  '/billing/done': 'title.paid',
}

export default function App() {
  const { t } = useI18n()
  const path = usePath()
  const code = parseRoomCode(path)
  // "다시 참여" 시 Meeting을 새로 마운트해 미디어·연결을 처음부터 다시 잡는다.
  const [session, setSession] = useState(0)

  const route = path.replace(/\/$/, '') || '/'
  const titleKey = TITLES[route]
  const title = titleKey ? `${t(titleKey)} · Tolkmeet` : 'Tolkmeet'
  useEffect(() => {
    document.title = title
  }, [title])

  const Page = PAGES[route]
  if (Page) {
    return (
      <LoadBoundary key={route} fallback={<LoadFailedPage />} onError={retryFailedLoads}>
        <Suspense fallback={<div className="shell" />}>
          <Page />
        </Suspense>
      </LoadBoundary>
    )
  }
  if (path === '/') return <Home />
  if (!code) return <NotFoundPage />
  // 회의 화면은 카메라·마이크가 필요하다. 메신저 앱 안의 브라우저면 장치를 요청하기 전에 다른 브라우저로 안내한다.
  if (isInAppBrowser()) return <InAppBrowserPage />
  if (path.startsWith('/talk/') && code) return <InPerson key={code} code={code} />
  if (path.startsWith('/live/') && code) return <LiveCaptions key={code} code={code} />
  return <Meeting key={`${code}:${session}`} code={code} onRejoin={() => setSession((s) => s + 1)} />
}
