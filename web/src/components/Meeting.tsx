import { useEffect, useState } from 'react'
import type { MediaState } from '../protocol/MediaState'
import { fetchRoom, type RoomInfo } from '../lib/api'
import { useAuth } from '../lib/auth'
import { downloadText, transcriptFileName } from '../lib/transcript'
import { useI18n } from '../lib/i18n'
import { acquireLocalMedia, type LocalMedia } from '../lib/media'
import { loadPrefs, savePrefs, type Prefs } from '../lib/prefs'
import { navigate } from '../lib/router'
import { MeetingGonePage, MicBlockedPage } from './ErrorPages'
import { Lobby } from './Lobby'
import { Icon } from './Icon'
import { Room, type LeaveSummary } from './Room'
import { StatusPage } from './StatusPage'

type Stage = 'lobby' | 'room' | 'left'

export function Meeting({ code, onRejoin }: { code: string; onRejoin: () => void }) {
  const [local, setLocal] = useState<LocalMedia | null>(null)
  const [prefs, setPrefs] = useState<Prefs>(loadPrefs)
  const [media, setMedia] = useState<MediaState>({ mic: true, cam: true, screen: false })
  const [stage, setStage] = useState<Stage>('lobby')
  // 나간 뒤에도 자막을 저장할 수 있게 회의실에서 넘겨받는다(메모리에만, 다시 참여하면 사라진다).
  const [summary, setSummary] = useState<(LeaveSummary & { leftAt: number }) | null>(null)
  // 마이크 권한이 막힌 채로 듣기만 하고 들어가기로 했는지
  const [listenOnly, setListenOnly] = useState(false)
  // undefined: 확인 중, null: 없는 회의
  const [room, setRoom] = useState<RoomInfo | null | undefined>(undefined)

  // 회의 정보는 카메라 준비와 병렬로 가져온다(로비 표시 시간을 늘리지 않음).
  useEffect(() => {
    let cancelled = false
    fetchRoom(code)
      .then((r) => !cancelled && setRoom(r))
      // 일시적 오류면 입장 시도에서 다시 판정되므로 번역 켜짐으로 가정하고 진행한다.
      .catch(() => !cancelled && setRoom({ translation: true, title: '', host: '' }))
    return () => {
      cancelled = true
    }
  }, [code])

  // 입장에 끝내 실패하면(거절 등) 장치를 놓는다. "다시 시도"는 Meeting을 새로 마운트해 다시 잡는다.
  const [gaveUp, setGaveUp] = useState(false)
  // 대면 대화·자막 화면은 주최자 기기에서만 돈다(로비가 안내한다). 그런 회의로 알려지면 카메라·마이크를 잡지 않고,
  // 이미 잡은 것은 아래 정리에서 놓는다. 종류를 모르는 동안(일반 회의 포함)은 회의 정보를 기다리지 않고 병렬로 잡는다.
  const hostOnly = !!room?.kind && room.kind !== 'meeting'
  const captureNeeded = room !== null && !hostOnly && stage !== 'left' && !gaveUp
  useEffect(() => {
    if (!captureNeeded) return
    let cancelled = false
    let acquired: MediaStream | null = null
    acquireLocalMedia().then((result) => {
      if (cancelled) {
        result.stream.getTracks().forEach((t) => t.stop())
        return
      }
      acquired = result.stream
      setLocal(result)
      // 잡지 못한 장치는 끈 상태로 둔다(카메라 없는 게스트·듣기만 하는 게스트가 켜진 것으로 알려지지 않게).
      const hasMic = result.stream.getAudioTracks().length > 0
      const hasCam = result.stream.getVideoTracks().length > 0
      setMedia((m) => ({ ...m, mic: m.mic && hasMic, cam: m.cam && hasCam }))
    })
    return () => {
      cancelled = true
      acquired?.getTracks().forEach((t) => t.stop())
    }
  }, [captureNeeded])

  // 트랙의 enabled는 미디어 상태를 따라간다(로비에서 끈 채로 입장하는 경우 포함).
  useEffect(() => {
    local?.stream.getAudioTracks().forEach((t) => (t.enabled = media.mic))
    local?.stream.getVideoTracks().forEach((t) => (t.enabled = media.cam))
  }, [local, media.mic, media.cam])

  function updatePrefs(next: Prefs) {
    setPrefs(next)
    savePrefs(next)
  }

  function leave(next: LeaveSummary) {
    local?.stream.getTracks().forEach((t) => t.stop())
    setSummary({ ...next, leftAt: Date.now() })
    setStage('left')
  }

  if (room === null) return <MeetingGonePage />

  if (stage === 'left') {
    return <EndedPage code={code} summary={summary} onRejoin={onRejoin} />
  }

  const micBlocked = local?.error?.device === 'both' && local.error.kind === 'denied'
  if (stage === 'lobby' && micBlocked && !listenOnly && !hostOnly) {
    return <MicBlockedPage onRetry={onRejoin} onListenOnly={() => setListenOnly(true)} />
  }

  if (stage === 'lobby' || !local) {
    return (
      <Lobby
        code={code}
        local={local}
        room={room}
        prefs={prefs}
        media={media}
        onPrefsChange={updatePrefs}
        onMediaChange={setMedia}
        onJoin={() => setStage('room')}
      />
    )
  }

  return (
    <Room
      code={code}
      local={local.stream}
      prefs={prefs}
      media={media}
      onPrefsChange={updatePrefs}
      onMediaChange={setMedia}
      onLeave={leave}
      onRejoin={onRejoin}
      onGiveUp={() => setGaveUp(true)}
    />
  )
}

