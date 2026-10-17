import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { fetchRoom } from '../lib/api'
import { useAuth } from '../lib/auth'
import { applyCaption, type Caption, type CaptionMsg } from '../lib/captions'
import { LOCALES, translate, useI18n, type MessageKey } from '../lib/i18n'
import { LANGUAGES, languageName } from '../lib/languages'
import { loadPrefs, savePrefs } from '../lib/prefs'
import { navigate } from '../lib/router'
import type { SttStatus } from '../lib/stt'
import { TalkSide, type TalkFailure } from '../lib/talk'
import { downloadText, formatTranscript, Transcript, transcriptFileName } from '../lib/transcript'
import { Icon } from './Icon'
import { Notice } from './Meeting'
import { LocaleSelect } from './LocaleSelect'
import { QuotaModal } from './QuotaModal'
import { Select } from './Select'
import { StatusPage } from './StatusPage'

/**
 * 대면 모드: 한 기기를 사이에 두고 두 사람이 서로 다른 언어로 대화한다.
 * 아래쪽(near)은 기기 주인, 위쪽(far)은 맞은편 사람이라 180도 돌려 보여 준다.
 * 말할 사람이 자기 쪽 버튼을 누르면 마이크 입력이 그쪽 음성 인식으로 간다.
 */
type Side = 'near' | 'far'

/** 아무도 말하지 않은 채 이만큼 지나면 미리 열어 둔 음성 인식 세션을 닫는다. */
const WARM_IDLE_MS = 10 * 60_000

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

export function InPerson({ code }: { code: string }) {
  const { t } = useI18n()
  const auth = useAuth()
  if (auth.status === 'loading') return null
  if (auth.status !== 'host') {
    return <Notice title={t('talk.title')} body={t('talk.hostOnly')} onHome={() => navigate('/')} />
  }
  return <Talk code={code} />
}

