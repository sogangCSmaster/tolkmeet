import { useEffect, useRef, useState } from 'react'
import { fetchRoom } from '../lib/api'
import { useAuth } from '../lib/auth'
import { applyCaption, type Caption, type CaptionMsg } from '../lib/captions'
import { useI18n, type MessageKey } from '../lib/i18n'
import { canAutoDetect } from '../lib/langdetect'
import { LANGUAGES, languageName } from '../lib/languages'
import { loadPrefs, savePrefs } from '../lib/prefs'
import { navigate } from '../lib/router'
import { useReviewScroll } from '../lib/reviewScroll'
import type { SttStatus } from '../lib/stt'
import { AutoTalk } from '../lib/autotalk'
import type { TalkFailure } from '../lib/talk'
import { downloadText, formatTranscript, Transcript, transcriptFileName } from '../lib/transcript'
import { Icon } from './Icon'
import { Notice } from './Meeting'
import { LocaleSelect } from './LocaleSelect'
import { QuotaModal } from './QuotaModal'
import { Select } from './Select'
import { StatusPage } from './StatusPage'
import { ToLatest } from './ToLatest'

/**
 * 자막 화면: 대면 미팅 탁자 위 태블릿 한 대가 듣고, 누가 어떤 언어로 말하든 원문과 번역을 크게 쌓아 보여 준다.
 * 버튼을 누를 필요가 없다. 언어는 발화마다 글자로 가린다(`lib/langdetect.ts`).
 */
const LANG_OPTIONS = LANGUAGES.map((l) => ({ value: l.code as string, label: l.name }))

const FAILURES: Partial<Record<TalkFailure, MessageKey>> = {
  room_not_found: 'room.notFound',
  room_full: 'room.full',
  forbidden: 'talk.notYours',
  not_talk_room: 'talk.notTalkRoom',
  in_use: 'talk.inUse',
  disconnected: 'room.disconnected',
  server_down: 'room.serverDown',
}

export function LiveCaptions({ code }: { code: string }) {
  const { t } = useI18n()
  const auth = useAuth()
  if (auth.status === 'loading') return null
  if (auth.status !== 'host') {
    return <Notice title={t('live.title')} body={t('live.hostOnly')} onHome={() => navigate('/')} />
  }
  return <Live code={code} />
}

