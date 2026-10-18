import { useEffect, useState } from 'react'
import { api, createRoom, openPortal, type Billing, type Plan, type User } from '../lib/api'
import { features, refreshAuth, setUser } from '../lib/auth'
import { useI18n, type MessageKey } from '../lib/i18n'
import '../lib/i18nPages'
import { formatDateTime } from '../lib/format'
import { legalUrl } from '../lib/links'
import { navigate, queryParam, useHostOnly } from '../lib/router'
import { BrandMark } from './BrandMark'
import { Header } from './Header'
import { Icon } from './Icon'
import { Rich } from './Rich'

type Item = 'basic' | 'pro' | 'hours' | 'day'

type CheckoutData = {
  clientToken: string
  sandbox: boolean
  /** 서버가 만든 Paddle 거래(금액·사용자 서명이 들어 있다). 옛 서버는 주지 않는다. */
  transactionId?: string
  priceId: string
  quantity: number
  email: string
  customData: Record<string, string>
}

type PaddleGlobal = {
  Environment: { set: (env: string) => void }
  Initialize: (o: { token: string; eventCallback?: (e: { name: string }) => void }) => void
  Checkout: { open: (o: object) => void }
}

/** Paddle.js는 결제 화면에서만 받는다(다른 화면의 첫 로드에 들어가지 않게). */
let paddleLoading: Promise<PaddleGlobal> | null = null
/** Paddle.js v2는 한 번만 초기화한다(결제 창을 닫았다 다시 열 때 또 부르지 않게). */
let paddleReady = false
function loadPaddle(): Promise<PaddleGlobal> {
  paddleLoading ??= new Promise((resolve, reject) => {
    const s = document.createElement('script')
    s.src = 'https://cdn.paddle.com/paddle/v2/paddle.js'
    s.async = true
    s.onload = () => {
      const p = (window as unknown as { Paddle?: PaddleGlobal }).Paddle
      if (p) resolve(p)
      else reject(new Error('paddle'))
    }
    s.onerror = () => {
      paddleLoading = null
      reject(new Error('paddle'))
    }
    document.head.appendChild(s)
  })
  return paddleLoading
}

/** 지금 화면이 어두운지(시스템 설정을 따르는 경우 포함) */
function isDark(): boolean {
  const chosen = document.documentElement.dataset.theme
  return chosen ? chosen === 'dark' : matchMedia('(prefers-color-scheme: dark)').matches
}

// 서버 dev_activate의 영수증 금액·소개 페이지 요금표와 같다. 바꿀 때 함께 바꾼다.
const PRICE: Record<Exclude<Item, 'hours'>, string> = { basic: '$19', pro: '$49', day: '$12' }
const HOURS = [1, 5, 10]
/** 서버가 한 번에 받는 시간 추가 수량(meetings.rs checkout) */
const MAX_HOURS = 100

/**
 * 시간을 사기 전의 추가 시간 잔액. 결제 완료 화면이 웹훅으로 늘어난 것을 확인할 때 비교한다.
 * 저장소가 막힌 브라우저(사생활 보호 모드 등)에서는 쓰지 못하므로, 그때는 잔액이 0보다 큰지로 판단한다.
 */
const HOURS_BEFORE = 'tolkmeet.hoursBefore'
/** 이보다 오래된 기록은 이번 결제의 것이 아니다. */
const HOURS_BEFORE_TTL = 60 * 60_000

function saveHoursBefore(extraSec: number) {
  try {
    sessionStorage.setItem(HOURS_BEFORE, JSON.stringify({ extraSec, at: Date.now() }))
  } catch {
    // 저장하지 못하면 완료 화면이 잔액 > 0으로 판단한다.
  }
}

function readHoursBefore(): number | null {
  try {
    const v = JSON.parse(sessionStorage.getItem(HOURS_BEFORE) ?? 'null') as { extraSec?: unknown; at?: unknown } | null
    if (v && typeof v.extraSec === 'number' && typeof v.at === 'number' && Date.now() - v.at < HOURS_BEFORE_TTL) return v.extraSec
  } catch {
    // 읽지 못하면 기록이 없는 것으로 본다.
  }
  return null
}

