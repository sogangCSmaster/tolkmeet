import { useEffect, useRef, useState, type FormEvent, type ReactNode } from 'react'
import { api, openPortal, type Billing, type CaptionSize, type Theme, type User, type UserPrefs } from '../lib/api'
import { features, logout, setUser, userName } from '../lib/auth'
import { formatDate, formatDateTime, formatDuration, relativeTime, deviceLabel } from '../lib/format'
import { useI18n, type MessageKey } from '../lib/i18n'
import '../lib/i18nPages'
import { LANGUAGES } from '../lib/languages'
import { registerPasskey, passkeySupported } from '../lib/passkey'
import { here, navigate, queryParam, replacePath, useHostOnly, withNext } from '../lib/router'
import { applyCaptionSize, applyTheme, currentCaptionSize, currentTheme } from '../lib/theme'
import { InLink } from './AuthBits'
import { CodeInput } from './CodeInput'
import { Header } from './Header'
import { Icon } from './Icon'
import { Select } from './Select'
import { Switch } from './Switch'
import { UnpaidNotice } from './UnpaidNotice'

const LANG_OPTIONS = LANGUAGES.map((l) => ({ value: l.code as string, label: l.name }))

/** 서버 오류 코드 → 문구 */
function errorKey(code: string): MessageKey {
  switch (code) {
    case 'current':
      return 'account.wrongPassword'
    case 'reauth':
      return 'account.reauth'
    case 'locked':
      return 'auth.locked'
    case 'busy':
      return 'auth.busy'
    // 세션이 끝났다(로그인 상태를 다시 확인해 로그인 화면으로 보낸다)
    case 'login':
      return 'account.reauth'
    case 'password':
      return 'auth.passwordShort'
    case 'breached':
      return 'auth.breached'
    case 'email':
      return 'auth.emailInvalid'
    case 'taken':
      return 'account.emailTaken'
    case 'code':
      return 'mfa.wrong'
    case 'rate_limited':
      return 'auth.rateLimited'
    case 'last_method':
      return 'account.lastMethod'
    default:
      return 'account.failed'
  }
}

/** 로그인한 사람만. 아니면 로그인 화면으로 보낸다(로그인 뒤 이 화면으로 돌아온다). */
function useUser(): User | null {
  const auth = useHostOnly('/login')
  return auth.status === 'host' ? auth.user : null
}

type Section = 'profile' | 'plan' | 'usage' | 'prefs' | 'security' | 'data'
const SECTIONS: Section[] = ['profile', 'plan', 'usage', 'prefs', 'data']

function AccountShell({ user, active, title, children }: { user: User; active: Section; title: string; children: ReactNode }) {
  const { t } = useI18n()
  // 계정 화면 안에서는 지금 보는 칸(주소의 #)을 표시한다.
  const [section, setSection] = useState<Section>(() => {
    const hash = location.hash.slice(1) as Section
    return active !== 'security' && SECTIONS.includes(hash) ? hash : active
  })
  // 주소의 #만 바뀌면(직접 고치거나 앞뒤로 가기) 화면을 다시 그리지 않으므로 직접 듣는다.
  useEffect(() => {
    if (active === 'security') return
    const onHash = () => {
      const hash = location.hash.slice(1) as Section
      if (SECTIONS.includes(hash)) setSection(hash)
    }
    addEventListener('hashchange', onHash)
    return () => removeEventListener('hashchange', onHash)
  }, [active])
  const [loggingOut, setLoggingOut] = useState(false)
  const [logoutFailed, setLogoutFailed] = useState(false)
  const items: [Section, MessageKey, string][] = [
    ['profile', 'account.profile', '/account#profile'],
    ['plan', 'account.plan', '/account#plan'],
    // 요금제가 없으면 사용량 칸이 없다.
    ...(user.plan === 'basic' || user.plan === 'pro' ? ([['usage', 'account.usage', '/account#usage']] as [Section, MessageKey, string][]) : []),
    ['prefs', 'account.prefs', '/account#prefs'],
    ['security', 'account.security', '/account/security'],
    ['data', 'account.data', '/account#data'],
  ]

  // 성공하면 auth 상태가 바뀌어 화면이 홈으로 간다(머리글의 로그아웃과 같다).
  async function out() {
    if (loggingOut) return
    setLoggingOut(true)
    setLogoutFailed(false)
    const ok = await logout()
    setLoggingOut(false)
    setLogoutFailed(!ok)
  }

  return (
    <div className="shell">
      <Header />
      <div className="settings">
        <aside className="settings-side">
          {active !== 'security' && (
            <div className="settings-me">
              <span className="avatar-icon" aria-hidden="true">
                <Icon name="person" />
              </span>
              <b>{userName(user)}</b>
              <span title={user.email}>{user.email}</span>
            </div>
          )}
          <nav aria-label={t('account.title')}>
            {items.map(([id, key, href]) => (
              <a
                key={id}
                href={href}
                aria-current={id === section ? 'page' : undefined}
                onClick={(e) => {
                  if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return
                  e.preventDefault()
                  const [path, hash] = href.split('#')
                  if (location.pathname === path && hash) {
                    setSection(id)
                    history.replaceState(null, '', href)
                    document.getElementById(hash)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
                    return
                  }
                  navigate(href)
                }}
              >
                {t(key)}
              </a>
            ))}
            <button className="settings-logout" disabled={loggingOut} onClick={() => void out()}>
              {t('auth.logout')}
            </button>
          </nav>
          {logoutFailed && (
            <p className="form-error" role="alert">
              {t('auth.logoutFailed')}
            </p>
          )}
        </aside>
        <main className="settings-main">
          <h1>{title}</h1>
          {children}
        </main>
      </div>
    </div>
  )
}

