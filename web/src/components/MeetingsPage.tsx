import { useEffect, useLayoutEffect, useRef, useState, type FormEvent } from 'react'
import { api, createRoom, type Meeting } from '../lib/api'
import { formatDate, formatDuration, formatTime, timeIn, zoneName } from '../lib/format'
import { useI18n, type MessageKey } from '../lib/i18n'
import '../lib/i18nPages'
import { LANGUAGES, languageName } from '../lib/languages'
import { inviteText, meetingPath, meetingUrl } from '../lib/meetings'
import { loadPrefs } from '../lib/prefs'
import { navigate, useHostOnly } from '../lib/router'
import { Header } from './Header'
import { Icon } from './Icon'
import { useModalDialog } from '../lib/modal'
import { Select } from './Select'

const LANG_OPTIONS = LANGUAGES.map((l) => ({ value: l.code as string, label: l.name }))

async function copy(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text)
    return true
  } catch {
    return false
  }
}

type Lists = { upcoming: Meeting[]; past: Meeting[]; pastMore?: boolean }

/** 지난 회의는 서버가 이만큼씩 나눠 준다(오래된 기록도 찾아 지울 수 있게). */
const PAST_PAGE = 50
const listUrl = (page: number) => (page ? `/api/meetings?pastOffset=${page * PAST_PAGE}` : '/api/meetings')

/** 목록과 받기 실패 여부. 다시 받는 동안에도 받아 둔 목록은 그대로 보인다. */
function useMeetings() {
  const [data, setData] = useState<Lists | null>(null)
  const [failed, setFailed] = useState(false)
  const [loadingMore, setLoadingMore] = useState(false)
  const [n, setN] = useState(0)
  /** 펼친 지난 회의 쪽 수 */
  const pages = useRef(1)
  useEffect(() => {
    let alive = true
    // 다시 받을 때는 펼쳐 둔 쪽까지 함께(병렬로) 받는다(지운 뒤에도 보던 자리가 남게).
    void Promise.all(Array.from({ length: pages.current }, (_, i) => api.get<Lists>(listUrl(i)))).then((rs) => {
      if (!alive) return
      if (!rs.every((r) => r.ok)) return setFailed(true)
      const lists = rs.map((r) => (r as { data: Lists }).data)
      setFailed(false)
      setData({ upcoming: lists[0].upcoming, past: lists.flatMap((l) => l.past), pastMore: lists[lists.length - 1].pastMore })
    })
    return () => {
      alive = false
    }
  }, [n])

  async function loadMore(): Promise<boolean> {
    if (loadingMore) return true
    setLoadingMore(true)
    const r = await api.get<Lists>(listUrl(pages.current))
    setLoadingMore(false)
    if (!r.ok) return false
    pages.current++
    // 그사이 끝난 회의가 생기면 쪽이 밀려 같은 회의가 다시 올 수 있다.
    setData((d) => d && { ...d, past: [...d.past, ...r.data.past.filter((m) => !d.past.some((p) => p.id === m.id))], pastMore: r.data.pastMore })
    return true
  }

  return { data, failed, reload: () => setN((x) => x + 1), loadMore, loadingMore }
}

type ToastState = { key: MessageKey; ok: boolean } | null

/** 잠깐 떴다 사라지는 알림. 대화상자 안에서 띄우면 그 안(맨 위 층)에 그려 배경에 가리지 않는다. */
function useToast(): [ToastState, (copied: boolean, ok: MessageKey) => void] {
  const [toast, setToast] = useState<ToastState>(null)
  useEffect(() => {
    if (!toast) return
    const id = setTimeout(() => setToast(null), 2500)
    return () => clearTimeout(id)
  }, [toast])
  return [toast, (copied, ok) => setToast(copied ? { key: ok, ok: true } : { key: 'meetings.copyFailed', ok: false })]
}

function Toast({ toast }: { toast: ToastState }) {
  const { t } = useI18n()
  if (!toast) return null
  return (
    <div className="toast light" role={toast.ok ? 'status' : 'alert'}>
      <Icon name={toast.ok ? 'check_circle' : 'info'} />
      {t(toast.key)}
    </div>
  )
}

