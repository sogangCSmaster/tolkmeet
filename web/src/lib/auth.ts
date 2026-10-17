/**
 * 로그인 상태 저장소. 앱이 뜨자마자(모듈 로드 시) 한 번 확인을 시작해 첫 화면과 병렬로 진행한다.
 */
import { useSyncExternalStore } from 'react'
import { fetchMe, logout as apiLogout, onUnauthorized, type Features, type User } from './api'

export type AuthState =
  | { status: 'loading' }
  /** loggedOut: 이 탭에서 로그아웃해서 손님이 됐다(세션 만료와 구분해 로그인 화면 대신 홈으로 보낸다). */
  | { status: 'guest'; features: Features; loggedOut?: true }
  | { status: 'host'; user: User; features: Features }

const NO_FEATURES: Features = { google: false, billing: false, devBilling: false }

let state: AuthState = { status: 'loading' }
let generation = 0
let refreshId = 0
/** 진행 중인 상태 조회 수 */
let refreshing = 0
const listeners = new Set<() => void>()
const userListeners = new Set<(u: User | null) => void>()

function set(next: AuthState) {
  state = next
  listeners.forEach((l) => l())
  userListeners.forEach((l) => l(next.status === 'host' ? next.user : null))
}

/** 로그인 사용자가 바뀔 때(테마·언어 같은 계정 설정을 적용하려고). */
export function onUserChange(cb: (u: User | null) => void): () => void {
  userListeners.add(cb)
  return () => userListeners.delete(cb)
}

export function refreshAuth(): Promise<void> {
  const startedGeneration = generation
  const id = ++refreshId
  const isCurrent = () => generation === startedGeneration && refreshId === id
  refreshing++
  return fetchMe()
    .then(({ user, features }) => {
      if (isCurrent()) set(user ? { status: 'host', user, features } : { status: 'guest', features })
    })
    .catch(() => {
      if (isCurrent()) set({ status: 'guest', features: features() })
    })
    .finally(() => refreshing--)
}

/** 서버가 돌려준 새 사용자 정보로 바로 바꾼다(설정 저장 뒤 다시 묻지 않게). */
export function setUser(user: User) {
  if (state.status === 'host') set({ ...state, user })
}

export function features(): Features {
  return state.status === 'loading' ? NO_FEATURES : state.features
}

export async function logout(): Promise<boolean> {
  generation++
  try {
    await apiLogout()
  } catch {
    return false
  }
  // 로그아웃 도중 시작된 상태 조회도 세션 폐기 이전 응답일 수 있다.
  generation++
  set({ status: 'guest', features: features(), loggedOut: true })
  return true
}

void refreshAuth()
// 세션이 끝났는데(만료·다른 탭에서 로그아웃) 화면은 로그인 상태면 다시 확인한다. 손님이 되면 화면이 로그인으로 보낸다.
// 이미 손님이면 할 일이 없고, 확인 중이면(첫 /api/me 포함) 그 결과로 충분하다. 새로 물으면 왕복이 늘고 진행 중인 결과를 버린다.
onUnauthorized(() => {
  if (state.status === 'host' && refreshing === 0) void refreshAuth()
})

export function useAuth(): AuthState {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb)
      return () => listeners.delete(cb)
    },
    () => state,
  )
}

/** 화면에 부를 이름: 회의 이름 → 이름 → 이메일 앞부분. */
export function userName(u: User): string {
  return u.displayName || u.name || u.email.split('@')[0]
}

/** 서버 기능(Google 로그인·결제 등). 첫 /api/me가 늦게 와도 화면이 다시 그려지게 구독한다. */
export function useFeatures(): Features {
  const s = useAuth()
  return s.status === 'loading' ? NO_FEATURES : s.features
}