function Card({ id, title, action, children }: { id?: string; title: string; action?: ReactNode; children: ReactNode }) {
  const ref = useRef<HTMLElement>(null)
  // 다른 화면에서 /account#data처럼 들어오면 브라우저는 칸이 그려지기 전이라 못 찾는다. 칸이 생길 때 직접 찾아간다
  // (사용량 칸은 요금 정보를 받은 뒤에야 생긴다).
  useEffect(() => {
    if (id && location.hash === `#${id}`) ref.current?.scrollIntoView({ block: 'start' })
  }, [id])
  return (
    <section ref={ref} className="settings-card" id={id} aria-labelledby={id ? `${id}-title` : undefined}>
      <div className="settings-card-head">
        <h2 id={id ? `${id}-title` : undefined}>{title}</h2>
        {action}
      </div>
      {children}
    </section>
  )
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="settings-row">
      <span className="settings-label">{label}</span>
      <div className="settings-value">{children}</div>
    </div>
  )
}

function Message({ error, note }: { error?: MessageKey | null; note?: MessageKey | null }) {
  const { t } = useI18n()
  if (error)
    return (
      <p className="form-error" role="alert">
        {t(error)}
        {error === 'account.reauth' && (
          <>
            {' '}
            <button
              className="link-btn"
              onClick={() => {
                // 로그아웃하면 화면(useHostOnly)이 먼저 홈으로 옮기므로 돌아올 주소는 그 전에 정해 둔다.
                const to = withNext('/login', here())
                void logout().then((ok) => ok && replacePath(to))
              }}
            >
              {t('account.loginAgain')}
            </button>
          </>
        )}
      </p>
    )
  if (note)
    return (
      <p className="form-note" role="status">
        {t(note)}
      </p>
    )
  return null
}

/** 한 줄짜리 값을 그 자리에서 고친다. */
function EditableRow({ label, value, onSave, maxLength = 60 }: { label: string; value: string; onSave: (v: string) => Promise<boolean>; maxLength?: number }) {
  const { t } = useI18n()
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(value)
  const [saving, setSaving] = useState(false)
  const [failed, setFailed] = useState(false)
  if (!editing)
    return (
      <Row label={label}>
        <span>{value || '—'}</span>
        <button
          className="link-btn"
          onClick={() => {
            setDraft(value)
            setFailed(false)
            setEditing(true)
          }}
        >
          {t('account.change')}
        </button>
      </Row>
    )
  return (
    <Row label={label}>
      <form
        className="inline-form"
        onSubmit={async (e) => {
          e.preventDefault()
          setSaving(true)
          setFailed(false)
          const ok = await onSave(draft.trim())
          setSaving(false)
          if (ok) setEditing(false)
          else setFailed(true)
        }}
      >
        <input className="input input-sm" value={draft} maxLength={maxLength} autoFocus onChange={(e) => setDraft(e.target.value)} aria-label={label} />
        <button className="btn btn-sm btn-ink" disabled={saving}>
          {t('account.save')}
        </button>
        <button type="button" className="btn btn-sm btn-outline" onClick={() => setEditing(false)}>
          {t('room.cancel')}
        </button>
        {failed && <Message error="account.failed" />}
      </form>
    </Row>
  )
}

async function patchAccount(body: object): Promise<boolean> {
  const r = await api.patch<User>('/api/account', body)
  if (r.ok) setUser(r.data)
  return r.ok
}

// ── 계정 ─────────────────────────────────────────────

export function Account() {
  const { t } = useI18n()
  const user = useUser()
  if (!user) return <div className="shell" />
  return (
    <AccountShell user={user} active="profile" title={t('account.title')}>
      <ProfileCard user={user} />
      <PlanCard user={user} />
      <PrefsCard user={user} />
      <SecuritySummary user={user} />
      <DataCard user={user} />
    </AccountShell>
  )
}

function ProfileCard({ user }: { user: User }) {
  const { t } = useI18n()
  return (
    <Card id="profile" title={t('account.profile')}>
      <EditableRow label={t('auth.name')} value={user.name} onSave={(name) => patchAccount({ name })} />
      <EmailRow user={user} />
      <EditableRow label={t('account.displayName')} value={user.displayName} onSave={(displayName) => patchAccount({ displayName })} />
    </Card>
  )
}