function clearHoursBefore() {
  try {
    sessionStorage.removeItem(HOURS_BEFORE)
  } catch {
    // 지우지 못해도 한 시간이 지나면 쓰지 않는다.
  }
}

export function Checkout() {
  const { t, locale } = useI18n()
  // 로그인 전이면 가입부터(가입 뒤 이 화면으로 돌아온다). 여기서 로그아웃했으면 홈으로.
  const auth = useHostOnly('/signup')
  const [item, setItem] = useState<Item>(() => {
    const q = queryParam('item') ?? queryParam('plan')
    return q === 'pro' || q === 'hours' || q === 'day' ? q : 'basic'
  })
  const [qty, setQty] = useState(() => {
    const q = Number(queryParam('q'))
    return HOURS.includes(q) ? q : 5
  })
  const [busy, setBusy] = useState(false)
  // 결제 정보는 판매 대행사 Paddle(영국)에 제공·국외 이전되므로 결제 전에 따로 동의를 받는다(개인정보 보호법 제17조·제28조의8).
  const [consentProvide, setConsentProvide] = useState(false)
  const [consentTransfer, setConsentTransfer] = useState(false)
  const consent = consentProvide && consentTransfer
  const [needReconsent, setNeedReconsent] = useState(false)
  const [error, setError] = useState<MessageKey | null>(null)

  if (auth.status !== 'host') return <div className="shell" />
  const user = auth.user
  // 구독 중인지(일일권만 남은 사람은 구독이 없다).
  const subscribed = user.plan === 'basic' || user.plan === 'pro'
  // 시간 추가는 구독자만, 일일권은 구독하지 않은 사람만 산다. 맞지 않으면 Basic을 보인다.
  const sel: Item = (item === 'hours' && !subscribed) || (item === 'day' && subscribed) ? 'basic' : item
  const isPlan = sel === 'basic' || sel === 'pro'
  // 이미 구독 중이면 다른 요금제는 "바꾸기"로 처리한다(올리면 남은 기간의 차액만 바로 청구, 내리면 청구·환불 없이 다음 결제부터 낮은 요금).
  const changing = isPlan && subscribed && user.plan !== sel
  const same = isPlan && user.plan === sel

  async function pay() {
    setBusy(true)
    setError(null)
    if (changing) {
      const r = await api.post('/api/billing/change', { plan: sel, consentProvide, consentTransfer })
      setBusy(false)
      if (!r.ok) {
        // 동의를 철회한 사람: 두 동의 항목을 보여 다시 동의하게 한다.
        if (r.code === 'consent') {
          setNeedReconsent(true)
          return setError('checkout.consentWithdrawn')
        }
        // 지난 결제가 실패한 구독은 Paddle이 바꾸지 않는다. 결제 수단부터 고치게 안내한다.
        if (r.code === 'past_due') return setError('checkout.pastDue')
        return setError('account.billingFailed')
      }
      return navigate(`/billing/done?item=${sel}&change=1`)
    }
    // 시간을 살 때는 지금 잔액을 함께 받아 둔다(완료 화면이 늘어난 것을 확인한다). 결제 요청과 병렬이라 기다림이 늘지 않는다.
    const [r, before] = await Promise.all([
      api.post<CheckoutData | { dev: true }>('/api/billing/checkout', {
        item: sel,
        quantity: sel === 'hours' ? qty : 1,
        consentProvide: consentProvide,
        consentTransfer: consentTransfer,
      }),
      sel === 'hours' ? api.get<Billing>('/api/billing') : null,
    ])
    if (!r.ok) {
      setBusy(false)
      // 앞선 구독 결제가 아직 확인 중이다(웹훅 전). 두 번 결제되지 않게 서버가 막는다.
      if (r.code === 'pending') return setError('checkout.pending')
      // 요금제가 없는데 시간을 사려 했다(그사이 요금제가 끝난 경우 등). 계정 정보를 다시 받아 요금제 고르기로 돌린다.
      if (r.code === 'plan') {
        void refreshAuth()
        return setError('checkout.planNeeded')
      }
      // 이미 구독 중이다(다른 탭·기기에서 결제해 이 화면의 계정 정보가 오래됐다). 결제되지 않았다.
      // 계정 정보를 다시 받으면 이 화면이 요금제 바꾸기(같은 요금제면 "이미 이 요금제") 화면으로 바뀐다.
      if (r.code === 'subscribed') {
        await refreshAuth()
        return setError('checkout.subscribed')
      }
      // 버튼이 동의 전에는 꺼져 있으므로 보통은 오지 않는다. 서버가 동의 없음으로 거절하면 그대로 알린다.
      return setError(r.code === 'consent' ? 'checkout.consentNeeded' : 'account.billingFailed')
    }
    if (before?.ok) saveHoursBefore(before.data.extraSec)
    if ('dev' in r.data) {
      // 개발 모드: 결제 없이 켠다.
      const d = await api.post('/api/billing/dev', { item: sel, quantity: qty })
      setBusy(false)
      if (!d.ok) return setError('account.billingFailed')
      return navigate(`/billing/done?item=${sel}${sel === 'hours' ? `&q=${qty}` : ''}`)
    }
    const data = r.data
    try {
      const paddle = await loadPaddle()
      if (!paddleReady) {
        if (data.sandbox) paddle.Environment.set('sandbox')
        paddle.Initialize({ token: data.clientToken })
        paddleReady = true
      }
      const settings = {
        displayMode: 'overlay',
        theme: isDark() ? 'dark' : 'light',
        locale: document.documentElement.lang || 'en',
        successUrl: `${location.origin}/billing/done?item=${sel}${sel === 'hours' ? `&q=${qty}` : ''}`,
      }
      const customer = data.email ? { customer: { email: data.email } } : {}
      paddle.Checkout.open(
        data.transactionId
          ? // 서버가 만든 거래로 연다(항목·사용자 서명은 거래에 있다). Paddle.js는 이때 items를 받지 않는다.
            { transactionId: data.transactionId, ...customer, settings }
          : // 거래 id를 주지 않는 옛 서버
            { items: [{ priceId: data.priceId, quantity: data.quantity }], ...customer, customData: data.customData, settings },
      )
    } catch {
      setError('checkout.loadFailed')
    }
    setBusy(false)
  }

  const priceLabel = sel === 'hours' ? `$${qty * 3}` : PRICE[sel]
  return (
    <div className="shell">
      <Header />
      <main className="checkout">
        <section className="checkout-main" aria-labelledby="checkout-title">
          <h1 id="checkout-title">
            {changing
              ? t('checkout.change', { plan: sel === 'pro' ? 'Pro' : 'Basic' })
              : t(
                  sel === 'hours' ? 'checkout.hoursTitle' : sel === 'day' ? 'checkout.dayTitle' : sel === 'pro' ? 'checkout.proTitle' : 'checkout.basicTitle',
                )}
          </h1>
          <div className="choice-grid" role="radiogroup" aria-label={t('account.plan')}>
            {(['basic', 'pro'] as const).map((p) => (
              <button key={p} type="button" role="radio" aria-checked={sel === p} className={`choice${sel === p ? ' on' : ''}`} onClick={() => { setItem(p); setError(null) }}>
                <span className="choice-top">
                  {p === 'pro' ? 'Pro' : 'Basic'}
                  {user.plan === p && <span className="badge-soft">{t('checkout.current')}</span>}
                </span>
                <b>
                  {PRICE[p]}
                  <small> / {t('checkout.month')}</small>
                </b>
                <small>{t(p === 'pro' ? 'checkout.proHours' : 'checkout.basicHours')}</small>
              </button>
            ))}
            {!subscribed && (
              <button type="button" role="radio" aria-checked={sel === 'day'} className={`choice${sel === 'day' ? ' on' : ''}`} onClick={() => { setItem('day'); setError(null) }}>
                <span className="choice-top">
                  {t('plan.dayPass')}
                  {user.plan === 'day' && <span className="badge-soft">{t('checkout.current')}</span>}
                </span>
                <b>
                  {PRICE.day}
                  <small> / {t('checkout.dayUnit')}</small>
                </b>
                <small>{t('checkout.dayNote')}</small>
              </button>
            )}
            {subscribed && (
              <button type="button" role="radio" aria-checked={sel === 'hours'} className={`choice${sel === 'hours' ? ' on' : ''}`} onClick={() => { setItem('hours'); setError(null) }}>
                <span className="choice-top">{t('account.addHours')}</span>
                <b>
                  $3
                  <small> / {t('checkout.hour')}</small>
                </b>
                <small>{t('checkout.hoursNote')}</small>
              </button>
            )}
          </div>
          {sel === 'hours' && (
            <div className="segmented" role="radiogroup" aria-label={t('checkout.howMany')}>
              {HOURS.map((h) => (
                <button key={h} type="button" role="radio" aria-checked={qty === h} className={qty === h ? 'on' : ''} onClick={() => setQty(h)}>
                  {t('checkout.hours', { n: String(h) })}
                </button>
              ))}
            </div>
          )}
          <ul className="ticks">
            <li>
              {t(sel === 'pro' ? 'checkout.proHours' : sel === 'basic' ? 'checkout.basicHours' : sel === 'day' ? 'checkout.dayHours' : 'checkout.hoursKeep')}
            </li>
            <li>{t('checkout.allModes')}</li>
            <li>{t('checkout.guestsFree')}</li>
            <li>{t(sel === 'day' ? 'checkout.dayNote' : 'checkout.extra')}</li>
          </ul>
          <p className="fine">
            {t(sel === 'day' ? 'checkout.dayFine' : 'checkout.fine')}{' '}
            <a href={legalUrl('refund', locale)} target="_blank" rel="noreferrer">
              {t('checkout.refund')}
            </a>
          </p>
        </section>
        <section className="checkout-pay" aria-labelledby="pay-title">
          <h2 id="pay-title">{t('checkout.summary')}</h2>
          <div className="pay-line">
            <span>{t('auth.email')}</span>
            <span title={user.email}>{user.email}</span>
          </div>
          <div className="pay-line">
            <span>
              {sel === 'hours'
                ? t('checkout.hours', { n: String(qty) })
                : sel === 'day'
                  ? `${t('plan.dayPass')} · ${t('checkout.dayUnit')}`
                  : `${sel === 'pro' ? 'Pro' : 'Basic'} · ${t('account.monthly')}`}
            </span>
            <span>{priceLabel}</span>
          </div>
          {changing ? (
            // 요금제 바꾸기는 결제창 없이 처리된다. 올리면 남은 기간의 차액이 등록된 결제 수단으로 바로 청구되고, 내리면 지금은 청구도 환불도 없다.
            <p className="form-note">{t(sel === 'basic' ? 'checkout.downgradeNote' : 'checkout.changeNote')}</p>
          ) : (
            <div className="pay-line muted">
              <span>{t('checkout.tax')}</span>
              <span>{t('checkout.taxLater')}</span>
            </div>
          )}
          {/* 같은 요금제로 바뀌었으면 아래 "이미 이 요금제" 안내로 충분하다. */}
          {error && !(error === 'checkout.subscribed' && same) && (
            <p className="form-error" role="alert">
              {t(error)}
              {error === 'checkout.pastDue' && (
                <>
                  {' '}
                  <button
                    className="link-btn"
                    disabled={busy}
                    onClick={() => {
                      setBusy(true)
                      void openPortal().then((ok) => {
                        setBusy(false)
                        if (!ok) setError('account.billingFailed')
                      })
                    }}
                  >
                    {t('checkout.updatePayment')}
                  </button>
                </>
              )}
            </p>
          )}
          {!same && (!changing || needReconsent) && (
            <div className="consent-box" id="paddle-consent">
              <label className="check-row">
                <input type="checkbox" checked={consentProvide} onChange={(e) => setConsentProvide(e.target.checked)} />
                <span>{t('checkout.consentProvide')}</span>
              </label>
              <label className="check-row">
                <input type="checkbox" checked={consentTransfer} onChange={(e) => setConsentTransfer(e.target.checked)} />
                <span>{t('checkout.consentTransfer')}</span>
              </label>
              <p className="fine">
                <Rich
                  text={t('checkout.consentNote')}
                  parts={{
                    privacy: (
                      <a href={legalUrl('privacy', locale)} target="_blank" rel="noreferrer">
                        {t('checkout.privacyLink')}
                      </a>
                    ),
                  }}
                />
              </p>
            </div>
          )}
          {same ? (
            <p className="form-note">{t('checkout.already')}</p>
          ) : (
            <button
              className="btn btn-primary btn-lg"
              disabled={busy || ((!changing || needReconsent) && !consent) || (!features().billing && !features().devBilling)}
              aria-describedby={(!changing || needReconsent) && !consent ? 'paddle-consent' : undefined}
              onClick={() => void pay()}
            >
              {changing ? t('checkout.change', { plan: sel === 'pro' ? 'Pro' : 'Basic' }) : t('checkout.pay', { price: priceLabel })}
            </button>
          )}
          {!changing && (
            <p className="fine center">
              <Icon name="lock_reset" />
              {t('checkout.secure')}
            </p>
          )}
        </section>
      </main>
    </div>
  )
}

