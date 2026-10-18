import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { Knock } from '../protocol/Knock'
import type { MediaState } from '../protocol/MediaState'
import type { Peer } from '../protocol/Peer'
import type { RoomSettings } from '../protocol/RoomSettings'
import type { ServerMsg } from '../protocol/ServerMsg'
import type { SignalData as WireSignal } from '../protocol/SignalData'
import { applyCaption, type Caption } from '../lib/captions'
import { useI18n, type Locale, type MessageKey } from '../lib/i18n'
import { reacquireTrack } from '../lib/media'
import { Mesh } from '../lib/mesh'
import type { Prefs } from '../lib/prefs'
import { Signaling } from '../lib/signaling'
import { ScreenShare } from '../lib/screen'
import { pipSupported, usePipWindow } from '../lib/pip'
import { useSpeakingPeers } from '../lib/speaking'
import { startSpeech, type Speech } from '../lib/speech'
import { downloadText, formatTranscript, Transcript, transcriptFileName } from '../lib/transcript'
import type { Grant, SegCounter, SttStatus, TokenError } from '../lib/stt'
import { CaptionPanel } from './CaptionPanel'
import { CaptionPip } from './CaptionPip'
import { MeetingGonePage } from './ErrorPages'
import { KnockPrompts } from './KnockPrompts'
import { QuotaModal } from './QuotaModal'
import { LocaleSelect } from './LocaleSelect'
import { StatusPage } from './StatusPage'
import { Icon } from './Icon'
import { VideoTile } from './VideoTile'

type Props = {
  code: string
  local: MediaStream
  prefs: Prefs
  media: MediaState
  onPrefsChange: (p: Prefs) => void
  onMediaChange: (m: MediaState) => void
  /** 나갈 때 회의 요약을 넘긴다. 자막 텍스트는 나간 화면에서 저장을 누를 때만 만든다. */
  onLeave: (summary: LeaveSummary) => void
  /** 입장이 끝내 안 됐다(거절·없는 방·가득 참 등): 카메라·마이크를 놓아도 된다. "다시 시도"는 처음부터 다시 잡는다. */
  onGiveUp: () => void
  onRejoin: () => void
}

export type LeaveSummary = {
  /** 확정된 자막이 없으면 null */
  exportTranscript: (() => string) | null
  /** 처음 입장한 시각(ms). 입장하지 못하고 나가면 null */
  joinedAt: number | null
  sentences: number
}

type Status =
  | { kind: 'connecting' }
  | { kind: 'waiting' }
  | { kind: 'joined'; selfId: string }
  | { kind: 'error'; message: MessageKey }

/** 다시 연결할 때 WebSocket이 열리기를 기다리는 시간. 넘으면 이 시도를 버리고 다음 시도로 넘어간다. */
/** 휴대폰 브라우저는 화면 공유(getDisplayMedia)를 지원하지 않으므로 버튼을 숨긴다. */
const canShare = typeof navigator !== 'undefined' && typeof navigator.mediaDevices?.getDisplayMedia === 'function'

const RECONNECT_TIMEOUT_MS = 5000
/** 끝난 장치를 다시 켜려다 실패한 뒤, 장치가 다시 연결되면 저절로 다시 잡아 보는 기간 */
const WANT_MS = 30_000
/** 처음 입장할 때는 재시도 없이 오류를 보이므로, 느린 모바일 망의 TLS 연결까지 기다리도록 넉넉히 둔다. */
const FIRST_CONNECT_TIMEOUT_MS = 10_000

// 입장 전 오류는 화면 전체를 바꾸고, 입장 후 오류(예: 가득 찬 방에 수락)는 토스트로만 알린다.
const FATAL_ERRORS: Partial<Record<string, MessageKey>> = {
  room_full: 'room.full',
  room_not_found: 'room.notFound',
  too_many_waiting: 'room.queueFull',
}