function EmailRow({ user }: { user: User }) {
  const { t } = useI18n()
  const [step, setStep] = useState<'view' | 'enter' | 'code'>('view')
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [code, setCode] = useState('')
  const [error, setError] = useState<MessageKey | null>(null)
  const [note, setNote] = useState<MessageKey | null>(null)
  const [busy, setBusy] = useState(false)

  async function send(e?: FormEvent) {
    e?.preventDefault()
    setBusy(true)
    setError(null)
    setNote(null)
    const r = await api.post('/api/account/email', { email: email.trim(), password })
    setBusy(false)
    if (r.ok) setStep('code')
    else setError(errorKey(r.code))
    return r.ok
  }

  async function confirm(value: string) {
    setBusy(true)
    setError(null)
    const r = await api.post<User>('/api/account/email/verify', { code: value })
    setBusy(false)
    if (r.ok) {
      setUser(r.data)
      setStep('view')
    } else {
      setCode('')
      setError(errorKey(r.code))
    }
  }

  if (step === 'view')
    return (
      <Row label={t('auth.email')}>
        <span>{user.email}</span>
        <button className="link-btn" onClick={() => setStep('enter')}>
          {t('account.change')}
        </button>
      </Row>
    )
  return (
    <div className="settings-row column">
      <span className="settings-label">{t('auth.email')}</span>
      {step === 'enter' ? (
        <form className="stack-form" onSubmit={send}>
          <input className="input" type="email" autoComplete="email" placeholder={t('account.newEmail')} value={email} onChange={(e) => setEmail(e.target.value)} autoFocus aria-label={t('account.newEmail')} />
          {user.hasPassword && (
            <input className="input" type="password" autoComplete="current-password" placeholder={t('account.currentPassword')} value={password} onChange={(e) => setPassword(e.target.value)} aria-label={t('account.currentPassword')} />
          )}
          <Message error={error} />
          <div className="row">
            <button className="btn btn-ink" disabled={busy || !email.trim()}>
              {t('account.sendCode')}
            </button>
            <button type="button" className="btn btn-outline" onClick={() => setStep('view')}>
              {t('room.cancel')}
            </button>
          </div>
        </form>
      ) : (
        <div className="stack-form">
          <p className="form-note">{t('account.codeSent', { email: email.trim() })}</p>
          <CodeInput value={code} onChange={setCode} onComplete={(v) => void confirm(v)} label={t('verify.code')} disabled={busy} autoFocus />
          <Message error={error} note={note} />
          <div className="row">
            <button type="button" className="link-btn" disabled={busy} onClick={() => void send().then((ok) => ok && setNote('verify.resent'))}>
              {t('verify.resend')}
            </button>
            <button type="button" className="link-btn" onClick={() => setStep('enter')}>
              {t('verify.changeEmail')}
            </button>
            <button type="button" className="link-btn" onClick={() => setStep('view')}>
              {t('room.cancel')}
            </button>
          </div>
        </div>
      )}
    </div>
  )
}

function useBilling(): [Billing | null, () => void] {
  const [billing, setBilling] = useState<Billing | null>(null)
  const [n, setN] = useState(0)
  useEffect(() => {
    let alive = true
    void api.get<Billing>('/api/billing').then((r) => alive && r.ok && setBilling(r.data))
    return () => {
      alive = false
    }
  }, [n])
  return [billing, () => setN((x) => x + 1)]
}

const PLAN_PRICE = { basic: '$19', pro: '$49' } as const

function PlanCard({ user }: { user: User }) {
  const { t, locale } = useI18n()
  const [billing, reload] = useBilling()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<MessageKey | null>(null)

  async function manage(action: string, plan?: string) {
    setBusy(true)
    setError(null)
    const r = await api.post(`/api/billing/${action}`, { plan: plan ?? '' })
    setBusy(false)
    // 지난 결제가 실패한 구독은 Paddle이 바꾸지 않는다. 결제 수단부터 고치게 안내한다.
    if (!r.ok) return setError(r.code === 'past_due' ? 'checkout.pastDue' : 'account.billingFailed')
    reload()
    // 요금제가 바뀌면 화면 맨 위(머리글·홈)도 맞춘다.
    const me = await api.get<{ user: User }>('/api/me')
    if (me.ok && me.data.user) setUser(me.data.user)
  }

  async function portal() {
    if (!(await openPortal())) setError('account.billingFailed')
  }

  const plan = user.plan
  return (
    <>
      <Card id="plan" title={t('account.plan')}>
        {plan === 'day' ? (
          // 일일권만 남은 사람: 끝나는 때와 남은 시간, 구독으로 넘어가는 길.
          <div className="plan-box">
            <p className="plan-name">{t('plan.dayPass')}</p>
            <p className="plan-meta">
              {billing?.dayUntil != null && t('plan.dayUntil', { date: formatDateTime(billing.dayUntil, locale) })}
              {billing && ` · ${t('home.left', { time: formatDuration(billing.remainingSec, t) })}`}
            </p>
            <div className="row">
              <InLink to="/checkout?item=basic" className="btn btn-primary">
                {t('plan.subscribe')}
              </InLink>
            </div>
          </div>
        ) : plan === 'none' ? (
          <div className="plan-empty">
            <p>{t('account.noPlan')}</p>
            <InLink to="/checkout" className="btn btn-primary">
              {t('account.choosePlan')}
            </InLink>
          </div>
        ) : (
          <div className="plan-box">
            <p className="plan-name">
              {plan === 'pro' ? 'Pro' : 'Basic'}
              <small> · {t('account.monthly')}</small>
            </p>
            <p className="plan-meta">
              {t('account.planPrice', { price: PLAN_PRICE[plan] })}
              {user.planRenewsAt != null &&
                ` · ${t(user.planCanceled ? 'account.endsOn' : 'account.renewsOn', { date: formatDate(user.planRenewsAt, locale) })}`}
            </p>
            <div className="row">
              {/* 올리면 남은 기간의 차액이 바로 청구되고(내리면 청구·환불 없음) 결제 화면에서 안내를 보고 확인한 뒤 바꾼다. */}
              {!user.operator && (
                <InLink to={`/checkout?item=${plan === 'pro' ? 'basic' : 'pro'}`} className="btn btn-primary">
                  {t(plan === 'pro' ? 'account.toBasic' : 'account.toPro')}
                </InLink>
              )}
              {billing?.hasCustomer && (
                <button className="btn btn-outline" onClick={() => void portal()}>
                  {t('account.paymentAndReceipts')}
                </button>
              )}
            </div>
            <div className="plan-foot">
              <span>{t('account.paddleNote')}</span>
              {!user.operator &&
                (user.planCanceled ? (
                  <button className="link-btn" disabled={busy} onClick={() => void manage('resume')}>
                    {t('account.resume')}
                  </button>
                ) : (
                  <button className="link-btn danger" disabled={busy} onClick={() => void manage('cancel')}>
                    {t('account.cancelPlan')}
                  </button>
                ))}
            </div>
            {error === 'checkout.pastDue' ? (
              <p className="form-error" role="alert">
                {t(error)}{' '}
                <button className="link-btn" onClick={() => void portal()}>
                  {t('checkout.updatePayment')}
                </button>
              </p>
            ) : (
              <Message error={error} />
            )}
          </div>
        )}
      </Card>
      {billing && (plan === 'basic' || plan === 'pro') && <UsageCard billing={billing} />}
    </>
  )
}

