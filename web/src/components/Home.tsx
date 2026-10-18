import { lazy, Suspense, useEffect, useState, type FormEvent } from 'react'
import { api, createRoom, type Billing, type Meeting, type User } from '../lib/api'
import { formatDate, formatDateTime, formatDuration, relativeTime } from '../lib/format'
import { languageName } from '../lib/languages'
import { meetingPath } from '../lib/meetings'
import { refreshAuth, useAuth, userName } from '../lib/auth'
import { useI18n, type MessageKey } from '../lib/i18n'
import { navigate, parseRoomCode } from '../lib/router'
import { InLink } from './AuthBits'
import { BrandMark } from './BrandMark'
import { LoadBoundary } from './ErrorPages'
import { Header } from './Header'
import { Icon } from './Icon'
import { UnpaidNotice } from './UnpaidNotice'

type Kind = 'meeting' | 'inPerson' | 'live'

export function Home() {
  const auth = useAuth()
  if (auth.status === 'loading') return null
  return (
    <div className="shell">
      <Header />
      {auth.status === 'host' ? <HostHome user={auth.user} /> : <GuestHome />}
    </div>
  )
}

/** 받은 링크나 코드를 붙여 넣어 들어가는 칸. 오류 문구는 키로 두어 화면 언어를 바꾸면 함께 바뀐다. */
function JoinForm({ primary }: { primary?: boolean }) {
  const { t } = useI18n()
  const [input, setInput] = useState('')
  const [error, setError] = useState<MessageKey | null>(null)

  function join(e: FormEvent) {
    e.preventDefault()
    const code = parseRoomCode(input)
    if (code) navigate(`/${code}`)
    else setError('home.invalidCode')
  }

  return (
    <form className="join-form" onSubmit={join}>
      <div className="join-row">
        <label className="field">
          <Icon name="link" />
          <input
            value={input}
            onChange={(e) => {
              setInput(e.target.value)
              setError(null)
            }}
            placeholder={t('home.codePlaceholder')}
            aria-label={t('home.codeLabel')}
          />
        </label>
        <button className={`btn ${primary ? 'btn-primary btn-lg' : 'btn-ink'}`} disabled={!input.trim()}>
          {t('home.join')}
        </button>
      </div>
      {error && (
        <p className="form-error" role="alert">
          {t(error)}
        </p>
      )}
    </form>
  )
}

/** 로그인하지 않은 사람: 받은 링크로 들어오는 곳. 회의를 여는 사람은 로그인으로 안내한다. */
function GuestHome() {
  const { t } = useI18n()
  return (
    <main className="guest-home">
      <BrandMark size={64} />
      <h1>{t('home.guestTitle')}</h1>
      <p className="lead">{t('home.guestHint')}</p>
      <JoinForm primary />
      <p className="guest-foot">
        {t('home.hostPrompt')}{' '}
        <a
          href="/login"
          onClick={(e) => {
            e.preventDefault()
            navigate('/login')
          }}
        >
          {t('auth.hostLogin')}
        </a>
      </p>
    </main>
  )
}

// 예약 대화상자·회의 목록 도우미는 회의 목록 화면과 같은 묶음에서 필요할 때 받는다.
const ScheduleDialog = lazy(() => import('./MeetingsPage').then((m) => ({ default: m.ScheduleDialog })))
const ScheduledDialog = lazy(() => import('./MeetingsPage').then((m) => ({ default: m.ScheduledDialog })))

