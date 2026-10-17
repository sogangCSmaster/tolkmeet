/**
 * 패스키(WebAuthn). 서버는 바이트 값을 base64url 문자열로 주고받고, 브라우저 API는 ArrayBuffer를 쓴다.
 */
import { api } from './api'

export const passkeySupported = typeof window !== 'undefined' && typeof window.PublicKeyCredential === 'function'

function fromB64(s: string): ArrayBuffer {
  const b64 = s.replace(/-/g, '+').replace(/_/g, '/') + '==='.slice((s.length + 3) % 4)
  const bin = atob(b64)
  const out = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
  return out.buffer
}

function toB64(buf: ArrayBuffer): string {
  let bin = ''
  for (const b of new Uint8Array(buf)) bin += String.fromCharCode(b)
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}

/** 이 기기 이름(목록에서 알아보게). 정확할 필요는 없다. */
function deviceName(): string {
  const ua = navigator.userAgent
  const os = /iPhone/.test(ua) ? 'iPhone' : /iPad/.test(ua) ? 'iPad' : /Android/.test(ua) ? 'Android' : /Mac/.test(ua) ? 'Mac' : /Windows/.test(ua) ? 'Windows' : 'Device'
  const browser = /Edg\//.test(ua) ? 'Edge' : /Chrome\//.test(ua) ? 'Chrome' : /Safari\//.test(ua) ? 'Safari' : /Firefox\//.test(ua) ? 'Firefox' : ''
  return browser ? `${os} · ${browser}` : os
}

type CreateOptions = {
  challenge: string
  rp: PublicKeyCredentialRpEntity
  user: { id: string; name: string; displayName: string }
  pubKeyCredParams: PublicKeyCredentialParameters[]
  authenticatorSelection: AuthenticatorSelectionCriteria
  excludeCredentials: { type: 'public-key'; id: string }[]
  attestation: AttestationConveyancePreference
  timeout: number
}

type GetOptions = { challenge: string; rpId: string; userVerification: UserVerificationRequirement; timeout: number }

/**
 * 'cancelled'는 사용자가 OS 창을 닫은 경우(오류로 보이지 않는다). 'failed'는 패스키가 받아들여지지 않은 경우,
 * 'limited'는 시도가 너무 많은 경우, 'error'는 네트워크·서버 오류.
 */
export type PasskeyResult = 'ok' | 'cancelled' | 'failed' | 'limited' | 'error'

function isCancel(e: unknown): boolean {
  return e instanceof DOMException && (e.name === 'NotAllowedError' || e.name === 'AbortError')
}

/** 등록 전에 본인 확인이 필요하다(비밀번호가 있는 계정은 비밀번호). 'reauth'·'current'·'locked'·'busy'는 그 실패(서버 오류 코드 그대로). */
export async function registerPasskey(password: string): Promise<PasskeyResult | 'reauth' | 'current' | 'locked' | 'busy'> {
  const opts = await api.post<CreateOptions>('/api/passkeys/register/options', { password })
  if (!opts.ok) return (['reauth', 'current', 'locked', 'busy'] as const).find((c) => c === opts.code) ?? 'failed'
  const o = opts.data
  let cred: PublicKeyCredential
  try {
    cred = (await navigator.credentials.create({
      publicKey: {
        ...o,
        challenge: fromB64(o.challenge),
        user: { ...o.user, id: fromB64(o.user.id) },
        excludeCredentials: o.excludeCredentials.map((c) => ({ ...c, id: fromB64(c.id) })),
      },
    })) as PublicKeyCredential
  } catch (e) {
    return isCancel(e) ? 'cancelled' : 'failed'
  }
  const res = cred.response as AuthenticatorAttestationResponse
  const done = await api.post('/api/passkeys/register', {
    name: deviceName(),
    credential: {
      id: cred.id,
      response: { clientDataJSON: toB64(res.clientDataJSON), attestationObject: toB64(res.attestationObject) },
    },
  })
  return done.ok ? 'ok' : 'failed'
}

export async function loginWithPasskey(): Promise<PasskeyResult> {
  const opts = await api.post<GetOptions>('/api/passkeys/login/options')
  if (!opts.ok) return opts.status === 429 ? 'limited' : 'error'
  let cred: PublicKeyCredential
  try {
    cred = (await navigator.credentials.get({
      publicKey: { ...opts.data, challenge: fromB64(opts.data.challenge) },
    })) as PublicKeyCredential
  } catch (e) {
    return isCancel(e) ? 'cancelled' : 'failed'
  }
  const res = cred.response as AuthenticatorAssertionResponse
  const done = await api.post('/api/passkeys/login', {
    credential: {
      id: cred.id,
      response: {
        clientDataJSON: toB64(res.clientDataJSON),
        authenticatorData: toB64(res.authenticatorData),
        signature: toB64(res.signature),
      },
    },
  })
  return done.ok ? 'ok' : done.status === 401 ? 'failed' : 'error'
}