export function MeetingsPage() {
  const { t, locale } = useI18n()
  useHostOnly('/login')
  const { data, failed, reload, loadMore, loadingMore } = useMeetings()
  const [tab, setTab] = useState<'upcoming' | 'past'>('upcoming')
  const [editing, setEditing] = useState<Meeting | 'new' | null>(null)
  const [done, setDone] = useState<Meeting | null>(null)
  const [toast, showCopied] = useToast()
  const [error, setError] = useState<MessageKey | null>(null)
  const [starting, setStarting] = useState(false)

  async function startNow() {
    setStarting(true)
    const r = await createRoom(true)
    setStarting(false)
    if (r.kind === 'ok') navigate(`/${r.code}`)
    else if (r.kind === 'plan') navigate('/checkout')
    else setError('home.createFailed')
  }

  async function copyLink(m: Meeting) {
    showCopied(await copy(meetingUrl(m)), 'room.linkCopied')
  }

  /** `past`: 지난 회의 탭에서 지우면 '기록 삭제' 문구로 확인한다. */
  async function remove(m: Meeting, past: boolean) {
    // 이미 링크를 받은 손님이 있을 수 있으니 한 번 확인한다.
    const title = m.title || t('meetings.untitled')
    if (!window.confirm(t(past ? 'meetings.deletePastConfirm' : 'meetings.deleteConfirm', { title }))) return
    const r = await api.del(`/api/meetings/${m.id}`)
    if (r.ok) reload()
    else setError('account.failed')
  }

  const list = data ? data[tab] : []
  return (
    <div className="shell">
      <Header />
      <main className="page">
        <div className="page-head">
          <h1>{t('meetings.title')}</h1>
          <div className="row">
            <button className="btn btn-outline" onClick={() => setEditing('new')}>
              <Icon name="calendar_month" />
              {t('meetings.schedule')}
            </button>
            <button className="btn btn-primary" onClick={() => void startNow()} disabled={starting}>
              <Icon name="video_call" />
              {t('home.startNow')}
            </button>
          </div>
        </div>
        {error && (
          <p className="form-error" role="alert">
            {t(error)}
          </p>
        )}
        <div className="segmented tabs" role="tablist" aria-label={t('meetings.title')}>
          <button role="tab" aria-selected={tab === 'upcoming'} className={tab === 'upcoming' ? 'on' : ''} onClick={() => setTab('upcoming')}>
            {t('meetings.upcoming')} {data ? data.upcoming.length : ''}
          </button>
          <button role="tab" aria-selected={tab === 'past'} className={tab === 'past' ? 'on' : ''} onClick={() => setTab('past')}>
            {t('meetings.past')}
          </button>
        </div>
        <div className="table-card" role="tabpanel" aria-busy={!data && !failed}>
          {failed ? (
            <div className="card-empty" role="alert">
              <p>{t('home.meetingsFailed')}</p>
              <button className="btn btn-sm btn-outline" onClick={reload}>
                {t('room.retry')}
              </button>
            </div>
          ) : (
            !data && <p className="card-empty loading">{t('meetings.loading')}</p>
          )}
          {data && list.length === 0 && <p className="card-empty">{t(tab === 'upcoming' ? 'meetings.noUpcoming' : 'meetings.noPast')}</p>}
          {list.length > 0 && (
            <table className="meetings-table">
              <thead>
                <tr>
                  <th>{t('meetings.date')}</th>
                  <th>{t('meetings.meeting')}</th>
                  <th className="hide-sm">{tab === 'upcoming' ? t('meetings.languages') : t('meetings.length')}</th>
                  <th className="hide-sm">{tab === 'upcoming' ? t('meetings.invited') : t('meetings.people')}</th>
                  {tab === 'past' && <th className="hide-sm">{t('meetings.translated')}</th>}
                  <th aria-label={t('meetings.actions')} />
                </tr>
              </thead>
              <tbody>
                {list.map((m) => {
                  const at = (tab === 'upcoming' ? m.startsAt : (m.startedAt ?? m.startsAt)) ?? m.createdAt
                  return (
                    <tr key={m.id}>
                      <td>
                        <b>{formatDate(at, locale)}</b>
                        <small>{new Date(at * 1000).toLocaleDateString(locale, { weekday: 'short' })} {formatTime(at, locale)}</small>
                      </td>
                      <td>
                        <b className="meeting-title">{m.title || t('meetings.untitled')}</b>
                        {m.kind !== 'meeting' && <span className="badge-soft">{t(m.kind === 'live' ? 'newMeeting.live' : 'newMeeting.inPerson')}</span>}
                      </td>
                      {tab === 'upcoming' ? (
                        <>
                          <td className="hide-sm">{m.langA && m.langB ? `${languageName(m.langA)} ⇄ ${languageName(m.langB)}` : '—'}</td>
                          <td className="hide-sm">{m.guests.length ? t('meetings.peopleCount', { n: String(m.guests.length) }) : t('meetings.none')}</td>
                        </>
                      ) : (
                        <>
                          <td className="hide-sm num">{formatDuration(m.lengthSec, t)}</td>
                          <td className="hide-sm num">{m.peak ? t('meetings.peopleCount', { n: String(m.peak) }) : '—'}</td>
                          <td className="hide-sm num">{formatDuration(m.translatedSec, t)}</td>
                        </>
                      )}
                      <td className="actions">
                        {tab === 'upcoming' && (
                          <>
                            {/* 자막 화면·대면 대화는 주최자 기기에서만 열려 손님이 들어올 수 없으므로 초대할 링크가 없다. */}
                            {m.kind === 'meeting' && (
                              <button className="btn btn-sm btn-outline hide-sm" onClick={() => void copyLink(m)}>
                                {t('meetings.copyLink')}
                              </button>
                            )}
                            <RowMenu
                              items={[
                                [t('meetings.open'), () => navigate(meetingPath(m))],
                                // 좁은 화면에서는 줄의 링크 복사 버튼을 숨기므로 메뉴에서도 할 수 있게 한다.
                                ...(m.kind === 'meeting' ? ([[t('meetings.copyLink'), () => void copyLink(m)]] as MenuItem[]) : []),
                                [t('meetings.edit'), () => setEditing(m)],
                                ...(m.kind === 'meeting' ? ([[t('meetings.inviteInfo'), () => setDone(m)]] as MenuItem[]) : []),
                                [t('meetings.addToCalendar'), () => (location.href = `/api/meetings/${m.id}/ics`)],
                                [t('meetings.delete'), () => void remove(m, false), true],
                              ]}
                              label={t('meetings.actions')}
                            />
                          </>
                        )}
                        {/* 지난 회의 기록(이름·초대한 사람 주소 등)도 지울 수 있게(개인정보 처리방침과 같게). */}
                        {tab === 'past' && <RowMenu items={[[t('meetings.delete'), () => void remove(m, true), true]]} label={t('meetings.actions')} />}
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          )}
          {tab === 'past' && data?.pastMore && (
            <div className="table-more">
              <button
                className="btn btn-sm btn-outline"
                disabled={loadingMore}
                onClick={() => {
                  setError(null)
                  void loadMore().then((ok) => ok || setError('home.meetingsFailed'))
                }}
              >
                {t('meetings.loadMore')}
              </button>
            </div>
          )}
        </div>
        <p className="page-foot">{t('meetings.privacy')}</p>
      </main>
      {editing && (
        <ScheduleDialog
          meeting={editing === 'new' ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={(m) => {
            setEditing(null)
            reload()
            setDone(m)
          }}
        />
      )}
      {done && <ScheduledDialog meeting={done} onClose={() => setDone(null)} />}
      <Toast toast={toast} />
    </div>
  )
}

/** [이름, 할 일, 위험한 일인지] */
type MenuItem = [string, () => void, boolean?]

/** "…" 메뉴 */
function RowMenu({ items, label }: { items: MenuItem[]; label: string }) {
  const [open, setOpen] = useState(false)
  const [up, setUp] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  const buttonRef = useRef<HTMLButtonElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)
  // 화면 아래쪽 줄이면 위로 펼친다(아래로 펼치면 화면 밖으로 나간다).
  useLayoutEffect(() => {
    if (!open || !buttonRef.current || !menuRef.current) return
    const rect = buttonRef.current.getBoundingClientRect()
    const below = innerHeight - rect.bottom
    setUp(below < menuRef.current.offsetHeight + 16 && rect.top > below)
  }, [open])
  useEffect(() => {
    if (!open) return
    const close = (e: PointerEvent) => !ref.current?.contains(e.target as Node) && setOpen(false)
    const esc = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false)
    document.addEventListener('pointerdown', close)
    document.addEventListener('keydown', esc)
    return () => {
      document.removeEventListener('pointerdown', close)
      document.removeEventListener('keydown', esc)
    }
  }, [open])
  return (
    <div className="row-menu" ref={ref}>
      <button ref={buttonRef} className="icon-btn" aria-label={label} aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <Icon name="more_horiz" />
      </button>
      {open && (
        <div ref={menuRef} className={`account-menu${up ? ' up' : ''}`}>
          {items.map(([text, fn, danger]) => (
            <button
              key={text}
              className={`account-item${danger ? ' danger' : ''}`}
              onClick={() => {
                // 누른 항목은 메뉴와 함께 사라지므로 포커스를 "…" 버튼에 먼저 옮긴다(열린 대화상자가 닫히면 여기로 돌아온다).
                buttonRef.current?.focus()
                setOpen(false)
                fn()
              }}
            >
              {text}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}

function Dialog({ title, onClose, children, labelledBy = 'dialog-title' }: { title?: string; onClose: () => void; children: React.ReactNode; labelledBy?: string }) {
  const ref = useRef<HTMLDialogElement>(null)
  useModalDialog(ref)
  // Escape는 cancel 다음 close를 부르므로 close 하나에서만 닫는다.
  return (
    <dialog ref={ref} className="sheet" aria-labelledby={labelledBy} onClose={onClose}>
      {title && <h2 id={labelledBy}>{title}</h2>}
      {children}
    </dialog>
  )
}

const ZONES = ['Asia/Seoul', 'Asia/Tokyo', 'America/New_York', 'Europe/London'] as const

function toLocalInputs(sec: number): { date: string; time: string } {
  const d = new Date(sec * 1000)
  const pad = (n: number) => String(n).padStart(2, '0')
  return { date: `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`, time: `${pad(d.getHours())}:${pad(d.getMinutes())}` }
}

function ScheduleDialog({ meeting, onClose, onSaved }: { meeting: Meeting | null; onClose: () => void; onSaved: (m: Meeting) => void }) {
  const { t, locale } = useI18n()
  const prefs = loadPrefs()
  // 기본값: 다음 정시 + 1시간
  const [initial] = useState(() => meeting?.startsAt ?? Math.ceil(Date.now() / 3_600_000) * 3600 + 3600)
  const [title, setTitle] = useState(meeting?.title ?? '')
  const [date, setDate] = useState(() => toLocalInputs(initial).date)
  const [time, setTime] = useState(() => toLocalInputs(initial).time)
  const [duration, setDuration] = useState(String(meeting?.durationMin ?? 60))
  const [kind, setKind] = useState<Meeting['kind']>(meeting?.kind ?? 'meeting')
  const [langA, setLangA] = useState(meeting?.langA || prefs.lang)
  const [langB, setLangB] = useState(meeting?.langB || prefs.partnerLang)
  const [guests, setGuests] = useState((meeting?.guests ?? []).join(', '))
  // 고칠 때는 기본으로 다시 보내지 않는다(제목만 고쳐도 손님 모두에게 메일이 가지 않게).
  const [sendMail, setSendMail] = useState(!meeting)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<MessageKey | null>(null)

  // 고칠 때 날짜·시각을 건드리지 않았으면 원래 값을 그대로 보낸다(서머타임 겹치는 시간에도 같은 순간이 되게).
  const original = toLocalInputs(initial)
  const startsAt =
    meeting?.startsAt != null && date === original.date && time === original.time
      ? meeting.startsAt
      : Math.floor(new Date(`${date}T${time}`).getTime() / 1000)
  const valid = Number.isFinite(startsAt)
  const myZone = Intl.DateTimeFormat().resolvedOptions().timeZone
  const joinable = kind === 'meeting'

  async function save(e: FormEvent) {
    e.preventDefault()
    if (!title.trim()) return setError('meetings.needTitle')
    if (!valid) return setError('meetings.badTime')
    setBusy(true)
    setError(null)
    // 자막 화면·대면 대화는 주최자 기기에서만 열리므로 손님을 받지 않는다(서버도 거절한다).
    const list = joinable ? guests.split(/[,\s;]+/).map((g) => g.trim()).filter(Boolean) : []
    const body = { title: title.trim(), kind, startsAt, durationMin: Number(duration), langA, langB, guests: list, invite: sendMail && list.length > 0 }
    const r = meeting ? await api.patch<Meeting>(`/api/meetings/${meeting.id}`, body) : await api.post<Meeting>('/api/meetings', body)
    setBusy(false)
    if (r.ok) {
      // 저장하면서 초대 글을 바로 클립보드에(바로 붙여 넣을 수 있게).
      if (joinable) void copy(inviteText(r.data, t, locale))
      return onSaved(r.data)
    }
    if (r.status === 402) return navigate('/checkout')
    setError(r.code === 'starts_at' ? 'meetings.badTime' : r.code === 'guests' ? 'meetings.badGuests' : r.code === 'title' ? 'meetings.needTitle' : 'account.failed')
  }

  return (
    <Dialog title={t(meeting ? 'meetings.editTitle' : 'meetings.schedule')} onClose={onClose}>
      <form className="sheet-form" onSubmit={save}>
        <label className="field-label">
          {t('meetings.name')}
          <input className="input" value={title} maxLength={80} onChange={(e) => setTitle(e.target.value)} autoFocus placeholder={t('meetings.namePlaceholder')} />
        </label>
        <div className="two">
          <label className="field-label">
            {t('meetings.date')}
            <input className="input" type="date" value={date} onChange={(e) => setDate(e.target.value)} required />
          </label>
          <label className="field-label">
            {t('meetings.time')}
            <input className="input" type="time" value={time} step={300} onChange={(e) => setTime(e.target.value)} required />
          </label>
        </div>
        {valid && (
          <p className="form-note">
            {ZONES.filter((z) => z !== myZone)
              .slice(0, 3)
              .map((z) => `${t(`zone.${z.split('/')[1]}` as MessageKey)} ${timeIn(startsAt, locale, z)}`)
              .join(' · ')}
          </p>
        )}
        {/* label로 감싸면 항목을 누른 클릭이 첫 버튼(목록 열기)으로 전달돼 목록이 다시 열린다. */}
        <div className="field-label">
          {t('meetings.duration')}
          <Select
            ariaLabel={t('meetings.duration')}
            value={duration}
            options={['30', '45', '60', '90', '120'].map((v) => ({ value: v, label: formatDuration(Number(v) * 60, t) }))}
            onChange={setDuration}
          />
        </div>
        <div className="field-label">
          {t('meetings.kind')}
          <div className="segmented" role="radiogroup" aria-label={t('meetings.kind')}>
            {(
              [
                ['meeting', 'meetings.kindMeeting'],
                ['live', 'newMeeting.live'],
                ['inPerson', 'newMeeting.inPerson'],
              ] as const
            ).map(([v, key]) => (
              <button key={v} type="button" role="radio" aria-checked={kind === v} className={kind === v ? 'on' : ''} onClick={() => setKind(v)}>
                {t(key)}
              </button>
            ))}
          </div>
        </div>
        <div className="two">
          <Select label={t('lang.mine')} ariaLabel={t('lang.mine')} value={langA} options={LANG_OPTIONS} onChange={setLangA} />
          <Select label={t('lang.partner')} ariaLabel={t('lang.partner')} value={langB} options={LANG_OPTIONS} onChange={setLangB} />
        </div>
        {joinable ? (
          <>
            <label className="field-label">
              {t('meetings.guests')}
              <input className="input" value={guests} onChange={(e) => setGuests(e.target.value)} placeholder={t('meetings.guestsPlaceholder')} autoComplete="off" />
            </label>
            <label className="check-row">
              <input type="checkbox" checked={sendMail} onChange={(e) => setSendMail(e.target.checked)} />
              {t('meetings.sendMail')}
            </label>
          </>
        ) : (
          <p className="form-note">{t('meetings.noGuestsForKind')}</p>
        )}
        {error && (
          <p className="form-error" role="alert">
            {t(error)}
          </p>
        )}
        <div className="sheet-actions">
          <button type="button" className="btn btn-outline" onClick={onClose}>
            {t('room.cancel')}
          </button>
          <button className="btn btn-primary" disabled={busy}>
            {t(meeting ? 'account.save' : joinable ? 'meetings.scheduleAndCopy' : 'meetings.schedule')}
          </button>
        </div>
      </form>
    </Dialog>
  )
}

function ScheduledDialog({ meeting, onClose }: { meeting: Meeting; onClose: () => void }) {
  const { t, locale } = useI18n()
  const [toast, showCopied] = useToast()
  const start = meeting.startsAt ?? 0
  const text = inviteText(meeting, t, locale)
  const myZone = Intl.DateTimeFormat().resolvedOptions().timeZone
  const mail = `mailto:${encodeURIComponent(meeting.guests.join(','))}?subject=${encodeURIComponent(meeting.title)}&body=${encodeURIComponent(text)}`
  const joinable = meeting.kind === 'meeting'
  // 초대 메일을 보내라고 했으면 sent가 온다. 받는 사람별 하루 한도에 걸린 주소는 빠지므로 알려 준다.
  const sent = meeting.sent
  const missed = sent ? meeting.guests.filter((g) => !sent.includes(g)) : []
  return (
    <Dialog onClose={onClose} labelledBy="done-title">
      <div className="done-head">
        <span className="done-check" aria-hidden="true">
          <Icon name="check" />
        </span>
        <h2 id="done-title">{t('meetings.scheduled')}</h2>
      </div>
      <p className="lead left">{t(joinable ? 'meetings.scheduledLead' : 'meetings.scheduledHostLead')}</p>
      <div className="invite-card">
        <b>{meeting.title}</b>
        <p>
          {formatDate(start, locale, true)} {formatTime(start, locale)} – {formatTime(start + meeting.durationMin * 60, locale)} ({zoneName(start, locale)})
          <small>
            {ZONES.filter((z) => z !== myZone)
              .slice(0, 2)
              .map((z) => `${t(`zone.${z.split('/')[1]}` as MessageKey)} ${timeIn(start, locale, z)}`)
              .join(' · ')}
          </small>
        </p>
        {joinable && (
          <p>
            <small>{t('meetings.joinLink')}</small>
            <a href={meetingUrl(meeting)}>{meetingUrl(meeting).replace(/^https?:\/\//, '')}</a>
          </p>
        )}
        {meeting.langA && meeting.langB && (
          <small>
            {languageName(meeting.langA)} ⇄ {languageName(meeting.langB)} · {t('meetings.browserNote')}
          </small>
        )}
      </div>
      {joinable ? (
        <>
          <button className="btn btn-primary btn-lg wide" onClick={async () => showCopied(await copy(text), 'meetings.copied')}>
            <Icon name="content_copy" />
            {t('meetings.copyInvite')}
          </button>
          <div className="three">
            <button className="btn btn-outline" onClick={async () => showCopied(await copy(meetingUrl(meeting)), 'room.linkCopied')}>
              {t('meetings.copyLinkOnly')}
            </button>
            <a className="btn btn-outline" href={`/api/meetings/${meeting.id}/ics`}>
              {t('meetings.addToCalendar')}
            </a>
            <a className="btn btn-outline" href={mail}>
              {t('meetings.sendByMail')}
            </a>
          </div>
        </>
      ) : (
        <a className="btn btn-outline btn-lg wide" href={`/api/meetings/${meeting.id}/ics`}>
          {t('meetings.addToCalendar')}
        </a>
      )}
      {sent && sent.length > 0 && <p className="form-note">{t('meetings.mailedTo', { list: sent.join(', ') })}</p>}
      {missed.length > 0 && (
        <p className="form-error" role="alert">
          {t('meetings.notSent', { list: missed.join(', ') })}
        </p>
      )}
      <div className="sheet-actions">
        <button className="btn btn-quiet" onClick={onClose}>
          {t('meetings.done')}
        </button>
      </div>
      <Toast toast={toast} />
    </Dialog>
  )
}

export { ScheduleDialog, ScheduledDialog }
