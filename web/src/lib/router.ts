import { useEffect, useRef, useSyncExternalStore } from 'react'
import { useAuth, type AuthState } from './auth'

const subscribe = (cb: () => void) => {
  addEventListener('popstate', cb)
  return () => removeEventListener('popstate', cb)
}

export function usePath(): string {
  return useSyncExternalStore(subscribe, () => location.pathname)
}

/** 경로와 쿼리 모두(쿼리만 바뀌어도 다시 그리려고). */
export function useLocationKey(): string {
  return useSyncExternalStore(subscribe, () => location.pathname + location.search)
}

export function navigate(path: string) {
  history.pushState(null, '', path)
  dispatchEvent(new PopStateEvent('popstate'))
}

const CODE = /^[a-km-np-z2-9]{4}-[a-km-np-z2-9]{4}-[a-km-np-z2-9]{4}$/

/** 코드 또는 회의 링크 전체를 받아 코드만 돌려준다. 형식이 틀리면 null. */
export function parseRoomCode(input: string): string | null {
  const last = input.trim().toLowerCase().split('/').filter(Boolean).pop() ?? ''
  return CODE.test(last) ? last : null
}

/** 지금 주소의 쿼리 값(화면은 주소가 바뀌면 다시 그려지므로 그때그때 읽는다). */
export function queryParam(name: string): string | null {
  return new URLSearchParams(location.search).get(name)
}

/** 주소를 바꾸되 방문 기록은 남기지 않는다(쿼리의 일회용 값 지우기 등). */
export function replacePath(path: string) {
  history.replaceState(null, '', path)
  dispatchEvent(new PopStateEvent('popstate'))
}

const AUTH_PAGES = ['/login', '/signup', '/verify', '/forgot', '/reset']

/**
 * 로그인 뒤 돌아갈 주소(?next=). 이 사이트 안의 경로만 받는다(다른 사이트로 보내는 데 쓰이지 않게).
 * '//'·'/\'로 시작하면 브라우저가 다른 호스트로 읽고, 역슬래시·제어 문자(탭·줄바꿈은 주소에서 지워진다)도 그렇게 될 수 있어 받지 않는다.
 */
export function safeNext(raw: string | null): string | null {
  if (!raw || raw[0] !== '/' || raw[1] === '/') return null
  for (const ch of raw) {
    const c = ch.charCodeAt(0)
    if (ch === '\\' || c < 0x20 || (c >= 0x7f && c <= 0x9f)) return null
  }
  // 인증 화면(로그인·가입 등)으로 돌아가면 로그인한 뒤 빈 화면에 멈추므로 받지 않는다.
  if (AUTH_PAGES.includes(raw.split(/[?#]/, 1)[0].replace(/\/+$/, ''))) return null
  return raw
}

export function nextParam(): string | null {
  return safeNext(queryParam('next'))
}

export function withNext(path: string, next: string | null): string {
  return next ? `${path}${path.includes('?') ? '&' : '?'}next=${encodeURIComponent(next)}` : path
}

/** 지금 화면 주소(경로·쿼리·해시) */
export function here(): string {
  return location.pathname + location.search + location.hash
}

/** 로그인·가입 화면 주소. 인증 화면끼리는 받은 next를 넘기고, 다른 화면에서는 지금 화면으로 돌아오게 한다. */
export function authPath(to: '/login' | '/signup'): string {
  const path = location.pathname
  return withNext(to, AUTH_PAGES.includes(path) ? nextParam() : path === '/' ? null : here())
}

/**
 * 로그인한 주최자만 보는 화면. 손님이면 로그인(또는 가입) 화면으로 보내고 돌아올 주소를 next로 붙인다.
 * 이 화면에서 로그아웃했으면 홈으로 보낸다. 방문 기록을 바꿔 치우므로 뒤로 가기가 다시 이 화면에 갇히지 않는다.
 */
export function useHostOnly(guestTo: '/login' | '/signup' = '/login'): AuthState {
  const auth = useAuth()
  const seenHost = useRef(false)
  const page = useRef(location.pathname)
  useEffect(() => {
    if (auth.status === 'host') seenHost.current = true
    // 이미 다른 화면으로 보냈으면 다시 보내지 않는다(효과가 한 번 더 돌면 next가 겹친다).
    else if (auth.status === 'guest' && location.pathname === page.current)
      replacePath(auth.loggedOut && seenHost.current ? '/' : withNext(guestTo, here()))
  }, [auth, guestTo])
  return auth
}