function Live({ code }: { code: string }) {
  const { t, locale } = useI18n()
  const [langs, setLangs] = useState(() => {
    const p = loadPrefs()
    return { a: p.lang, b: p.partnerLang }
  })
  const [track, setTrack] = useState<MediaStreamTrack | null>(null)
  const [micError, setMicError] = useState(false)
  const [micAttempt, setMicAttempt] = useState(0)
  const [ids, setIds] = useState<{ a: string | null; b: string | null }>({ a: null, b: null })
  const [captions, setCaptions] = useState<Caption[]>([])
  const [stt, setStt] = useState<SttStatus | null>(null)
  const [listening, setListening] = useState(true)
  // 언어 설정은 처음에 펼쳐 두어 확인하게 한다. 닫으면 자막만 크게 보인다.
  const [settingsOpen, setSettingsOpen] = useState(true)
  const [failure, setFailure] = useState<MessageKey | null>(null)
  // 번역 시간이 떨어지면 듣지 않는다(AutoTalk). 시간을 추가하면 서버 알림으로 다시 듣는다.
  const [outOfTime, setOutOfTime] = useState(false)
  const [quotaClosed, setQuotaClosed] = useState(false)
  const [transcript] = useState(() => new Transcript())
  const [hasTranscript, setHasTranscript] = useState(false)
  const talkRef = useRef<AutoTalk | null>(null)
  const langsRef = useRef(langs)
  /** 직접 언어를 고쳤는지(그 뒤에 온 예약 언어로 되돌리지 않게) */
  const touchedRef = useRef(false)
  const listeningRef = useRef(listening)
  // 실패하면 연결과 마이크를 모두 정리한다.
  const live = failure === null

  useEffect(() => {
    if (!live) return
    let cancelled = false
    let stream: MediaStream | null = null
    navigator.mediaDevices
      .getUserMedia({ audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true } })
      .then((s) => {
        if (cancelled) return s.getTracks().forEach((tr) => tr.stop())
        stream = s
        setTrack(s.getAudioTracks()[0] ?? null)
      })
      .catch(() => !cancelled && setMicError(true))
    return () => {
      cancelled = true
      stream?.getTracks().forEach((tr) => tr.stop())
      setTrack(null)
    }
  }, [live, micAttempt])

  useEffect(() => {
    if (!track || !live) return
    const talk = new AutoTalk(code, langsRef.current.a, langsRef.current.b, track, {
      onJoined: (side, id) => setIds((m) => ({ ...m, [side]: id })),
      onCaption: (msg: CaptionMsg) => {
        const now = Date.now()
        setCaptions((list) => applyCaption(list, msg, now))
        transcript.add(msg, now)
        setHasTranscript(transcript.size > 0)
      },
      onSttStatus: setStt,
      onQuota: (out) => {
        setOutOfTime(out)
        if (!out) setQuotaClosed(false)
      },
      onFailed: (reason) => setFailure(FAILURES[reason] ?? 'room.cannotJoin'),
    })
    talk.setListening(listeningRef.current)
    talkRef.current = talk
    return () => {
      talk.close()
      talkRef.current = null
    }
  }, [code, track, transcript, live])

  // 같은 문자를 쓰는 두 언어는 가를 수 없으므로 듣지 않는다(엉뚱한 언어로 번역하지 않게).
  const pairOk = canAutoDetect(langs.a, langs.b)
  const hearing = listening && pairOk
  useEffect(() => {
    listeningRef.current = hearing
    talkRef.current?.setListening(hearing)
  }, [hearing, ids])

  // 예약할 때 정한 언어 쌍이 있으면 그것으로 시작한다. 회의 정보는 마이크 준비와 함께(기다리지 않고) 받는다.
  useEffect(() => {
    let cancelled = false
    fetchRoom(code)
      .then((r) => {
        if (cancelled || touchedRef.current || !r?.langA || !r.langB) return
        const next = { a: r.langA, b: r.langB }
        langsRef.current = next
        setLangs(next)
        talkRef.current?.setLangs(next.a, next.b)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [code])

  function changeLang(side: 'a' | 'b', lang: string) {
    touchedRef.current = true
    const next = { ...langs, [side]: lang }
    langsRef.current = next
    setLangs(next)
    talkRef.current?.setLangs(next.a, next.b)
    savePrefs({ ...loadPrefs(), lang: next.a, partnerLang: next.b })
  }

  function toggleListening() {
    setListening((on) => !on)
  }

  function speakerLabel(id: string) {
    if (id === ids.a) return languageName(langs.a)
    if (id === ids.b) return languageName(langs.b)
    return t('captions.participant')
  }

  function save() {
    const text = formatTranscript(transcript.entries(), {
      title: t('transcript.title'),
      code,
      savedAt: new Date(),
      locale,
      speakerName: speakerLabel,
    })
    downloadText(transcriptFileName(code, new Date()), text)
  }

  if (failure) {
    return (
      <StatusPage
        title={t('live.title')}
        body={t(failure)}
        actions={
          <>
            {hasTranscript && (
              <button className="btn btn-outline" onClick={save}>
                <Icon name="download" />
                {t('captions.download')}
              </button>
            )}
            <button className="btn btn-primary" onClick={() => navigate('/')}>
              {t('room.home')}
            </button>
          </>
        }
      />
    )
  }

  const ready = !!ids.a && !!ids.b
  const status: MessageKey = micError
    ? 'talk.noMic'
    : !ready
      ? 'talk.connecting'
      : outOfTime
        ? 'quota.title'
        : !hearing
          ? 'live.paused'
          : stt === 'unavailable'
            ? 'captions.sttUnavailable'
            : stt === 'paused'
              ? 'captions.sttPaused'
              : stt === 'live'
                ? 'live.listening'
                : 'talk.connecting'

  const on = hearing && stt === 'live' && !micError
  return (
    <div className="live">
      <header className="live-bar">
        <p className={`live-status${on ? ' on' : ''}`} role="status">
          <span className={`levels${on ? ' on' : ''}`} aria-hidden="true">
            <i />
            <i />
            <i />
            <i />
          </span>
          {t(status)}
        </p>
        <div className="live-actions">
          <LocaleSelect />
          {micError ? (
            <button
              className="btn btn-outline"
              onClick={() => {
                setMicError(false)
                setMicAttempt((n) => n + 1)
              }}
            >
              {t('room.retry')}
            </button>
          ) : (
            <button
              className="icon-btn"
              onClick={toggleListening}
              disabled={!ready || !pairOk}
              aria-label={t(listening ? 'live.pause' : 'live.resume')}
              title={t(listening ? 'live.pause' : 'live.resume')}
            >
              <Icon name={listening ? 'mic' : 'mic_off'} />
            </button>
          )}
          {hasTranscript && (
            <button className="icon-btn" aria-label={t('captions.download')} title={t('captions.download')} onClick={save}>
              <Icon name="download" />
            </button>
          )}
          <button
            className={`icon-btn${settingsOpen ? ' on' : ''}`}
            aria-label={t('history.settings')}
            title={t('history.settings')}
            aria-expanded={settingsOpen}
            aria-controls="live-settings"
            onClick={() => setSettingsOpen((o) => !o)}
          >
            <Icon name="tune" />
          </button>
          <button className="btn btn-outline" onClick={() => navigate('/')}>
            {t('talk.end')}
          </button>
        </div>
        {settingsOpen && (
          // 언어는 처음에 한 번 고르면 되므로 평소에는 접어 두고 자막만 크게 보인다.
          <div className="live-settings" id="live-settings">
            <Select ariaLabel={t('live.langA')} value={langs.a} options={LANG_OPTIONS} onChange={(l) => changeLang('a', l)} variant="pill" icon="language" />
            <Icon name="sync_alt" />
            <Select ariaLabel={t('live.langB')} value={langs.b} options={LANG_OPTIONS} onChange={(l) => changeLang('b', l)} variant="pill" icon="language" />
          </div>
        )}
      </header>
      {!pairOk && (
        <p className="live-warning" role="alert">
          {t('live.unsupportedPair')}
        </p>
      )}
      <LiveFeed captions={captions} />
      {outOfTime && !quotaClosed && <QuotaModal onClose={() => setQuotaClosed(true)} />}
    </div>
  )
}

/**
 * 쌓여 가는 자막(아래에서 위로). 방금 문장이 가장 크고 진하며 지난 문장일수록 작고 흐려진다.
 * 위로 스크롤해 지난 말을 읽는 동안에는 모두 같은 크기로 바뀌고 "최신 대화로" 버튼이 뜬다.
 */
function LiveFeed({ captions }: { captions: Caption[] }) {
  const { t } = useI18n()
  const visible = captions.filter((c) => !(c.final && c.src === ''))
  const { ref, reviewing, onScroll, toLatest, newCount } = useReviewScroll<HTMLElement>(visible.map((c) => String(c.seg)), captions)

  return (
    <div className="live-feed-wrap">
      <main className={`live-feed${reviewing ? ' reviewing' : ''}`} ref={ref} role="log" aria-live="polite" onScroll={onScroll}>
        <div className="live-feed-inner">
          {visible.length === 0 && <p className="feed-empty">{t('live.empty')}</p>}
          {visible.map((c, i) => {
            const age = Math.min(3, visible.length - 1 - i)
            // 번역이 없는 확정 줄(번역 실패 등)은 원문을 크게 보인다.
            const translated = c.tgt_lang !== c.src_lang && (c.text != null || !c.final)
            return (
              <article key={c.seg} className={`live-entry age-${age}${c.final ? '' : ' pending'}`} aria-hidden={!c.final}>
                {translated && (
                  <p className="live-src" lang={c.src_lang}>
                    {c.src}
                  </p>
                )}
                <p className="live-tgt" lang={translated ? (c.text != null ? c.tgt_lang : undefined) : c.src_lang}>
                  {translated ? (c.text ?? t('captions.translating')) : c.src}
                </p>
              </article>
            )
          })}
        </div>
      </main>
      {reviewing && <ToLatest newCount={newCount} onClick={toLatest} />}
    </div>
  )
}