function Talk({ code }: { code: string }) {
  const { t, locale } = useI18n()
  const [langs, setLangs] = useState(() => {
    const p = loadPrefs()
    return { near: p.lang, far: p.partnerLang }
  })
  const [track, setTrack] = useState<MediaStreamTrack | null>(null)
  const [micError, setMicError] = useState(false)
  const [active, setActive] = useState<Side | null>(null)
  const [ids, setIds] = useState<Record<Side, string | null>>({ near: null, far: null })
  const [captions, setCaptions] = useState<Record<Side, Caption[]>>({ near: [], far: [] })
  const [stt, setStt] = useState<Record<Side, SttStatus | null>>({ near: null, far: null })
  const [failure, setFailure] = useState<MessageKey | null>(null)
  // 번역 시간이 떨어지면 음성 인식이 멈춘다(TalkSide). 시간을 추가하면 서버 알림으로 다시 켜진다.
  const [outOfTime, setOutOfTime] = useState(false)
  const [quotaClosed, setQuotaClosed] = useState(false)
  // 실패하면 두 연결과 마이크를 모두 정리한다(실패 화면 뒤에서 계속 듣거나 회의에 남지 않게).
  const live = failure === null
  // 마이크 권한을 다시 요청할 때 올린다.
  const [micAttempt, setMicAttempt] = useState(0)
  const [rotated, setRotated] = useState(true)
  const [transcript] = useState(() => new Transcript())
  const [hasTranscript, setHasTranscript] = useState(false)
  const sidesRef = useRef<Record<Side, TalkSide | null>>({ near: null, far: null })
  const langsRef = useRef(langs)
  /** 직접 언어를 고쳤는지(그 뒤에 온 예약 언어로 되돌리지 않게) */
  const touchedRef = useRef(false)

  // 마이크만 쓴다(카메라 없음). 회의와 같은 처리(에코 제거·잡음 억제·자동 음량).
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

  // 두 사람 = 같은 방의 두 연결. 마이크가 준비되면 연다(음성 인식 세션을 미리 열어 두기 위해).
  useEffect(() => {
    if (!track || !live) return
    const open = (side: Side, other: Side) =>
      new TalkSide(code, langsRef.current[side], langsRef.current[other], track, {
        onJoined: (id) => setIds((m) => ({ ...m, [side]: id })),
        onCaption: (msg: CaptionMsg) => {
          const now = Date.now()
          setCaptions((m) => ({ ...m, [side]: applyCaption(m[side], msg, now) }))
          // 기록은 아래쪽 사람 기준 한 벌만 모은다(양쪽 발화가 모두 들어 있다).
          if (side === 'near') {
            transcript.add(msg, now)
            setHasTranscript(transcript.size > 0)
          }
        },
        onSttStatus: (s) => setStt((m) => ({ ...m, [side]: s })),
        onQuota: (out) => {
          setOutOfTime(out)
          if (!out) setQuotaClosed(false)
        },
        onFailed: (reason) => setFailure(FAILURES[reason] ?? 'room.cannotJoin'),
      })
    const sides = { near: open('near', 'far'), far: open('far', 'near') }
    sidesRef.current = sides
    return () => {
      sides.near.close()
      sides.far.close()
      sidesRef.current = { near: null, far: null }
    }
  }, [code, track, transcript, live])

  useEffect(() => {
    const sides = sidesRef.current
    sides.near?.setActive(active === 'near')
    sides.far?.setActive(active === 'far')
    // 누군가 누르면 두 세션을 다시 열어 두고, 아무도 안 누른 채 오래 지나면 닫는다(다음에 누를 때 약 0.7초 더 걸림).
    if (active) {
      sides.near?.setWarm(true)
      sides.far?.setWarm(true)
      return
    }
    const id = setTimeout(() => {
      sides.near?.setWarm(false)
      sides.far?.setWarm(false)
    }, WARM_IDLE_MS)
    return () => clearTimeout(id)
  }, [active, ids])

  // 예약할 때 정한 언어 쌍(내 언어 → 아래쪽, 상대 언어 → 위쪽)으로 시작한다. 회의 정보는 마이크 준비와 함께(기다리지 않고) 받는다.
  useEffect(() => {
    let cancelled = false
    fetchRoom(code)
      .then((r) => {
        if (cancelled || touchedRef.current || !r?.langA || !r.langB) return
        const next = { near: r.langA, far: r.langB }
        langsRef.current = next
        setLangs(next)
        sidesRef.current.near?.setLangs(next.near, next.far)
        sidesRef.current.far?.setLangs(next.far, next.near)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [code])

  function changeLang(side: Side, lang: string) {
    touchedRef.current = true
    const other: Side = side === 'near' ? 'far' : 'near'
    const next = { ...langs, [side]: lang }
    langsRef.current = next
    setLangs(next)
    sidesRef.current[side]?.setLangs(next[side], next[other])
    sidesRef.current[other]?.setLangs(next[other], next[side])
    const p = loadPrefs()
    savePrefs({ ...p, lang: next.near, partnerLang: next.far })
  }

  function speakerLabel(id: string) {
    if (id === ids.near) return languageName(langs.near)
    if (id === ids.far) return languageName(langs.far)
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
        title={t('talk.title')}
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

  const ready = !!ids.near && !!ids.far
  const half = (side: Side) => (
    <TalkHalf
      side={side}
      lang={langs[side]}
      selfId={ids[side]}
      captions={captions[side]}
      speaking={active === side}
      ready={ready}
      micError={micError}
      sttStatus={stt[side]}
      outOfTime={outOfTime}
      rotated={side === 'far' && rotated}
      onLang={(l) => changeLang(side, l)}
      onTalk={() => setActive((a) => (a === side ? null : side))}
    />
  )

  return (
    <div className="talk">
      {half('far')}
      <div className="talk-bar">
        <button className="btn btn-text" onClick={() => setRotated((r) => !r)}>
          {t(rotated ? 'talk.sideBySide' : 'talk.faceToFace')}
        </button>
        {micError && (
          <button
            className="btn btn-text"
            onClick={() => {
              setMicError(false)
              setMicAttempt((n) => n + 1)
            }}
          >
            {t('room.retry')}
          </button>
        )}
        <div className="talk-bar-actions">
          <LocaleSelect />
          {hasTranscript && (
            <button className="icon-btn" aria-label={t('captions.download')} title={t('captions.download')} onClick={save}>
              <Icon name="download" />
            </button>
          )}
          <button className="btn btn-outline" onClick={() => navigate('/')}>
            {t('talk.end')}
          </button>
        </div>
      </div>
      {half('near')}
      {outOfTime && !quotaClosed && <QuotaModal onClose={() => setQuotaClosed(true)} />}
    </div>
  )
}

type HalfProps = {
  side: Side
  lang: string
  selfId: string | null
  captions: Caption[]
  speaking: boolean
  ready: boolean
  micError: boolean
  sttStatus: SttStatus | null
  outOfTime: boolean
  rotated: boolean
  onLang: (lang: string) => void
  onTalk: () => void
}

/** 한 사람의 화면. 상대 말은 내 언어 번역을 크게, 내 말은 원문을 크게 보인다. */
function TalkHalf({ side, lang, selfId, captions, speaking, ready, micError, sttStatus, outOfTime, rotated, onLang, onTalk }: HalfProps) {
  const { t: uiT } = useI18n()
  // 맞은편 사람은 화면 언어를 못 읽을 수 있다. 그 사람 언어로 된 문구가 있으면 그것으로 보인다.
  const own = LOCALES.find((l) => l.code === lang)?.code
  const t = own ? (key: MessageKey) => translate(own, key) : uiT
  const ref = useRef<HTMLDivElement>(null)
  const visible = captions.filter((c) => !(c.final && c.src === ''))
  useLayoutEffect(() => {
    const el = ref.current
    if (el) el.scrollTop = el.scrollHeight
  }, [captions])

  return (
    <section className={`talk-half ${side}${rotated ? ' rotated' : ''}`} lang={lang} aria-label={languageName(lang)}>
      <div className="talk-head">
        <Select ariaLabel={t('lang.mine')} value={lang} options={LANG_OPTIONS} onChange={onLang} variant="pill" icon="language" alwaysDown />
        {outOfTime ? (
          <span className="talk-status" role="status">
            {t('quota.title')}
          </span>
        ) : (
          (sttStatus === 'paused' || sttStatus === 'unavailable') && (
            <span className="talk-status" role="status">
              {t(sttStatus === 'paused' ? 'captions.sttPaused' : 'captions.sttUnavailable')}
            </span>
          )
        )}
      </div>
      {/* 화면 낭독은 기기 주인 쪽만(같은 말이 원문·번역으로 두 번 읽히지 않게). */}
      <div className="talk-feed" ref={ref} aria-live={side === 'near' ? 'polite' : undefined}>
        {visible.length === 0 && (
          <p className="feed-empty">{t(micError ? 'talk.noMic' : ready ? 'talk.empty' : 'talk.connecting')}</p>
        )}
        {visible.map((c, i) => {
          const mine = c.speaker === selfId
          const main = mine ? c.src : (c.text ?? (c.final ? c.src : null))
          const sub = mine ? c.text : c.src
          // 최신순 크기: 방금 문장이 가장 크고, 지난 문장일수록 작고 흐려진다.
          const age = Math.min(3, visible.length - 1 - i)
          return (
            <div key={c.seg} className={`talk-line age-${age}${mine ? ' mine' : ''}${c.final ? '' : ' pending'}`} aria-hidden={!c.final}>
              {/* 작은 회색 줄(상대 원문·내 말의 번역)이 위, 읽을 줄이 아래에 크게 */}
              {sub && sub !== main && (
                <p className={`talk-sub${mine ? ' tr-mark' : ''}`} lang={mine ? c.tgt_lang : c.src_lang}>
                  {sub}
                </p>
              )}
              <p className={`talk-main${!mine && (c.text != null || !c.final) ? ' tr-mark' : ''}`} lang={mine ? c.src_lang : c.tgt_lang}>
                {main ?? t('captions.translating')}
              </p>
            </div>
          )
        })}
      </div>
      <button
        className={`talk-button${speaking ? ' on' : ''}`}
        onClick={onTalk}
        disabled={!ready || outOfTime}
      >
        <Icon name="mic" />
        {speaking ? t('talk.listening') : t('talk.tapToSpeak')}
      </button>
    </section>
  )
}