export function Room({ code, local, prefs, media, onPrefsChange, onMediaChange, onLeave, onRejoin, onGiveUp }: Props) {
  const { t, locale } = useI18n()
  const [status, setStatus] = useState<Status>({ kind: 'connecting' })
  const [peers, setPeers] = useState<Record<string, Peer>>({})
  const [streams, setStreams] = useState<Record<string, MediaStream>>({})
  const [screenTrack, setScreenTrack] = useState<MediaStreamTrack | null>(null)
  const [screenBusy, setScreenBusy] = useState(false)
  const [panelOpen, setPanelOpen] = useState(true)
  const [disconnected, setDisconnected] = useState(false)
  // 혼자일 때 띄우는 초대 카드를 닫았는지(이 회의 동안).
  const [inviteClosed, setInviteClosed] = useState(false)
  const copyLinkRef = useRef<HTMLButtonElement>(null)
  const [toast, setToast] = useState<{ key: MessageKey; params?: Record<string, string>; ms?: number } | null>(null)
  // 헤드폰 안내는 회의에 한 번만(재연결·번역 토글로 음성 인식이 다시 시작돼도).
  const echoHintedRef = useRef(false)
  const [isHost, setIsHost] = useState(false)
  const [settings, setSettings] = useState<RoomSettings>({ translation: true })
  const [knocks, setKnocks] = useState<Knock[]>([])
  const [captions, setCaptions] = useState<Caption[]>([])
  // 화면 목록(최근 200줄)과 별개로 회의 내내 확정된 자막을 모은다. 다시 연결해도 이어진다.
  const [transcript] = useState(() => new Transcript())
  const transcriptRef = useRef(transcript)
  const [hasTranscript, setHasTranscript] = useState(false)
  // 나간 참가자의 지난 자막에도 이름이 보이도록 이름은 지우지 않고 모은다.
  const [names, setNames] = useState<Record<string, string>>({})
  const [sttStatus, setSttStatus] = useState<SttStatus | null>(null)
  const [outOfTime, setOutOfTime] = useState(false)
  const [quotaClosed, setQuotaClosed] = useState(false)
  // 주최자가 오래 자리를 비워 서버가 음성 인식·번역을 멈췄다(주최자의 시간을 쓰지 않게). 돌아오면 서버가 알린다.
  const [hostAway, setHostAway] = useState(false)
  // 서비스 전체가 오늘 지출 상한에 닿아 서버가 음성 인식·번역을 멈췄다. 다음 UTC 날짜에 서버가 알린다.
  const [spendPaused, setSpendPaused] = useState(false)
  // 화면 공유 중 자막 창(Document PiP). 공유를 시작한 클릭 안에서 바로 열어 보고, 막히면 버튼으로 연다.
  const pip = usePipWindow(`Tolkmeet · ${t('history.title')}`)
  const pipOpenRef = useRef(pip.open)
  const pipResetRef = useRef(pip.resetClosed)
  useEffect(() => {
    pipOpenRef.current = pip.open
    pipResetRef.current = pip.resetClosed
  }, [pip.open, pip.resetClosed])
  // 자막 창을 못 여는 브라우저 안내는 회의에 한 번만.
  const pipHintedRef = useRef(false)
  // 이번 공유에서 자막 창 안내 카드를 닫았는지. 공유를 새로 시작하면 다시 보인다.
  const [pipDismissed, setPipDismissed] = useState(false)
  // 여는 사이 공유가 끝났는지 확인하는 용도(비동기 열기가 끝난 뒤 판단).
  const sharingRef = useRef(false)

  const sigRef = useRef<Signaling | null>(null)
  const meshRef = useRef<Mesh | null>(null)
  const screenRef = useRef<ScreenShare | null>(null)
  const speechRef = useRef<Speech | null>(null)
  // STT 구간 번호는 서버 연결 하나에서 계속 늘어나야 한다(음성 인식이 꺼졌다 켜져도).
  const segRef = useRef<SegCounter>({ next: 0 })
  // 발급은 비동기라 답 순서가 바뀔 수 있어 요청 id로 짝을 맞춘다. 남은 요청은 정리할 때 실패로 끝낸다.
  const tokensRef = useRef(new Map<number, { resolve: (g: Grant) => void; reject: (e: TokenError) => void }>())
  const tokenIdRef = useRef(0)
  const onMediaChangeRef = useRef(onMediaChange)
  useEffect(() => { onMediaChangeRef.current = onMediaChange }, [onMediaChange])
  // 이름은 입장할 때 정한 값으로 고정한다. 언어·미디어 상태는 다시 연결할 때 그때의 값을 보낸다.
  const nameRef = useRef(prefs.name.trim())
  const prefsRef = useRef(prefs)
  useEffect(() => {
    prefsRef.current = prefs
  }, [prefs])
  // 연결이 끊기면 같은 화면에서 다시 연결한다. 한 번 입장했으면 받은 재입장 토큰으로 수락 없이 돌아온다.
  const [attempt, setAttempt] = useState(0)
  const resumeRef = useRef<string | null>(null)
  const everJoinedRef = useRef(false)
  // 나간 화면에 회의 시간을 보이려고 처음 입장한 시각을 둔다(다시 연결해도 유지).
  const joinedAtRef = useRef<number | null>(null)
  const retriesRef = useRef(0)
  // 대기 중인 재시도 타이머. 시도가 진행 중이면 null이다.
  const retryTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  // 방이 없어졌거나 거절된 경우처럼 다시 연결해도 소용없을 때 true. 사용자가 직접 다시 시도한다.
  const stoppedRef = useRef(false)
  // 다시 연결하면 내 참가자 id가 바뀐다. 지난 id로 남은 내 자막도 "나"로 보이게 모은다.
  const selfIdsRef = useRef(new Set<string>())
  // 브라우저의 "공유 중지"처럼 나중에 호출되는 콜백이 최신 미디어 상태를 보도록 한다.
  const mediaRef = useRef(media)
  useEffect(() => {
    mediaRef.current = media
  }, [media])
  // 음성 인식 세션 열쇠: 서버가 토큰을 내 언어 + 상대 언어로 잠그므로 둘 중 하나라도 바뀌면 세션을 새로 연다.
  const sttKey = `${prefs.lang}|${prefs.partnerLang}`
  const langRef = useRef(sttKey)
  useEffect(() => {
    langRef.current = sttKey
  }, [sttKey])

  useEffect(() => {
    let disposed = false

    const onMessage = (msg: ServerMsg) => {
      if (disposed) return
      switch (msg.t) {
        case 'welcome': {
          const mesh = new Mesh(
            msg.you,
            local,
            msg.ice_servers,
            // 브라우저 toJSON() 형식과 같다(DOM 타입만 candidate를 선택 항목으로 둔다).
            (to, data) => sig.send({ t: 'signal', to, data: data as WireSignal }),
            (id, stream) => setStreams((s) => ({ ...s, [id]: stream })),
          )
          meshRef.current = mesh
          const camera = local.getVideoTracks()[0] ?? null
          screenRef.current = new ScreenShare(
            // 공유가 끝나면 카메라로 돌아간다. 그사이 카메라를 다시 잡았으면(장치를 다시 연결) 지금 카메라로.
            { setVideoTrack: (track) => mesh.setVideoTrack(track === camera ? (local.getVideoTracks()[0] ?? null) : track) },
            camera,
            (track) => {
              setScreenTrack(track)
              sharingRef.current = !!track
              if (track) {
                setPipDismissed(false)
                pipResetRef.current()
              }
              if (track && pipSupported) void pipOpenRef.current(() => sharingRef.current)
              else if (track && !pipHintedRef.current) {
                pipHintedRef.current = true
                setToast({ key: 'pip.unsupported', ms: 7000 })
              }
              const next = { ...mediaRef.current, screen: !!track }
              mediaRef.current = next
              onMediaChangeRef.current(next)
              sig.send({ t: 'media', media: announced(local, next) })
            },
            setScreenBusy,
            () => setToast({ key: 'room.screenFailed' }),
          )
          joinedRef.current = true
          everJoinedRef.current = true
          joinedAtRef.current ??= Date.now()
          resumeRef.current = msg.resume
          selfIdsRef.current.add(msg.you)
          retriesRef.current = 0
          setDisconnected(false)
          // 끊긴 사이 놓친 알림이 있을 수 있어 새로 시작한다. 아직 막혀 있으면 서버가 입장 직후 다시 알린다.
          setOutOfTime(false)
          setHostAway(false)
          setSpendPaused(false)
          setStatus({ kind: 'joined', selfId: msg.you })
          setIsHost(msg.host)
          setSettings(msg.settings)
          setKnocks(msg.knocks)
          setPeers(Object.fromEntries(msg.peers.map((p) => [p.id, p])))
          setNames((n) => ({ ...n, ...Object.fromEntries(msg.peers.map((p) => [p.id, p.name])) }))
          msg.peers.forEach((p) => mesh.connect(p.id))
          break
        }
        case 'peer_joined':
        case 'peer_updated':
          setPeers((ps) => ({ ...ps, [msg.peer.id]: msg.peer }))
          setNames((n) => ({ ...n, [msg.peer.id]: msg.peer.name }))
          break
        case 'peer_left':
          meshRef.current?.remove(msg.id)
          setPeers((ps) => without(ps, msg.id))
          setStreams((s) => without(s, msg.id))
          break
        case 'signal':
          meshRef.current?.handleSignal(msg.from, msg.data)
          break
        case 'waiting':
          // 토큰이 만료돼 다시 수락을 기다리는 경우: 이제부터는 처음 입장할 때처럼 다룬다.
          resumeRef.current = null
          everJoinedRef.current = false
          setDisconnected(false)
          setStatus({ kind: 'waiting' })
          break
        case 'denied':
          stoppedRef.current = true
          setStatus({ kind: 'error', message: 'room.denied' })
          break
        case 'knock':
          setKnocks((ks) => [...ks.filter((k) => k.id !== msg.knock.id), msg.knock])
          break
        case 'knock_resolved':
          setKnocks((ks) => ks.filter((k) => k.id !== msg.id))
          break
        case 'settings':
          setSettings(msg.settings)
          break
        case 'caption': {
          const now = Date.now()
          setCaptions((list) => applyCaption(list, msg, now))
          transcriptRef.current.add(msg, now)
          setHasTranscript(transcriptRef.current.size > 0)
          break
        }
        case 'stt_token':
          tokensRef.current.get(msg.id)?.resolve({ token: msg.token, model: msg.model })
          tokensRef.current.delete(msg.id)
          break
        case 'stt_token_denied':
          tokensRef.current.get(msg.id)?.reject(msg.retry ? 'failed' : 'unavailable')
          tokensRef.current.delete(msg.id)
          break
        case 'notice':
          if (msg.code === 'translation_failed') setToast({ key: 'captions.failed' })
          else if (msg.code === 'quota_exceeded') setOutOfTime(true)
          else if (msg.code === 'quota_restored') {
            setOutOfTime(false)
            setQuotaClosed(false)
            setToast({ key: 'quota.restored' })
          }
          else if (msg.code === 'host_away') setHostAway(true)
          else if (msg.code === 'spend_limit') setSpendPaused(true)
          else if (msg.code === 'spend_restored') {
            setSpendPaused(false)
            setToast({ key: 'quota.restored' })
          }
          else if (msg.code === 'host_back') {
            setHostAway(false)
            setToast({ key: 'quota.restored' })
          }
          break
        case 'error': {
          const fatal = FATAL_ERRORS[msg.code]
          if (!joinedRef.current) {
            stoppedRef.current = true
            resumeRef.current = null
            setStatus({ kind: 'error', message: fatal ?? 'room.cannotJoin' })
          }
          else if (msg.code === 'room_full') setToast({ key: 'room.full' })
          else if (msg.code === 'room_not_found') {
            // 회의가 지워졌다(주최자가 삭제). 다시 연결하지 않고 바로 알린다.
            stoppedRef.current = true
            resumeRef.current = null
            setStatus({ kind: 'error', message: 'room.notFound' })
          }
          else console.warn('server error', msg.code, msg.message)
          break
        }
      }
    }

    const joinedRef = { current: false }
    const sig = new Signaling(onMessage, (opened) => {
      if (disposed) return
      // 이 시도는 끝났다. 연결 시간 제한이 같은 시도에 재시도를 한 번 더 걸지 않게 한다.
      clearTimeout(connectTimer)
      screenRef.current?.dispose()
      screenRef.current = null
      setScreenTrack(null)
      setScreenBusy(false)
      meshRef.current?.close()
      meshRef.current = null
      setStreams({})
      if (mediaRef.current.screen) {
        const next = { ...mediaRef.current, screen: false }
        mediaRef.current = next
        onMediaChangeRef.current(next)
      }
      if (stoppedRef.current) return
      if (!everJoinedRef.current) {
        setStatus({ kind: 'error', message: opened ? 'room.disconnected' : 'room.serverDown' })
        return
      }
      scheduleRetry()
    })
    // 입장한 적이 있으면 화면을 그대로 두고 1·2·4·8·10초… 간격으로 다시 연결한다.
    function scheduleRetry() {
      setDisconnected(true)
      if (retryTimerRef.current) clearTimeout(retryTimerRef.current)
      const delay = Math.min(10_000, 1000 * 2 ** retriesRef.current++)
      retryTimerRef.current = setTimeout(() => {
        retryTimerRef.current = null
        setAttempt((a) => a + 1)
      }, delay)
    }
    // 응답 없는 망(VPN 전환·캡티브 포털)에서는 연결 시도가 OS 시간 제한(1분 이상)까지 걸려 있을 수 있다.
    const connectTimer = setTimeout(() => {
      if (disposed || sig.hasOpened || stoppedRef.current) return
      sig.close()
      if (everJoinedRef.current) scheduleRetry()
      else setStatus({ kind: 'error', message: 'room.serverDown' })
    }, everJoinedRef.current ? RECONNECT_TIMEOUT_MS : FIRST_CONNECT_TIMEOUT_MS)
    sigRef.current = sig
    segRef.current = { next: 0 }

    // ICE 서버(TURN 자격 포함)는 welcome으로 온다. 열리기 전에 보낸 join은 큐에 모였다가 바로 나간다.
    const p = prefsRef.current
    sig.send({
      t: 'join',
      room: code,
      name: nameRef.current,
      lang: p.lang,
      partner_lang: p.partnerLang,
      media: announced(local, mediaRef.current),
      ...(resumeRef.current ? { resume: resumeRef.current } : {}),
    })
    // 토큰은 welcome에서 새것으로 바뀔 때까지 지니고 있는다(이 연결이 열리지 못하면 다음 시도에 다시 쓴다).

    return () => {
      disposed = true
      clearTimeout(connectTimer)
      if (retryTimerRef.current) clearTimeout(retryTimerRef.current)
      sig.close()
      screenRef.current?.dispose()
      screenRef.current = null
      meshRef.current?.close()
      meshRef.current = null
      sigRef.current = null
    }
  }, [code, local, attempt])

  // 망이 돌아오면 기다리지 않고 바로 다시 연결한다.
  useEffect(() => {
    if (!disconnected) return
    const retryNow = () => {
      // 이미 진행 중인 시도는 건드리지 않는다(입장 직전에 끊으면 받은 토큰을 잃는다).
      if (!retryTimerRef.current) return
      clearTimeout(retryTimerRef.current)
      retryTimerRef.current = null
      setAttempt((a) => a + 1)
    }
    window.addEventListener('online', retryNow)
    return () => window.removeEventListener('online', retryNow)
  }, [disconnected])

  // 오류 화면(최종 실패)으로 바뀌면 장치를 놓게 한다. 그대로 두면 거절된 뒤에도 카메라 불이 켜져 있다.
  const failed = status.kind === 'error'
  const onGiveUpRef = useRef(onGiveUp)
  useEffect(() => {
    onGiveUpRef.current = onGiveUp
  }, [onGiveUp])
  useEffect(() => {
    if (failed) onGiveUpRef.current()
  }, [failed])

  useEffect(() => {
    if (!toast) return
    const id = setTimeout(() => setToast(null), toast.ms ?? 3000)
    return () => clearTimeout(id)
  }, [toast])

  const joined = status.kind === 'joined'
  // 입장한 뒤에만 미디어 상태를 보낸다(다시 연결하는 동안 바뀐 상태는 다음 join이 싣는다). 나중에 불리는 콜백이 본다.
  const canAnnounceRef = useRef(false)
  useEffect(() => {
    canAnnounceRef.current = joined
  }, [joined])
  const setMediaAndAnnounce = useCallback(
    (next: MediaState) => {
      mediaRef.current = next
      onMediaChangeRef.current(next)
      if (canAnnounceRef.current) sigRef.current?.send({ t: 'media', media: announced(local, next) })
    },
    [local],
  )

  // 마이크·카메라를 다시 잡으면 올린다(트랙이 바뀌어 음성 인식·내 화면이 새 트랙을 쓰게).
  const [trackGen, setTrackGen] = useState(0)
  // 끝난 장치를 다시 켜 달라고 했는데 아직 못 잡았다. 그 뒤 잠깐(WANT_MS) 안에 장치가 바뀌면(다시 꽂음) 다시 시도한다.
  // 기한을 두는 건 한참 뒤 상관없는 장치(이어폰 등)가 연결될 때 마이크가 저절로 켜지지 않게 하려는 것.
  const wantRef = useRef({ mic: 0, cam: 0 })
  const reacquiringRef = useRef({ mic: false, cam: false })
  const mountedRef = useRef(true)
  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
    }
  }, [])

  // 장치가 빠지거나 권한이 회수되면 트랙이 끝난다. 끈 것으로 알리고 안내한다(켜진 채 빈 타일로 남지 않게).
  useEffect(() => {
    const tracks = local.getTracks()
    const onEnded = (e: Event) => {
      const kind = (e.target as MediaStreamTrack).kind === 'audio' ? 'mic' : 'cam'
      setMediaAndAnnounce({ ...mediaRef.current, [kind]: false })
      setToast({ key: kind === 'mic' ? 'room.micEnded' : 'room.camEnded', ms: 6000 })
    }
    tracks.forEach((t) => t.addEventListener('ended', onEnded))
    return () => tracks.forEach((t) => t.removeEventListener('ended', onEnded))
  }, [local, trackGen, setMediaAndAnnounce])

  const reacquire = useCallback(
    async (kind: 'mic' | 'cam') => {
      if (reacquiringRef.current[kind]) return
      reacquiringRef.current[kind] = true
      try {
        const fresh = await reacquireTrack(kind === 'mic' ? 'audio' : 'video')
        // 그사이 회의를 나갔으면 장치를 바로 놓는다.
        if (!mountedRef.current) return fresh.stop()
        wantRef.current[kind] = 0
        for (const old of kind === 'mic' ? local.getAudioTracks() : local.getVideoTracks()) {
          local.removeTrack(old)
          old.stop()
        }
        local.addTrack(fresh)
        const mesh = meshRef.current
        const replaced =
          kind === 'mic'
            ? mesh?.setAudioTrack(fresh)
            : // 화면 공유 중이면 공유가 끝날 때 이 카메라로 돌아간다(ScreenShare에 넘긴 함수).
              sharingRef.current
              ? undefined
              : mesh?.setVideoTrack(fresh)
        replaced?.catch((err) => console.error('device replacement failed', kind, err))
        setTrackGen((g) => g + 1)
        setMediaAndAnnounce({ ...mediaRef.current, [kind]: true })
      } catch (err) {
        console.warn('device unavailable', kind, err)
        if (mountedRef.current) setToast({ key: 'room.deviceRetry', ms: 6000 })
      } finally {
        reacquiringRef.current[kind] = false
      }
    },
    [local, setMediaAndAnnounce],
  )

  useEffect(() => {
    const devices = navigator.mediaDevices
    if (!devices?.addEventListener) return
    const onChange = () => {
      for (const kind of ['mic', 'cam'] as const) {
        if (wantRef.current[kind] > Date.now() && !hasLiveTrack(local, kind)) void reacquire(kind)
      }
    }
    devices.addEventListener('devicechange', onChange)
    return () => devices.removeEventListener('devicechange', onChange)
  }, [local, reacquire])

  const micTrack = local.getAudioTracks()[0] ?? null
  // 내 말은 내 브라우저가 직접 인식한다. 번역이 켜진 방에 입장해 있는 동안만.
  // 주최자의 번역 시간이 떨어지거나 주최자가 오래 없으면 음성 인식을 멈추고, 풀리면(서버 알림) 효과가 다시 켠다.
  const sttEnabled = joined && !disconnected && settings.translation && !!micTrack && !outOfTime && !hostAway && !spendPaused

  useEffect(() => {
    const sig = sigRef.current
    if (!sttEnabled || !micTrack || !sig) return
    const tokens = tokensRef.current
    const speech = startSpeech({
      track: micTrack,
      lang: langRef.current,
      mic: mediaRef.current.mic,
      seg: segRef.current,
      requestToken: () =>
        new Promise<Grant>((resolve, reject) => {
          const id = ++tokenIdRef.current
          tokens.set(id, { resolve, reject })
          sig.send({ t: 'stt_token', id })
          setTimeout(() => {
            if (!tokens.delete(id)) return
            reject('failed')
          }, 10_000)
        }),
      sendStt: (seg, text, final) => sig.send({ t: 'stt', seg, text, final }),
      onStatus: setSttStatus,
      farEnd: () => meshRef.current?.audioReceivers() ?? [],
      onEcho: () => {
        if (echoHintedRef.current) return
        echoHintedRef.current = true
        // 두 문장짜리 안내라 다른 알림보다 오래 보여 준다.
        setToast({ key: 'captions.echo', ms: 6000 })
      },
    })
    speechRef.current = speech
    return () => {
      speech.close()
      speechRef.current = null
      // 늦게 온 답은 위에서 id를 못 찾아 버려진다.
      for (const waiter of tokens.values()) waiter.reject('failed')
      tokens.clear()
      setSttStatus(null)
    }
  }, [sttEnabled, micTrack])

  useEffect(() => {
    speechRef.current?.setMic(media.mic)
  }, [media.mic, sttEnabled])

  useEffect(() => {
    speechRef.current?.setLang(sttKey)
  }, [sttKey, sttEnabled])
  // 받은 오디오 레벨로 말하는 상대를 표시한다. 함수는 effect 의존성이라 고정해 둔다.
  const peerReceivers = useCallback(() => meshRef.current?.audioReceiversByPeer() ?? [], [])
  const speaking = useSpeakingPeers(peerReceivers, joined && !disconnected && Object.keys(peers).length > 0)
  // 공유가 끝나면(직접 중지·연결 끊김 모두) 자막 창도 닫는다.
  const pipClose = pip.close
  useEffect(() => {
    if (!screenTrack) pipClose()
  }, [screenTrack, pipClose])
  const showPanel = joined && panelOpen && (settings.translation || isHost)
  // 장치를 다시 잡은 뒤에는 새 스트림 객체로 넘긴다(내 화면이 바뀐 트랙을 다시 붙이게).
  const selfStream = useMemo(
    () => (screenTrack ? new MediaStream([screenTrack]) : trackGen ? new MediaStream(local.getTracks()) : local),
    [screenTrack, local, trackGen],
  )

  function changeMedia(next: MediaState) {
    // 끝난 장치를 다시 켜면 새로 잡아 본다(잡으면 켜진다).
    const revive = (['mic', 'cam'] as const).find((k) => next[k] && !media[k] && !hasLiveTrack(local, k))
    if (revive) {
      wantRef.current[revive] = Date.now() + WANT_MS
      void reacquire(revive)
      return
    }
    setMediaAndAnnounce(next)
  }

  function changePrefs(next: Prefs) {
    onPrefsChange(next)
    if (joined) sigRef.current?.send({ t: 'lang', lang: next.lang, partner_lang: next.partnerLang })
  }

  async function copyLink() {
    try {
      await navigator.clipboard.writeText(location.href)
      setToast({ key: 'room.linkCopied' })
    } catch {
      setToast({ key: 'room.linkFallback', params: { url: location.href } })
    }
  }

  function speakerName(id: string) {
    return selfIdsRef.current.has(id) ? t('room.you', { name: prefs.name.trim() }) : (names[id] ?? '')
  }

  function transcriptText(): string {
    const fileName = (id: string) => speakerName(id) || t('captions.participant')
    return formatTranscript(transcriptRef.current.entries(), {
      title: t('transcript.title'),
      code,
      savedAt: new Date(),
      locale,
      speakerName: fileName,
    })
  }

  function downloadTranscript() {
    downloadText(transcriptFileName(code, new Date()), transcriptText())
  }

  function leave() {
    // 나가는 클릭을 늦추지 않도록 텍스트는 저장을 누를 때 만든다.
    const size = transcriptRef.current.size
    onLeave({ exportTranscript: size > 0 ? transcriptText : null, joinedAt: joinedAtRef.current, sentences: size })
  }

  function admit(id: string) {
    sigRef.current?.send({ t: 'admit', id })
  }

  function deny(id: string) {
    sigRef.current?.send({ t: 'deny', id })
  }

  function changeTranslation(translation: boolean) {
    sigRef.current?.send({ t: 'settings', settings: { translation } })
  }

  if (status.kind === 'waiting') {
    return (
      <StatusPage
        icon="spinner"
        live
        title={t('room.waitingTitle')}
        body={t('room.waitingBody')}
        actions={
          <button className="btn btn-outline" onClick={leave}>
            {t('room.cancel')}
          </button>
        }
      />
    )
  }

  if (status.kind === 'error') {
    if (status.message === 'room.notFound') return <MeetingGonePage />
    return (
      <StatusPage
        title={t('room.cannotJoin')}
        body={t(status.message)}
        actions={
          <>
            <button className="btn btn-outline" onClick={onRejoin}>
              {t('room.retry')}
            </button>
            <button className="btn btn-primary" onClick={leave}>
              {t('room.leave')}
            </button>
          </>
        }
      />
    )
  }

  const hasCam = local.getVideoTracks().length > 0
  const hasMic = local.getAudioTracks().length > 0
  const peerList = Object.values(peers)
  const alone = peerList.length === 0 && joined
  const presenterId = screenTrack ? 'self' : peerList.find((p) => p.media.screen)?.id

  const selfTile = (
    <VideoTile
      key="self"
      name={t('room.you', { name: prefs.name.trim() })}
      stream={selfStream}
      showVideo={!!screenTrack || (hasCam && media.cam)}
      micOn={hasMic && media.mic}
      mirror={!screenTrack}
      muted
      presenting={!!screenTrack}
      className={presenterId === 'self' ? 'main' : undefined}
    />
  )
  const peerTiles = peerList.map((p) => (
    <VideoTile
      key={p.id}
      name={p.name}
      stream={streams[p.id] ?? null}
      showVideo={!!streams[p.id]?.getVideoTracks().length && (p.media.cam || p.media.screen)}
      micOn={p.media.mic}
      speaking={p.media.mic && speaking.has(p.id)}
      presenting={p.media.screen}
      className={presenterId === p.id ? 'main' : undefined}
    />
  ))
  const tiles = [...peerTiles, selfTile]
  const mainTile = tiles.find((t) => t.key === (presenterId ?? ''))

  return (
    <div className={`room${showPanel ? ' with-panel' : ''}`}>
      {disconnected && (
        <div className="banner" role="status">
          <span className="spinner spinner-sm" aria-hidden="true" />
          {t('room.reconnecting')}
        </div>
      )}
      <div className="room-main">
        {mainTile ? (
          <section className="stage presenting" aria-label={t('room.participants')}>
            {mainTile}
            <div className="strip">{tiles.filter((t) => t !== mainTile)}</div>
          </section>
        ) : (
          <section
            className={`stage count-${tiles.length}${alone ? ' alone' : ''}`}
            aria-label={t('room.participants')}
          >
            {tiles}
            {alone && !inviteClosed && (
              // 내 화면은 무대를 다 쓰고, 초대 카드는 그 위 왼쪽 아래에 띄운다(닫을 수 있음, 링크 복사는 아래 캡슐에도 있다).
              // 내 화면을 감싸지 않는다: 감싸면 혼자 ↔ 상대 있음이 바뀔 때 영상이 다시 마운트돼 깜박인다.
              <div className="invite" role="region" aria-labelledby="invite-title">
                <button
                  className="icon-btn invite-close"
                  aria-label={t('room.closeInvite')}
                  onClick={() => {
                    setInviteClosed(true)
                    // 닫으면 이 버튼이 사라진다. 키보드 포커스를 같은 일을 하는 아래 캡슐의 링크 복사 버튼으로 옮긴다.
                    copyLinkRef.current?.focus()
                  }}
                >
                  <Icon name="close" />
                </button>
                <h2 id="invite-title">{t('room.aloneTitle')}</h2>
                <p>{t('room.aloneBody')}</p>
                <code>{location.host + location.pathname}</code>
                <button className="btn btn-primary" onClick={copyLink}>
                  <Icon name="content_copy" />
                  {t('room.copyLink')}
                </button>
              </div>
            )}
          </section>
        )}
        {showPanel && (
          <CaptionPanel
            prefs={prefs}
            translation={settings.translation}
            isHost={isHost}
            captions={captions}
            sttStatus={sttStatus}
            speakerName={speakerName}
            onDownload={hasTranscript ? downloadTranscript : null}
            onPrefsChange={changePrefs}
            onTranslationChange={changeTranslation}
            onClose={() => setPanelOpen(false)}
          />
        )}
      </div>

      <div className="room-meta">
        <Clock locale={locale} />
        <span aria-hidden="true">·</span>
        {code}
        <LocaleSelect />
      </div>

      {/* 무대 아래 가운데에 뜨는 유리 캡슐: 마이크·카메라·자막·공유·링크 | 나가기 */}
      <div className="capsule" role="toolbar" aria-label={t('room.controls')}>
        <button
          className={`ctl ${hasMic && media.mic ? '' : 'off'}`}
          disabled={!joined || disconnected || !hasMic}
          aria-pressed={media.mic}
          aria-label={media.mic ? t('ctl.micOff') : t('ctl.micOn')}
          onClick={() => changeMedia({ ...media, mic: !media.mic })}
        >
          <Icon name={hasMic && media.mic ? 'mic' : 'mic_off'} />
        </button>
        <button
          className={`ctl ${hasCam && media.cam ? '' : 'off'}`}
          disabled={!joined || disconnected || !hasCam}
          aria-pressed={media.cam}
          aria-label={media.cam ? t('ctl.camOff') : t('ctl.camOn')}
          onClick={() => changeMedia({ ...media, cam: !media.cam })}
        >
          <Icon name={hasCam && media.cam ? 'videocam' : 'videocam_off'} />
        </button>
        {/* 번역이 꺼지면 게스트에게는 자막 패널이 없다. 주최자는 다시 켤 수 있도록 남긴다. */}
        {joined && (settings.translation || isHost) && (
          <button
            className={`ctl ${showPanel ? 'on' : ''}`}
            aria-pressed={showPanel}
            aria-label={t('ctl.captions')}
            onClick={() => setPanelOpen((o) => !o)}
          >
            <Icon name="subtitles" />
          </button>
        )}
        {canShare && (
          <button
            className={`ctl ${screenTrack ? 'on' : ''}`}
            disabled={!joined || disconnected || screenBusy || (!!presenterId && !screenTrack)}
            aria-pressed={!!screenTrack}
            aria-label={screenTrack ? t('ctl.stopShare') : t('ctl.share')}
            onClick={() => { void screenRef.current?.toggle() }}
          >
            <Icon name={screenTrack ? 'cancel_presentation' : 'present_to_all'} />
          </button>
        )}
        <button ref={copyLinkRef} className="ctl" aria-label={t('ctl.copyLink')} onClick={copyLink}>
          <Icon name="link" />
        </button>
        {screenTrack && pipSupported && (
          <button
            className={`ctl ${pip.win ? 'on' : ''}`}
            aria-pressed={!!pip.win}
            aria-label={pip.win ? t('pip.close') : t('pip.open')}
            title={pip.win ? t('pip.close') : t('pip.open')}
            onClick={() => (pip.win ? pip.close() : void pip.open(() => sharingRef.current))}
          >
            <Icon name="picture_in_picture_alt" />
          </button>
        )}
        <span className="capsule-sep" aria-hidden="true" />
        <button className="ctl leave" aria-label={t('ctl.leave')} onClick={leave}>
          <Icon name="call_end" />
        </button>
      </div>

      {/* 공유를 시작한 클릭에서 자막 창이 열리지 않았으면(브라우저가 막음) 한 번 더 누르게 안내한다. */}
      {screenTrack && pipSupported && !pip.win && !pip.closedByUser && !pipDismissed && (
        <div className="pip-prompt" role="status">
          <button className="icon-btn pip-prompt-close" aria-label={t('room.closeInvite')} onClick={() => setPipDismissed(true)}>
            <Icon name="close" />
          </button>
          <p>
            <b>{t('pip.prompt')}</b>
            {t('pip.hint')}
          </p>
          <button className="btn btn-primary" onClick={() => void pip.open(() => sharingRef.current)}>
            <Icon name="picture_in_picture_alt" />
            {t('pip.open')}
          </button>
        </div>
      )}
      {pip.win && screenTrack && (
        <CaptionPip
          win={pip.win}
          captions={captions}
          speakerName={speakerName}
          hasMic={hasMic}
          micOn={hasMic && media.mic}
          onMic={() => changeMedia({ ...media, mic: !media.mic })}
          onStop={() => void screenRef.current?.toggle()}
        />
      )}
      {outOfTime && isHost && !quotaClosed && <QuotaModal onClose={() => setQuotaClosed(true)} />}
      {isHost && <KnockPrompts knocks={knocks} onAdmit={admit} onDeny={deny} />}
      {/* 계속 뜨는 안내가 아래, 잠깐 뜨는 안내가 그 위에 쌓인다(겹치지 않게). */}
      <div className="toasts">
        {outOfTime && !isHost && (
          <div className="toast" role="status">
            {t('quota.guest')}
          </div>
        )}
        {spendPaused && (
          <div className="toast" role="status">
            {t('room.spendPaused')}
          </div>
        )}
        {hostAway && !outOfTime && !spendPaused && (
          <div className="toast" role="status">
            {t('room.hostAway')}
          </div>
        )}
        {toast && (
          <div className="toast" role="status">
            {t(toast.key, toast.params)}
          </div>
        )}
      </div>
    </div>
  )
}

function Clock({ locale }: { locale: Locale }) {
  const [now, setNow] = useState(() => new Date())
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 15_000)
    return () => clearInterval(id)
  }, [])
  return <span>{now.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit', hour12: false })}</span>
}

function hasLiveTrack(local: MediaStream, kind: 'mic' | 'cam'): boolean {
  return (kind === 'mic' ? local.getAudioTracks() : local.getVideoTracks()).some((t) => t.readyState === 'live')
}

/** 다른 참가자에게 알릴 상태: 실제로 쓸 수 있는 장치만 켜진 것으로 본다(카메라가 없거나 트랙이 끝났으면 꺼짐). */
function announced(local: MediaStream, media: MediaState): MediaState {
  return { ...media, mic: media.mic && hasLiveTrack(local, 'mic'), cam: media.cam && hasLiveTrack(local, 'cam') }
}

function without<T>(record: Record<string, T>, key: string): Record<string, T> {
  const next = { ...record }
  delete next[key]
  return next
}
