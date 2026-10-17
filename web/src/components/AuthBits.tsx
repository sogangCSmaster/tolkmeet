import type { ReactNode } from 'react'
import { useI18n } from '../lib/i18n'
import { authPath, navigate, nextParam, withNext } from '../lib/router'
import { Header } from './Header'

/** 인증 화면 공통 틀: 머리글 + 가운데 카드. `aside`는 카드 아래 작은 링크 줄. */
export function AuthLayout({ children }: { children: ReactNode }) {
  return (
    <div className="shell">
      <Header />
      <main className="auth">{children}</main>
    </div>
  )
}

/** 앱 안 이동 링크(새로 고침 없이). */
export function InLink({ to, children, className }: { to: string; children: ReactNode; className?: string }) {
  return (
    <a
      href={to}
      className={className}
      onClick={(e) => {
        if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return
        e.preventDefault()
        navigate(to)
      }}
    >
      {children}
    </a>
  )
}

/** 공식 다색 G 로고(Google 로그인 버튼 가이드의 모양). */
function GoogleMark() {
  return (
    <svg width="18" height="18" viewBox="0 0 48 48" aria-hidden="true">
      <path fill="#FFC107" d="M43.6 20.5H42V20H24v8h11.3C33.7 32.7 29.2 36 24 36c-6.6 0-12-5.4-12-12s5.4-12 12-12c3.1 0 5.8 1.2 7.9 3.1l5.7-5.7C34.1 6.1 29.3 4 24 4 12.9 4 4 12.9 4 24s8.9 20 20 20 20-8.9 20-20c0-1.3-.1-2.4-.4-3.5z" />
      <path fill="#FF3D00" d="m6.3 14.7 6.6 4.8C14.7 15.1 19 12 24 12c3.1 0 5.8 1.2 7.9 3.1l5.7-5.7C34.1 6.1 29.3 4 24 4 16.3 4 9.7 8.3 6.3 14.7z" />
      <path fill="#4CAF50" d="M24 44c5.2 0 9.9-2 13.4-5.2l-6.2-5.2C29.2 35.1 26.7 36 24 36c-5.2 0-9.6-3.3-11.3-8l-6.5 5C9.5 39.6 16.2 44 24 44z" />
      <path fill="#1976D2" d="M43.6 20.5H42V20H24v8h11.3c-.8 2.2-2.2 4.2-4.1 5.6l6.2 5.2C37 39.2 44 34 44 24c0-1.3-.1-2.4-.4-3.5z" />
    </svg>
  )
}

/**
 * Google로 계속하기: 서버가 Google로 보냈다가 돌아오면 로그인된 채로 가려던 곳(next, 없으면 홈)에 온다.
 * 서버도 next를 같은 규칙으로 다시 확인하고, 2차 인증이 필요하면 로그인 화면에 next를 붙여 보낸다.
 */
export function GoogleButton() {
  const { t, locale } = useI18n()
  return (
    <a className="btn btn-lg btn-plain" href={withNext(`/api/auth/google?locale=${locale}`, nextParam())}>
      <GoogleMark />
      {t('auth.google')}
    </a>
  )
}

export function Divider({ label }: { label: string }) {
  return (
    <div className="divider" role="separator">
      <span>{label}</span>
    </div>
  )
}

/** 머리 줄 오른쪽의 계정 전환 링크(가입 ↔ 로그인). 돌아갈 주소(next)는 그대로 넘긴다. */
export function AuthSwitch({ prompt, to, label }: { prompt: string; to: '/login' | '/signup'; label: string }) {
  return (
    <p className="auth-switch">
      {prompt} <InLink to={authPath(to)}>{label}</InLink>
    </p>
  )
}
