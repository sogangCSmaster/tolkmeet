const JSON_HEADERS = { 'Content-Type': 'application/json' }

/** 'day': 구독이 없고 남은 일일권이 있다(24시간짜리, 한 번 결제). */
export type Plan = 'none' | 'basic' | 'pro' | 'day'
export type Theme = 'system' | 'light' | 'dark'
export type CaptionSize = 's' | 'm' | 'l'

export type UserPrefs = {
  lang?: string
  partnerLang?: string
  theme?: Theme
  captionSize?: CaptionSize
  savePrompt?: boolean
}

export type Passkey = { id: string; name: string; createdAt: number; lastUsedAt: number | null }
export type Device = { id: string; current: boolean; userAgent: string; createdAt: number; lastSeenAt: number | null }

export type User = {
  id: number
  email: string
  name: string
  displayName: string
  locale: string
  verified: boolean
  hasPassword: boolean
  google: boolean
  totp: boolean
  plan: Plan
  planRenewsAt: number | null
  planCanceled: boolean
  operator: boolean
  createdAt: number
  prefs: UserPrefs
  passwordChangedAt: number | null
  passkeys: Passkey[]
  devices: Device[]
  recoveryLeft: number
}

export type Features = { google: boolean; billing: boolean; devBilling: boolean }

export type Meeting = {
  id: number
  code: string
  title: string
  kind: 'meeting' | 'inPerson' | 'live'
  langA: string
  langB: string
  startsAt: number | null
  durationMin: number
  guests: string[]
  createdAt: number
  startedAt: number | null
  endedAt: number | null
  lengthSec: number
  translatedSec: number
  peak: number
  /** 저장하면서 초대 메일을 보낸 주소(한도에 걸린 주소는 빠진다) */
  sent?: string[]
}

export type Billing = {
  plan: Plan
  includedSec: number
  usedSec: number
  extraSec: number
  /** 남은 일일권 시간과 끝나는 때(없거나 끝났으면 0·null) */
  daySec: number
  dayUntil: number | null
  remainingSec: number
  periodStart: number | null
  /** 이번 기간의 결제가 끝나 포함 시간을 받았는지. 첫 결제 직후 잠깐, 또는 갱신 결제가 밀리면 false(그동안 포함 시간은 0). */
  periodPaid: boolean
  /** 갱신 결제가 실패해 연체 중이다(첫 결제를 확인하는 몇 초는 아니다). 이때만 결제 수단 업데이트를 안내한다. */
  pastDue: boolean
  resetsAt: number
  renewsAt: number | null
  canceled: boolean
  hasCustomer: boolean
  /** 최근 6달(오래된 달부터) */
  history: { period: string; seconds: number }[]
}

/** kind·langA·langB는 예약 회의에서 온다(새 서버만 준다. 언어는 비어 있을 수 있다). */
export type RoomInfo = { translation: boolean; title: string; host: string; kind?: Meeting['kind']; langA?: string; langB?: string }

/** 서버 응답: 성공이면 data, 실패면 상태 코드와 서버가 준 오류 코드(`{code}`). 네트워크 오류는 status 0. */
export type Result<T> = { ok: true; data: T } | { ok: false; status: number; code: string }

let unauthorized: (() => void) | null = null

/** 로그인이 필요한 요청이 세션 없음(401 `login`)을 받으면 부른다. 로그인·인증 요청 자체는 이 코드를 주지 않는다. */
export function onUnauthorized(cb: () => void) {
  unauthorized = cb
}

async function call<T>(method: string, path: string, body?: unknown): Promise<Result<T>> {
  try {
    const res = await fetch(path, {
      method,
      headers: body === undefined ? undefined : JSON_HEADERS,
      body: body === undefined ? undefined : JSON.stringify(body),
    })
    const text = await res.text()
    const json = text ? (JSON.parse(text) as unknown) : null
    if (res.ok) return { ok: true, data: json as T }
    const code = json && typeof json === 'object' && 'code' in json ? String((json as { code: unknown }).code) : ''
    if (res.status === 401 && code === 'login') unauthorized?.()
    return { ok: false, status: res.status, code }
  } catch {
    return { ok: false, status: 0, code: 'network' }
  }
}

export const api = {
  get: <T>(path: string) => call<T>('GET', path),
  post: <T = null>(path: string, body: unknown = {}) => call<T>('POST', path, body),
  patch: <T = null>(path: string, body: unknown) => call<T>('PATCH', path, body),
  del: <T = null>(path: string, body?: unknown) => call<T>('DELETE', path, body),
}

/** 결제 수단·영수증을 고치는 Paddle 고객 포털로 보낸다. `tab`을 주면 이 탭 대신 그 탭에서 연다(실패하면 닫는다). 실패하면 false. */
export async function openPortal(tab?: Window): Promise<boolean> {
  const r = await api.post<{ url: string }>('/api/billing/portal')
  if (r.ok) (tab ?? window).location.href = r.data.url
  else tab?.close()
  return r.ok
}

export async function fetchMe(): Promise<{ user: User | null; features: Features }> {
  const res = await fetch('/api/me')
  if (!res.ok) throw new Error(`status ${res.status}`)
  return (await res.json()) as { user: User | null; features: Features }
}

export type LoginResult =
  | { kind: 'ok' }
  | { kind: 'mfa'; ticket: string }
  | { kind: 'invalid' | 'locked' | 'busy' | 'unverified' | 'failed' }

export async function login(email: string, password: string): Promise<LoginResult> {
  const r = await api.post<{ mfa?: string } | null>('/api/login', { email, password })
  if (r.ok) return r.data?.mfa ? { kind: 'mfa', ticket: r.data.mfa } : { kind: 'ok' }
  if (r.status === 401) return { kind: 'invalid' }
  if (r.status === 429) return { kind: 'locked' }
  if (r.status === 503 && r.code === 'busy') return { kind: 'busy' }
  // 비밀번호는 맞고 메일 인증만 남은 계정
  if (r.status === 403) return { kind: 'unverified' }
  return { kind: 'failed' }
}

export async function logout() {
  const res = await fetch('/api/logout', { method: 'POST' })
  if (!res.ok) throw new Error(`status ${res.status}`)
}

export type CreateResult = { kind: 'ok'; code: string } | { kind: 'plan' | 'failed' }

export async function createRoom(translation: boolean, kind: Meeting['kind'] = 'meeting'): Promise<CreateResult> {
  const r = await api.post<{ code: string }>('/api/rooms', { translation, kind })
  if (r.ok) return { kind: 'ok', code: r.data.code }
  return { kind: r.status === 402 ? 'plan' : 'failed' }
}

/** 없는 회의면 null. */
export async function fetchRoom(code: string): Promise<RoomInfo | null> {
  const res = await fetch(`/api/rooms/${code}`)
  if (res.status === 404) return null
  if (!res.ok) throw new Error(`status ${res.status}`)
  return (await res.json()) as RoomInfo
}