function UsageCard({ billing }: { billing: Billing }) {
  const { t, locale } = useI18n()
  const max = Math.max(billing.includedSec, ...billing.history.map((h) => h.seconds), 1)
  const pct = Math.min(100, (billing.usedSec / Math.max(billing.includedSec, 1)) * 100)
  return (
    <Card id="usage" title={t('account.usage')} action={<span className="card-aside">{t('account.resetsOn', { date: formatDate(billing.resetsAt, locale) })}</span>}>
      {/* 포함 시간은 결제 기간마다 채워진다. 아래 막대는 달력 달별 사용량이다. */}
      <p className="card-lead">{t('account.thisPeriod')}</p>
      <p className="usage-big">
        {formatDuration(billing.usedSec, t)}
        <small> / {t('account.usedOf', { total: formatDuration(billing.includedSec, t) })}</small>
      </p>
      <div className="meter" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct)} aria-label={t('account.usage')}>
        <span style={{ width: `${pct}%` }} />
      </div>
      {billing.pastDue && <UnpaidNotice />}
      <div className="usage-bars" aria-hidden="true">
        {billing.history.map((h, i) => {
          const month = Number(h.period.slice(5))
          const label = new Date(2026, month - 1, 1).toLocaleDateString(locale, { month: 'short' })
          return (
            <div key={h.period} className={`usage-bar${i === billing.history.length - 1 ? ' now' : ''}`}>
              <span style={{ height: `${Math.max(4, (h.seconds / max) * 100)}%` }} />
              <small>{label}</small>
            </div>
          )
        })}
      </div>
      {billing.extraSec > 0 && <p className="form-note">{t('account.extraLeft', { time: formatDuration(billing.extraSec, t) })}</p>}
      <div className="plan-foot">
        <span>{t('account.addHoursNote')}</span>
        <InLink to="/checkout?item=hours" className="btn btn-sm btn-outline">
          {t('account.addHours')}
        </InLink>
      </div>
    </Card>
  )
}

function Segmented<T extends string>({ value, options, onChange, label }: { value: T; options: [T, string][]; onChange: (v: T) => void; label: string }) {
  return (
    <div className="segmented" role="radiogroup" aria-label={label}>
      {options.map(([v, text]) => (
        <button key={v} type="button" role="radio" aria-checked={value === v} className={value === v ? 'on' : ''} onClick={() => onChange(v)}>
          {text}
        </button>
      ))}
    </div>
  )
}