export function PaidDone() {
  const { t, locale } = useI18n()
  // 결제는 로그인한 주최자만 하므로, 손님이면 로그인 뒤 이 화면으로 돌아오게 한다.
  const auth = useHostOnly('/login')
  // 주소는 누구나 고칠 수 있으므로 아는 값만 믿는다. 모르는 값이면 무엇을 샀는지 말하지 않는 화면을 보인다.
  const rawItem = queryParam('item')
  const item: Item | null = rawItem === 'basic' || rawItem === 'pro' || rawItem === 'hours' || rawItem === 'day' ? rawItem : null
  const q = Number(queryParam('q'))
  const hours = item === 'hours' && Number.isInteger(q) && q >= 1 && q <= MAX_HOURS ? q : null
  const [starting, setStarting] = useState(false)
  const user: User | null = auth.status === 'host' ? auth.user : null
  const [round, setRound] = useState(0)
  const plan: Plan | null = item === 'pro' || item === 'basic' ? item : null
  // 이미 구독하던 사람이 요금제를 바꾼 경우(결제 화면이 표시한다): 올리면 남은 기간만큼 시간이 늘고, 내리면 청구가 없다.
  const changed = plan != null && queryParam('change') === '1'
  // 새로 산 요금제는 켜진 뒤 그 기간의 결제가 끝나야(몇 초 뒤, transaction.completed) 포함 시간이 들어온다. 그때까지 준비됐다고 하지 않는다.
  const bought = plan != null && !changed
  // 요금제를 샀거나 바꿨으면 그 요금제가 될 때까지, 시간만 샀으면 요금제가 있을 때까지 확인한다.
  const waiting = (p: string | undefined) => (plan ? p !== plan : p === 'none')
  const [stalled, setStalled] = useState(false)
  // 시간을 샀으면 결제 전 잔액(결제 화면이 남긴 값)보다 늘어야 들어온 것이다. 값이 없으면 잔액이 있는지로 본다.
  const [hoursBefore] = useState(() => (item === 'hours' ? readHoursBefore() : null))
  const [ready, setReady] = useState(false)
  // 일일권: 들어오면 끝나는 때가 방금 산 때부터 24시간이 된다(이미 남은 일일권이 있어도 늘어난다).
  const [dayUntil, setDayUntil] = useState<number | null>(null)

  // 결제 결과는 Paddle 웹훅으로 조금 늦게 들어올 수 있다. 요금제가 켜지거나 산 시간이 들어올 때까지 잠깐 다시 확인한다.
  useEffect(() => {
    // 요금제를 바꿨으면 15초쯤, 새로 산 요금제·시간은 30초쯤 기다린 뒤 멈추고 다시 확인할 수 있게 알린다.
    const [interval, maxTries] = item === 'hours' || item === 'day' ? [2000, 15] : bought ? [1500, 20] : [1500, 10]
    let tries = 0
    let stop = false
    let timer = 0
    async function poll() {
      let done: boolean
      if (item === 'hours') {
        const r = await api.get<Billing>('/api/billing')
        if (stop) return
        done = r.ok && (hoursBefore == null ? r.data.extraSec > 0 : r.data.extraSec > hoursBefore)
        if (done) clearHoursBefore()
      } else if (item === 'day') {
        // 끝나는 때가 지금부터 24시간 가까이 남았으면 이번 결제가 들어온 것이다(결제창을 연 뒤 10분까지 봐준다).
        const r = await api.get<Billing>('/api/billing')
        if (stop) return
        const until = r.ok ? r.data.dayUntil : null
        done = until != null && until >= Date.now() / 1000 + 86_400 - 600
        if (done) {
          setDayUntil(until)
          void refreshAuth()
        }
      } else {
        // 새로 산 요금제는 계정(머리글·버튼이 쓴다)과 이번 기간의 결제를 함께 본다. 병렬이라 기다림이 늘지 않는다.
        const [r, b] = await Promise.all([api.get<{ user: User | null }>('/api/me'), bought ? api.get<Billing>('/api/billing') : null])
        if (stop) return
        if (r.ok && r.data.user) setUser(r.data.user)
        done =
          r.ok &&
          !waiting(r.data.user?.plan) &&
          // 결제가 끝났다고 알려 주지 않는 옛 서버는 포함 시간이 들어왔는지로 본다.
          (b == null || (b.ok && b.data.plan === plan && (b.data.periodPaid || b.data.includedSec > 0)))
      }
      if (done) return setReady(true)
      if (tries++ < maxTries) timer = window.setTimeout(() => void poll(), interval)
      else setStalled(true)
    }
    void poll()
    return () => {
      stop = true
      clearTimeout(timer)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- item·plan·bought·hoursBefore는 주소와 첫 화면에서 한 번 정해진다
  }, [round])

  async function start() {
    setStarting(true)
    const r = await createRoom(true)
    if (r.kind === 'ok') navigate(`/${r.code}`)
    else {
      setStarting(false)
      void refreshAuth()
    }
  }

  if (!user) return <div className="shell" />
  // 새로 산 요금제가 아직 준비되지 않았으면 시간을 산 경우처럼 "결제를 받았어요"만 보인다.
  const planShown = plan != null && (ready || changed)
  const title = planShown
    ? t('paid.title', { plan: plan === 'pro' ? 'Pro' : 'Basic' })
    : ready && hours
      ? t('paid.hoursTitle', { n: String(hours) })
      : ready && item === 'day'
        ? t('paid.dayTitle')
        : t('paid.receivedTitle')
  const lead = planShown
    ? changed
      ? t(plan === 'pro' ? 'paid.upgradeLead' : 'paid.downgradeLead', { email: user.email })
      : t('paid.lead', { email: user.email, hours: plan === 'pro' ? '20' : '8' })
    : ready && item === 'day' && dayUntil != null
      ? t('paid.dayLead', { email: user.email, until: formatDateTime(dayUntil, locale) })
      : t(ready && item === 'hours' ? 'paid.hoursLead' : 'paid.receivedLead', { email: user.email })
  return (
    <div className="shell">
      <Header />
      <main className="status-page">
        <span className="status-brand big" aria-hidden="true">
          <BrandMark size={96} />
        </span>
        <div className="status-text" aria-live="polite">
          <h1>{title}</h1>
          <p className="lead">{lead}</p>
        </div>
        {planShown && (
          <ol className="steps3">
            <li>
              <b>{t('paid.step1')}</b>
              <span>{t('paid.step1Body')}</span>
            </li>
            <li>
              <b>{t('paid.step2')}</b>
              <span>{t('paid.step2Body')}</span>
            </li>
            <li>
              <b>{t('paid.step3')}</b>
              <span>{t('paid.step3Body')}</span>
            </li>
          </ol>
        )}
        {stalled && !ready && (
          <p className="form-note" role="status">
            {t(changed ? 'paid.changePending' : 'paid.pending')}{' '}
            <button
              className="link-btn"
              onClick={() => {
                setStalled(false)
                setRound((n) => n + 1)
              }}
            >
              {t('paid.checkAgain')}
            </button>
          </p>
        )}
        <div className="row status-actions">
          <button className="btn btn-primary btn-lg" onClick={() => void start()} disabled={starting || (bought || item === 'day' ? !ready : user.plan === 'none')}>
            {t(plan ? 'paid.firstMeeting' : 'home.startNow')}
          </button>
          <button className="btn btn-outline btn-lg" onClick={() => navigate('/')}>
            {t('room.home')}
          </button>
        </div>
      </main>
    </div>
  )
}