/** 로그인한 주최자: 인사·코드로 참여, 다음 회의(또는 지금 시작), 이번 달 번역 시간, 이 기기에서 바로, 예정·지난 회의. */
function HostHome({ user }: { user: User }) {
  const { t, locale } = useI18n()
  const [creating, setCreating] = useState<Kind | null>(null)
  const [error, setError] = useState<MessageKey | null>(null)
  const [meetings, setMeetings] = useState<{ upcoming: Meeting[]; past: Meeting[] } | null>(null)
  const [meetingsFailed, setMeetingsFailed] = useState(false)
  const [billing, setBilling] = useState<Billing | null>(null)
  const [scheduling, setScheduling] = useState(false)
  const [done, setDone] = useState<Meeting | null>(null)
  const [copied, setCopied] = useState(false)
  const [n, setN] = useState(0)
  const today = new Date().toLocaleDateString(locale, { month: 'long', day: 'numeric', weekday: 'long' })
  const name = userName(user)

  // 회의 목록과 사용량은 첫 화면과 함께 병렬로 받는다(서로 기다리지 않게).
  useEffect(() => {
    let alive = true
    void api.get<{ upcoming: Meeting[]; past: Meeting[] }>('/api/meetings').then((r) => {
      if (!alive) return
      setMeetingsFailed(!r.ok)
      if (!r.ok) return
      // 홈에는 끝날 시각이 아직 안 된 회의만(시간이 지났지만 링크는 쓸 수 있는 회의는 회의 목록에서 본다).
      const now = Date.now() / 1000
      setMeetings({ ...r.data, upcoming: r.data.upcoming.filter((m) => (m.startsAt ?? 0) + m.durationMin * 60 > now) })
    })
    if (user.plan !== 'none')
      void api.get<Billing>('/api/billing').then((r) => {
        if (!alive || !r.ok) return
        setBilling(r.data)
        // 화면을 연 사이 일일권이 끝났다: 계정 정보를 다시 받아 요금제 고르기 화면으로 바꾼다.
        if (r.data.plan === 'none') void refreshAuth()
      })
    return () => {
      alive = false
    }
  }, [n, user.plan])

  async function create(kind: Kind) {
    if (creating) return
    setCreating(kind)
    setError(null)
    // 대면 대화·자막 화면도 회의실 하나를 쓴다(번역 켜짐). 번역은 회의 중에 주최자가 끌 수 있다.
    const r = await createRoom(true, kind)
    if (r.kind === 'ok') return navigate(meetingPath({ code: r.code, kind }))
    setCreating(null)
    if (r.kind === 'plan') return navigate('/checkout')
    // 세션이 만료됐을 수 있으니 로그인 상태를 다시 확인한다.
    void refreshAuth()
    setError('home.createFailed')
  }

  const next = meetings?.upcoming[0]
  const noPlan = user.plan === 'none'
  return (
    <main className="host-home">
      <div className="host-top">
        <div>
          <p className="status-eyebrow">{today}</p>
          <h1>{t('home.hello', { name })}</h1>
        </div>
        <JoinForm />
      </div>
      {error && (
        <p className="form-error" role="alert">
          {t(error)}
        </p>
      )}
      {meetingsFailed && (
        <p className="form-error" role="alert">
          {t('home.meetingsFailed')}{' '}
          <button className="link-btn" onClick={() => setN((x) => x + 1)}>
            {t('room.retry')}
          </button>
        </p>
      )}
      <div className="host-grid">
        <div className="host-col">
          {next ? (
            <section className="start-card" aria-labelledby="next-title">
              <p className="next-eyebrow">{t('home.nextMeeting', { when: relativeTime(next.startsAt ?? 0, locale) })}</p>
              <h2 id="next-title">{next.title}</h2>
              <p>
                {formatDateTime(next.startsAt ?? 0, locale)}
                {next.langA && next.langB && ` · ${languageName(next.langA)} ⇄ ${languageName(next.langB)}`}
                {next.guests.length > 0 && ` · ${t('home.guestsInvited', { n: String(next.guests.length) })}`}
              </p>
              <div className="row">
                <button className="btn btn-primary" onClick={() => navigate(meetingPath(next))}>
                  {t('home.enterNow')}
                </button>
                {/* 자막 화면·대면 대화는 주최자 기기에서만 열리므로 보낼 링크가 없다. */}
                {next.kind === 'meeting' && (
                  <button
                    className="btn btn-glass"
                    onClick={async () => {
                      try {
                        await navigator.clipboard.writeText(`${location.origin}/${next.code}`)
                        setCopied(true)
                        setTimeout(() => setCopied(false), 2000)
                      } catch {
                        setError('account.failed')
                      }
                    }}
                  >
                    {copied ? t('room.linkCopied') : t('home.copyInvite')}
                  </button>
                )}
                <button className="btn btn-glass" onClick={() => navigate('/meetings')}>
                  {t('home.allMeetings')}
                </button>
              </div>
              <span className="start-mark" aria-hidden="true">
                <BrandMark size={44} />
              </span>
            </section>
          ) : (
            <section className="start-card" aria-labelledby="start-title">
              <BrandMark size={44} />
              <h2 id="start-title">{t('home.startTitle')}</h2>
              <p>{t('home.startBody')}</p>
              <button className="btn btn-primary btn-lg" onClick={() => void create('meeting')} disabled={!!creating}>
                <Icon name="video_call" />
                {creating === 'meeting' ? t('home.creating') : t(noPlan ? 'home.choosePlanFirst' : 'home.startNow')}
              </button>
            </section>
          )}
          <div className="quick-tiles">
            {next && (
              <button className="quick-tile" onClick={() => void create('meeting')} disabled={!!creating}>
                <span className="quick-icon accent" aria-hidden="true">
                  <Icon name="video_call" />
                </span>
                <span>
                  <b>{creating === 'meeting' ? t('home.creating') : t('home.startNow')}</b>
                  <small>{t('home.startNowBody')}</small>
                </span>
              </button>
            )}
            <button className="quick-tile" onClick={() => (noPlan ? navigate('/checkout') : setScheduling(true))}>
              <span className="quick-icon" aria-hidden="true">
                <Icon name="calendar_month" />
              </span>
              <span>
                <b>{t('meetings.schedule')}</b>
                <small>{t('home.scheduleBody')}</small>
              </span>
            </button>
          </div>
          {meetings && meetings.upcoming.length > 0 && (
            <section className="list-card" aria-labelledby="upcoming-title">
              <h2 id="upcoming-title">{t('home.upcoming')}</h2>
              <ul>
                {meetings.upcoming.slice(0, 4).map((m) => (
                  <li key={m.id}>
                    <span className="date-chip" aria-hidden="true">
                      <small>{new Date((m.startsAt ?? 0) * 1000).toLocaleDateString(locale, { month: 'short' })}</small>
                      <b>{new Date((m.startsAt ?? 0) * 1000).getDate()}</b>
                    </span>
                    <span className="list-main">
                      <b>{m.title}</b>
                      <small>
                        {formatDateTime(m.startsAt ?? 0, locale)}
                        {m.kind !== 'meeting' && ` · ${t(m.kind === 'live' ? 'newMeeting.live' : 'newMeeting.inPerson')}`}
                        {m.langA && m.langB && ` · ${languageName(m.langA)} ⇄ ${languageName(m.langB)}`}
                      </small>
                    </span>
                    {m.kind === 'meeting' ? (
                      <button className="btn btn-sm btn-outline" onClick={() => setDone(m)}>
                        {t('meetings.inviteInfo')}
                      </button>
                    ) : (
                      <button className="btn btn-sm btn-outline" onClick={() => navigate(meetingPath(m))}>
                        {t('home.open')}
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            </section>
          )}
        </div>
        <div className="host-col">
          {noPlan ? (
            <section className="plan-cta" aria-labelledby="plan-title">
              <h2 id="plan-title">{t('home.planTitle')}</h2>
              <p>{t('home.planBody')}</p>
              <InLink to="/checkout" className="btn btn-primary">
                {t('account.choosePlan')}
              </InLink>
            </section>
          ) : (
            billing && billing.plan !== 'none' && <UsageRing billing={billing} />
          )}
          <section className="device-card" aria-labelledby="device-title">
            <h2 id="device-title">{t('home.onThisDevice')}</h2>
            <div className="device-options">
              <button className="device-option" onClick={() => void create('inPerson')} disabled={!!creating}>
                <Icon name="forum" />
                <b>{creating === 'inPerson' ? t('home.creating') : t('newMeeting.inPerson')}</b>
                <span>{t('home.inPersonShort')}</span>
              </button>
              <button className="device-option" onClick={() => void create('live')} disabled={!!creating}>
                <Icon name="subtitles" />
                <b>{creating === 'live' ? t('home.creating') : t('newMeeting.live')}</b>
                <span>{t('home.liveShort')}</span>
              </button>
            </div>
          </section>
          {meetings && meetings.past.length > 0 && (
            <section className="list-card compact" aria-labelledby="past-title">
              <h2 id="past-title">{t('home.past')}</h2>
              <ul>
                {meetings.past.slice(0, 4).map((m) => (
                  <li key={m.id}>
                    <span className="list-main">
                      <b>{m.title || t('meetings.untitled')}</b>
                    </span>
                    <small className="muted">
                      {formatDate(m.startedAt ?? m.startsAt ?? m.createdAt, locale)} · {formatDuration(m.lengthSec, t)}
                    </small>
                  </li>
                ))}
              </ul>
            </section>
          )}
        </div>
      </div>
      {/* 대화상자 조각을 받지 못해도 홈 전체가 비지 않게 막는다. 실패한 import는 기억되므로 다시 받으려면 새로 고친다. */}
      <LoadBoundary
        fallback={
          <p className="form-error" role="alert">
            {t('status.loadFailedTitle')}{' '}
            <button className="link-btn" onClick={() => location.reload()}>
              {t('status.reload')}
            </button>
          </p>
        }
      >
        <Suspense fallback={null}>
          {scheduling && (
            <ScheduleDialog
              meeting={null}
              onClose={() => setScheduling(false)}
              onSaved={(m) => {
                setScheduling(false)
                setN((x) => x + 1)
                setDone(m)
              }}
            />
          )}
          {done && <ScheduledDialog meeting={done} onClose={() => setDone(null)} />}
        </Suspense>
      </LoadBoundary>
    </main>
  )
}

/** 이번 결제 기간의 번역 시간: 남은 비율 고리 + 남은 시간 + 다시 채워지는 날 */
function UsageRing({ billing }: { billing: Billing }) {
  const { t, locale } = useI18n()
  const day = billing.plan === 'day'
  const total = billing.includedSec + billing.extraSec
  // 일일권은 한 장(3시간) 기준으로 쓴 비율을 보인다(남은 시간이 더 많으면 0%).
  const usedPct = day
    ? Math.max(0, 100 - Math.round((billing.daySec / (3 * 3600)) * 100))
    : total > 0
      ? Math.min(100, Math.round((billing.usedSec / Math.max(billing.includedSec, 1)) * 100))
      : 0
  const r = 44
  const c = 2 * Math.PI * r
  return (
    <section className="usage-card" aria-labelledby="usage-title">
      <svg className="ring" viewBox="0 0 110 110" role="img" aria-label={t('home.usedPct', { n: String(usedPct) })}>
        <circle cx="55" cy="55" r={r} className="ring-bg" />
        <circle cx="55" cy="55" r={r} className="ring-fg" strokeDasharray={`${(usedPct / 100) * c} ${c}`} transform="rotate(-90 55 55)" />
        <text x="55" y="61" textAnchor="middle">
          {usedPct}%
        </text>
      </svg>
      <div>
        <p className="usage-label" id="usage-title">
          {day ? t('plan.dayPass') : t('home.thisPeriod')}
        </p>
        <p className="usage-left">{t('home.left', { time: formatDuration(billing.remainingSec, t) })}</p>
        <p className="muted">
          {day
            ? billing.dayUntil != null && t('plan.dayUntil', { date: formatDateTime(billing.dayUntil, locale) })
            : `${billing.plan === 'pro' ? 'Pro' : 'Basic'} · ${t('account.resetsOn', { date: formatDate(billing.resetsAt, locale) })}`}
        </p>
        <InLink to={day ? '/checkout?item=basic' : '/checkout?item=hours'} className="auth-link">
          {t(day ? 'plan.subscribe' : 'account.addHours')}
        </InLink>
        {billing.pastDue && <UnpaidNotice />}
      </div>
    </section>
  )
}