function PrefsCard({ user }: { user: User }) {
  const { t } = useI18n()
  const p = user.prefs
  const [theme, setTheme] = useState<Theme>(p.theme ?? currentTheme())
  const [size, setSize] = useState<CaptionSize>(p.captionSize ?? currentCaptionSize())
  const [error, setError] = useState<MessageKey | null>(null)
  const seq = useRef(0)
  const queue = useRef<Promise<unknown>>(Promise.resolve())

  // 빠르게 여러 번 바꿔도 마지막으로 누른 값이 이긴다: 요청은 차례로 보내고(서버에 순서대로 닿게),
  // 응답은 마지막 요청의 것만 반영한다(앞 요청의 사용자 정보가 테마·자막 크기를 되돌리지 않게).
  function save(prefs: UserPrefs, undo?: () => void) {
    const id = ++seq.current
    setError(null)
    const sent = queue.current.then(() => api.patch<User>('/api/account', { prefs }))
    queue.current = sent
    void sent.then((r) => {
      if (id !== seq.current) return
      if (r.ok) return setUser(r.data)
      undo?.()
      setError('account.failed')
    })
  }

  return (
    <Card id="prefs" title={t('account.prefs')}>
      <Row label={t('lang.mine')}>
        <Select variant="pill" ariaLabel={t('lang.mine')} value={p.lang ?? user.locale} options={LANG_OPTIONS} onChange={(lang) => save({ lang })} />
      </Row>
      <Row label={t('account.partnerLang')}>
        <Select variant="pill" ariaLabel={t('account.partnerLang')} value={p.partnerLang ?? (user.locale === 'ko' ? 'ja' : 'ko')} options={LANG_OPTIONS} onChange={(partnerLang) => save({ partnerLang })} />
      </Row>
      <Row label={t('account.theme')}>
        <Segmented
          label={t('account.theme')}
          value={theme}
          options={[
            ['system', t('account.themeSystem')],
            ['light', t('account.themeLight')],
            ['dark', t('account.themeDark')],
          ]}
          onChange={(v) => {
            const before = theme
            setTheme(v)
            applyTheme(v)
            save({ theme: v }, () => {
              setTheme(before)
              applyTheme(before)
            })
          }}
        />
      </Row>
      <Row label={t('account.captionSize')}>
        <Segmented
          label={t('account.captionSize')}
          value={size}
          options={[
            ['s', t('account.sizeS')],
            ['m', t('account.sizeM')],
            ['l', t('account.sizeL')],
          ]}
          onChange={(v) => {
            const before = size
            setSize(v)
            applyCaptionSize(v)
            save({ captionSize: v }, () => {
              setSize(before)
              applyCaptionSize(before)
            })
          }}
        />
      </Row>
      <Row label={t('account.savePrompt')}>
        <Switch checked={p.savePrompt ?? true} onChange={(savePrompt) => save({ savePrompt })} label={t('account.savePrompt')} hideLabel />
      </Row>
      <Message error={error} />
    </Card>
  )
}

function SecuritySummary({ user }: { user: User }) {
  const { t, locale } = useI18n()
  return (
    <Card id="security" title={t('account.security')} action={<InLink to="/account/security" className="link-btn">{t('account.manage')}</InLink>}>
      <Row label={t('auth.password')}>
        <span>{user.hasPassword ? (user.passwordChangedAt ? t('account.changedAgo', { when: relativeTime(user.passwordChangedAt, locale) }) : t('account.set')) : t('account.notSet')}</span>
      </Row>
      <Row label={t('account.twoStep')}>
        <span>{t(user.totp ? 'account.on' : 'account.off')}</span>
      </Row>
      <Row label={t('account.passkeys')}>
        <span>{t('account.count', { n: String(user.passkeys.length) })}</span>
      </Row>
      <Row label={t('account.google')}>
        <span>{t(user.google ? 'account.connected' : 'account.notConnected')}</span>
      </Row>
      <Row label={t('account.devices')}>
        <span>{t('account.count', { n: String(user.devices.length) })}</span>
      </Row>
    </Card>
  )
}

function DataCard({ user }: { user: User }) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [password, setPassword] = useState('')
  const [confirm, setConfirm] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<MessageKey | null>(null)

  async function remove(e: FormEvent) {
    e.preventDefault()
    setBusy(true)
    setError(null)
    const r = await api.del('/api/account', { password, confirm })
    setBusy(false)
    if (!r.ok) return setError(r.code === 'confirm' ? 'account.confirmMismatch' : r.code === 'billing' ? 'account.billingFailed' : errorKey(r.code))
    await logout()
    replacePath('/')
  }

  return (
    <Card id="data" title={t('account.data')}>
      <Row label={t('account.export')}>
        <a className="link-btn" href="/api/account/export" download>
          <Icon name="download" />
          {t('account.exportJson')}
        </a>
      </Row>
      {!open ? (
        <Row label={t('account.delete')}>
          <button className="link-btn danger" onClick={() => setOpen(true)}>
            {t('account.deleteStart')}
          </button>
        </Row>
      ) : (
        <form className="settings-row column danger-zone" onSubmit={remove}>
          <p>{t('account.deleteWarn')}</p>
          {user.hasPassword ? (
            <input className="input" type="password" autoComplete="current-password" placeholder={t('account.currentPassword')} value={password} onChange={(e) => setPassword(e.target.value)} aria-label={t('account.currentPassword')} autoFocus />
          ) : (
            <input className="input" type="email" placeholder={user.email} value={confirm} onChange={(e) => setConfirm(e.target.value)} aria-label={t('account.typeEmail')} autoFocus />
          )}
          <Message error={error} />
          <div className="row">
            <button className="btn btn-danger" disabled={busy}>
              {t('account.deleteConfirm')}
            </button>
            <button type="button" className="btn btn-outline" onClick={() => setOpen(false)}>
              {t('room.cancel')}
            </button>
          </div>
        </form>
      )}
    </Card>
  )
}

// ── 로그인과 보안 ─────────────────────────────────────