export function Notice({ title, body, onHome }: { title: string; body: string; onHome: () => void }) {
  const { t } = useI18n()
  return (
    <StatusPage
      title={title}
      body={body}
      actions={
        <button className="btn btn-primary" onClick={onHome}>
          {t('room.home')}
        </button>
      }
    />
  )
}

/** 나간 화면: 회의 시간·번역한 문장 수, 대화 기록 저장, 다시 참여·홈으로. */
function EndedPage({
  code,
  summary,
  onRejoin,
}: {
  code: string
  summary: (LeaveSummary & { leftAt: number }) | null
  onRejoin: () => void
}) {
  const { t } = useI18n()
  const auth = useAuth()
  // 계정 설정에서 "저장 안내"를 끈 주최자에게는 큰 카드 대신 작은 버튼만 보인다.
  const prompt = auth.status !== 'host' || auth.user.prefs.savePrompt !== false
  const exportTranscript = summary?.exportTranscript ?? null
  const save = () => exportTranscript && downloadText(transcriptFileName(code, new Date()), exportTranscript())
  const minutes = summary?.joinedAt != null ? Math.max(1, Math.round((summary.leftAt - summary.joinedAt) / 60_000)) : null
  const duration =
    minutes == null
      ? null
      : minutes < 60
        ? t('ended.minutes', { n: String(minutes) })
        : t('ended.hoursMinutes', { h: String(Math.floor(minutes / 60)), m: String(minutes % 60) })
  return (
    <StatusPage
      eyebrow={t('lobby.meeting', { code })}
      title={t('room.left')}
      actions={
        <>
          {exportTranscript && !prompt && (
            <button className="btn btn-outline" onClick={save}>
              <Icon name="download" />
              {t('ended.save')}
            </button>
          )}
          <button className="btn btn-outline" onClick={onRejoin}>
            {t('room.rejoin')}
          </button>
          <button className="btn btn-primary" onClick={() => navigate('/')}>
            {t('room.home')}
          </button>
        </>
      }
    >
      {duration && (
        <dl className="ended-stats">
          <div className="status-card">
            <dt>{t('ended.duration')}</dt>
            <dd>{duration}</dd>
          </div>
          <div className="status-card">
            <dt>{t('ended.sentences')}</dt>
            <dd>{summary?.sentences ?? 0}</dd>
          </div>
        </dl>
      )}
      {exportTranscript && prompt && (
        <div className="status-card ended-save">
          <span className="ended-save-icon" aria-hidden="true">
            <Icon name="download" />
          </span>
          <div>
            <h2>{t('ended.saveTitle')}</h2>
            <p>{t('ended.saveBody')}</p>
          </div>
          <button className="btn btn-ink" onClick={save}>
            {t('ended.save')}
          </button>
        </div>
      )}
    </StatusPage>
  )
}