export function Security() {
  const { t } = useI18n()
  const user = useUser()
  if (!user) return <div className="shell" />
  return (
    <AccountShell user={user} active="security" title={t('account.security')}>
      <PasskeyCard user={user} />
      <TotpCard user={user} />
      <PasswordCard user={user} />
      <DevicesCard user={user} />
    </AccountShell>
  )
}

async function refreshUser() {
  const me = await api.get<{ user: User | null }>('/api/me')
  if (me.ok && me.data.user) setUser(me.data.user)
}

/** 민감한 작업 전 비밀번호 한 번(비밀번호가 없는 계정은 묻지 않고 서버가 최근 로그인을 본다). */
function PasswordGate({ user, onPass, onCancel, label, busy = false }: { user: User; onPass: (password: string) => void; onCancel?: () => void; label: string; busy?: boolean }) {
  const { t } = useI18n()
  const [password, setPassword] = useState('')
  const cancel = onCancel && (
    <button type="button" className="btn btn-sm btn-outline" onClick={onCancel}>
      {t('room.cancel')}
    </button>
  )
  if (!user.hasPassword)
    return (
      <div className="row">
        <button className="btn btn-primary" disabled={busy} onClick={() => onPass('')}>
          {label}
        </button>
        {cancel}
      </div>
    )
  return (
    <form
      className="inline-form"
      onSubmit={(e) => {
        e.preventDefault()
        onPass(password)
      }}
    >
      <input className="input input-sm" type="password" autoComplete="current-password" placeholder={t('account.currentPassword')} value={password} onChange={(e) => setPassword(e.target.value)} aria-label={t('account.currentPassword')} autoFocus />
      <button className="btn btn-sm btn-primary" disabled={!password || busy}>
        {label}
      </button>
      {cancel}
    </form>
  )
}

function PasskeyCard({ user }: { user: User }) {
  const { t, locale } = useI18n()
  const [adding, setAdding] = useState(false)
  const [error, setError] = useState<MessageKey | null>(null)

  async function add(password: string) {
    setError(null)
    const r = await registerPasskey(password)
    if (r === 'ok') {
      setAdding(false)
      return refreshUser()
    }
    if (r === 'failed' || r === 'error' || r === 'limited') setError('account.passkeyFailed')
    else if (r !== 'cancelled') setError(errorKey(r))
  }

  async function remove(id: string) {
    const r = await api.del(`/api/passkeys/${encodeURIComponent(id)}`)
    if (r.ok) void refreshUser()
    else setError('account.failed')
  }

  return (
    <Card
      title={t('account.passkeys')}
      action={
        passkeySupported && (
          // 비밀번호가 없는 계정은 물을 것이 없으므로 바로 등록한다. 비밀번호를 묻는 중에 다시 누르면 접는다.
          <button
            className="btn btn-primary btn-sm"
            aria-expanded={user.hasPassword ? adding : undefined}
            onClick={() => {
              setError(null)
              if (user.hasPassword) setAdding((a) => !a)
              else void add('')
            }}
          >
            <Icon name="add" />
            {t('account.addPasskey')}
          </button>
        )
      }
    >
      <p className="card-lead">{t('account.passkeyLead')}</p>
      {adding && (
        <div className="settings-row column">
          <PasswordGate user={user} label={t('account.addPasskey')} onPass={(p) => void add(p)} onCancel={() => setAdding(false)} />
        </div>
      )}
      {user.passkeys.map((k) => (
        <div className="settings-row" key={k.id}>
          <span className="row-icon" aria-hidden="true">
            <Icon name={/iPhone|Android/.test(k.name) ? 'smartphone' : 'laptop_mac'} />
          </span>
          <div className="row-text">
            <b>{k.name || t('account.passkey')}</b>
            <small>
              {t('account.addedOn', { date: formatDate(k.createdAt, locale) })}
              {k.lastUsedAt != null && ` · ${t('account.usedAgo', { when: relativeTime(k.lastUsedAt, locale) })}`}
            </small>
          </div>
          <button className="link-btn danger" onClick={() => void remove(k.id)}>
            {t('account.remove')}
          </button>
        </div>
      ))}
      {user.passkeys.length === 0 && !adding && <p className="card-empty">{t('account.noPasskeys')}</p>}
      <Message error={error} />
    </Card>
  )
}

function RecoveryCodes({ codes }: { codes: string[] }) {
  const { t } = useI18n()
  const text = codes.join('\n')
  return (
    <div className="recovery">
      <p className="form-note">{t('account.recoveryOnce')}</p>
      <ol className="recovery-list">
        {codes.map((c) => (
          <li key={c}>
            <code>{c}</code>
          </li>
        ))}
      </ol>
      <div className="row">
        <button className="btn btn-sm btn-outline" onClick={() => void navigator.clipboard?.writeText(text)}>
          <Icon name="content_copy" />
          {t('account.copy')}
        </button>
        <a className="btn btn-sm btn-outline" href={`data:text/plain;charset=utf-8,${encodeURIComponent(text + '\n')}`} download="tolkmeet-recovery-codes.txt">
          <Icon name="download" />
          {t('account.download')}
        </a>
      </div>
    </div>
  )
}

function TotpCard({ user }: { user: User }) {
  const { t } = useI18n()
  const [setup, setSetup] = useState<{ secret: string; qr: string } | null>(null)
  const [gate, setGate] = useState(false)
  const [code, setCode] = useState('')
  const [codes, setCodes] = useState<string[] | null>(null)
  const [mode, setMode] = useState<'idle' | 'disable' | 'regen'>('idle')
  const [error, setError] = useState<MessageKey | null>(null)

  async function start(password: string) {
    setError(null)
    const r = await api.post<{ secret: string; qr: string }>('/api/totp/setup', { password })
    if (r.ok) {
      setGate(false)
      setSetup(r.data)
    } else if (r.code === 'enabled') {
      // 다른 탭에서 이미 켰다: 틀린 게 아니라 화면이 뒤처진 것이므로 상태를 새로 받는다.
      setGate(false)
      void refreshUser()
    } else setError(errorKey(r.code))
  }

  async function enable(value: string) {
    setError(null)
    const r = await api.post<{ recoveryCodes: string[] }>('/api/totp/enable', { code: value })
    setCode('')
    // 세션 만료(401)·서버 오류는 틀린 코드가 아니다. 만료면 로그인 상태를 다시 확인해 로그인 화면으로 간다.
    if (!r.ok) return setError(r.status === 429 ? 'auth.locked' : errorKey(r.code))
    setSetup(null)
    setCodes(r.data.recoveryCodes)
    void refreshUser()
  }

  async function withCode(value: string) {
    setError(null)
    const path = mode === 'disable' ? '/api/totp/disable' : '/api/totp/recovery'
    const r = await api.post<{ recoveryCodes?: string[] } | null>(path, { code: value })
    setCode('')
    if (!r.ok) return setError(errorKey(r.code))
    if (r.data?.recoveryCodes) setCodes(r.data.recoveryCodes)
    setMode('idle')
    void refreshUser()
  }

  return (
    <Card
      title={t('account.twoStepApp')}
      action={
        // 켜는 중(비밀번호 확인·QR)이면 켜진 것으로, 끄는 확인 중이면 꺼진 것으로 보인다. 다시 누르면 그 단계를 접는다.
        <Switch
          checked={user.totp ? mode !== 'disable' : gate || !!setup}
          onChange={(on) => {
            setError(null)
            setCodes(null)
            setCode('')
            if (user.totp) setMode(on ? 'idle' : 'disable')
            else if (on) setGate(true)
            else {
              setGate(false)
              setSetup(null)
            }
          }}
          label={t('account.twoStepApp')}
          hideLabel
        />
      }
    >
      <p className="card-lead">{t('account.twoStepLead')}</p>
      {gate && !setup && (
        <div className="settings-row column">
          <PasswordGate user={user} label={t('account.continue')} onPass={(p) => void start(p)} onCancel={() => setGate(false)} />
        </div>
      )}
      {setup && (
        <div className="totp-setup">
          {/* 서버가 만든 QR SVG를 이미지로 보인다(이미지 안의 SVG는 스크립트가 돌지 않는다). */}
          <img className="totp-qr" src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(setup.qr)}`} alt={t('account.totpQrAlt')} width={168} height={168} />
          <div className="totp-steps">
            <b>{t('account.totpNew')}</b>
            <ol>
              <li>{t('account.totpStep1')}</li>
              <li>{t('account.totpStep2')}</li>
            </ol>
            <p className="form-note">
              {t('account.totpKey')} <code className="secret">{setup.secret}</code>
            </p>
            <CodeInput value={code} onChange={setCode} onComplete={(v) => void enable(v)} label={t('verify.code')} autoFocus />
            <button
              className="link-btn"
              onClick={() => {
                setSetup(null)
                setCode('')
                setError(null)
              }}
            >
              {t('room.cancel')}
            </button>
          </div>
        </div>
      )}
      {mode !== 'idle' && (
        <div className="settings-row column">
          <p className="form-note">{t(mode === 'disable' ? 'account.totpDisablePrompt' : 'account.totpRegenPrompt')}</p>
          <CodeInput value={code} onChange={setCode} onComplete={(v) => void withCode(v)} label={t('verify.code')} autoFocus />
          <button className="link-btn" onClick={() => setMode('idle')}>
            {t('room.cancel')}
          </button>
        </div>
      )}
      {codes && <RecoveryCodes codes={codes} />}
      {user.totp && mode === 'idle' && !codes && (
        <div className="settings-row">
          <div className="row-text">
            <b>{t('account.recovery')}</b>
            <small>{t('account.recoveryLeft', { n: String(user.recoveryLeft) })}</small>
          </div>
          <button className="link-btn" onClick={() => setMode('regen')}>
            {t('account.regenerate')}
          </button>
        </div>
      )}
      <Message error={error} />
    </Card>
  )
}

function PasswordCard({ user }: { user: User }) {
  const { t, locale } = useI18n()
  const [open, setOpen] = useState(false)
  const [linking, setLinking] = useState(false)
  const [current, setCurrent] = useState('')
  const [next, setNext] = useState('')
  const [busy, setBusy] = useState(false)
  // Google 연결이 실패하면 서버가 ?error=google로 돌려보낸다.
  const [error, setError] = useState<MessageKey | null>(() => (queryParam('error') === 'google' ? 'account.googleLinkFailed' : null))
  useEffect(() => {
    if (queryParam('error')) replacePath('/account/security')
  }, [])
  const [note, setNote] = useState<MessageKey | null>(null)
  // Google에서 뒤로 가기로 돌아오면(bfcache) 연결 중 상태가 남아 있으므로 풀어 준다.
  useEffect(() => {
    const back = (e: PageTransitionEvent) => {
      if (!e.persisted) return
      setBusy(false)
      setLinking(false)
    }
    addEventListener('pageshow', back)
    return () => removeEventListener('pageshow', back)
  }, [])

  async function save(e: FormEvent) {
    e.preventDefault()
    if ([...next].length < 8) return setError('auth.passwordShort')
    setBusy(true)
    setError(null)
    const r = await api.post('/api/account/password', { current, new: next })
    setBusy(false)
    if (!r.ok) return setError(errorKey(r.code))
    setOpen(false)
    setCurrent('')
    setNext('')
    setNote('account.passwordChanged')
    void refreshUser()
  }

  // Google 연결은 비밀번호 없이 들어올 길이 하나 더 생기므로 한 번 더 확인한 뒤 Google로 보낸다.
  async function link(password: string) {
    if (busy) return
    setBusy(true)
    setError(null)
    const r = await api.post<{ url: string }>('/api/auth/google/link', { password, locale })
    if (r.ok) return location.assign(r.data.url)
    setBusy(false)
    setLinking(false)
    setError(errorKey(r.code))
  }

  async function unlink() {
    const r = await api.post('/api/auth/google/unlink')
    if (r.ok) void refreshUser()
    else setError(errorKey(r.code))
  }

  return (
    <Card title={t('account.passwordAndLinked')}>
      {!open ? (
        <Row label={t('auth.password')}>
          <span>{user.hasPassword ? (user.passwordChangedAt ? t('account.changedAgo', { when: relativeTime(user.passwordChangedAt, locale) }) : t('account.set')) : t('account.notSet')}</span>
          <button className="link-btn" onClick={() => setOpen(true)}>
            {t(user.hasPassword ? 'account.change' : 'account.setPassword')}
          </button>
        </Row>
      ) : (
        <form className="settings-row column" onSubmit={save}>
          <span className="settings-label">{t('auth.password')}</span>
          {user.hasPassword && (
            <input className="input" type="password" autoComplete="current-password" placeholder={t('account.currentPassword')} value={current} onChange={(e) => setCurrent(e.target.value)} aria-label={t('account.currentPassword')} autoFocus />
          )}
          <input className="input" type="password" autoComplete="new-password" placeholder={t('reset.new')} value={next} onChange={(e) => setNext(e.target.value)} aria-label={t('reset.new')} />
          <p className="form-note">{t('reset.note')}</p>
          <Message error={error} />
          <div className="row">
            <button className="btn btn-ink" disabled={busy}>
              {t('account.save')}
            </button>
            <button type="button" className="btn btn-outline" onClick={() => setOpen(false)}>
              {t('room.cancel')}
            </button>
          </div>
        </form>
      )}
      <Row label={t('account.google')}>
        <span>{t(user.google ? 'account.connected' : 'account.notConnected')}</span>
        {user.google ? (
          <button className="link-btn danger" onClick={() => void unlink()}>
            {t('account.disconnect')}
          </button>
        ) : (
          features().google &&
          !linking && (
            <button className="link-btn" onClick={() => setLinking(true)}>
              {t('account.connect')}
            </button>
          )
        )}
      </Row>
      {linking && (
        <div className="settings-row">
          <PasswordGate user={user} label={t('account.continue')} onPass={(p) => void link(p)} busy={busy} />
        </div>
      )}
      {!open && <Message error={error} note={note} />}
    </Card>
  )
}

function DevicesCard({ user }: { user: User }) {
  const { t, locale } = useI18n()
  const [error, setError] = useState<MessageKey | null>(null)

  async function out(id: string) {
    const r = await api.del(`/api/account/sessions/${id}`)
    if (r.ok) void refreshUser()
    else setError('account.failed')
  }

  async function others() {
    const r = await api.post('/api/account/sessions/revoke')
    if (r.ok) void refreshUser()
    else setError('account.failed')
  }

  return (
    <Card title={t('account.devices')}>
      {user.devices.map((d) => (
        <div className="settings-row" key={d.id}>
          <span>{deviceLabel(d.userAgent)}</span>
          {d.current ? (
            <span className="ok-text">{t('account.thisDevice')}</span>
          ) : (
            <span className="settings-value">
              <span className="muted">{relativeTime(d.lastSeenAt ?? d.createdAt, locale)}</span>
              <button className="link-btn danger" onClick={() => void out(d.id)}>
                {t('auth.logout')}
              </button>
            </span>
          )}
        </div>
      ))}
      {user.devices.length > 1 && (
        <div className="settings-row end">
          <button className="link-btn danger" onClick={() => void others()}>
            {t('account.logoutOthers')}
          </button>
        </div>
      )}
      <Message error={error} />
    </Card>
  )
}
