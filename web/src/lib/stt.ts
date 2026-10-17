/**
 * 내 마이크 → Gemini Live 음성 인식 → `/ws`의 `stt` 메시지. 브라우저 하나가 자기 마이크만 인식한다.
 * 규칙과 측정 근거는 docs/ARCHITECTURE.md("음성 구간 감지(VAD)와 침묵 비용").
 *
 * - 말하는 구간만 보낸다(클라이언트 VAD → activityStart / activityEnd).
 * - 세션은 미리 열어 둔다. 말이 시작될 때 토큰 발급(~0.3s)과 연결(~0.4s)을 기다리지 않게.
 * - 침묵 중 30초마다 100ms 무음(keepalive). 입력이 60초 없으면 서버가 끊는다.
 * - 교체: 3분 넘게 조용하거나, 9분이 지났거나, 언어가 바뀌었거나, goAway를 받으면 새 세션을
 *   백그라운드에서 열어 두고 말이 끊긴 틈(확정 결과까지 받은 뒤)에 바꾼다. 마이크가 꺼져 있으면 닫기만 한다.
 * - 끊기면 다시 연다. 연달아 실패하면 점점 늦게 다시 시도하고 그동안 "일시 중단"으로 표시한다.
 *
 * 시간에 따른 일은 모두 `tick()`이 한다(테스트에서 시계를 직접 돌릴 수 있게).
 */
import type { Session, LiveEvents } from './live'
import type { VadResult } from './vad'

export type SttStatus = 'starting' | 'live' | 'paused' | 'unavailable'
export type Grant = { token: string; model: string }
/** `unavailable`이면 이 연결에서는 다시 요청하지 않는다. */
export type TokenError = 'unavailable' | 'failed'

/** 서버 연결(`/ws`) 하나에서 STT 구간 번호는 계속 늘어나야 한다. 컨트롤러가 다시 만들어져도 공유한다. */
export type SegCounter = { next: number }

export type SttDeps = {
  requestToken: () => Promise<Grant>
  openSession: (grant: Grant, events: LiveEvents) => Session
  sendStt: (seg: number, text: string, final: boolean) => void
  onStatus: (status: SttStatus) => void
  now: () => number
}

/** 100ms = 20ms 프레임 5개를 모아 보낸다. */
const FRAMES_PER_CHUNK = 5
const KEEPALIVE_MS = 30_000
const IDLE_ROTATE_MS = 3 * 60_000
const MAX_AGE_MS = 9 * 60_000
/** 오디오 세션 상한(15분) 전에는 말하는 중이어도 교체한다. */
const HARD_AGE_MS = 14 * 60_000
/** activityEnd 뒤 확정 결과(보통 0.3~0.4초)를 기다리는 시간. 넘으면 마지막 중간 결과를 확정으로 본다. */
const FINAL_WAIT_MS = 2_000
/** 이만큼 정상 동작한 세션이 끊기면 실패로 세지 않고 바로 다시 연다. */
const HEALTHY_MS = 30_000
const BACKOFF_MS = [1_000, 3_000, 10_000, 30_000]
/** 세션이 없을 때 쌓아 두는 메시지(약 10초 분량) */
const MAX_BACKLOG = 110
/** 쉬지 않고 오래 말해 중간 결과가 이보다 길어지면 그 자리에서 구간을 끊는다(서버는 메시지 하나에 1000자까지 받는다). */
const MAX_INTERIM_CHARS = 800
const KEEPALIVE_SILENCE = new Int16Array(1600)

type Slot = {
  session: Session
  lang: string
  openedAt: number
  lastSentAt: number
  ready: boolean
  goAway: boolean
}

type Msg = { kind: 'start' } | { kind: 'audio'; pcm: Int16Array } | { kind: 'end' }

export class SttController {
  private deps: SttDeps
  private seg: SegCounter
  private lang: string
  private micOn = false
  /** 마이크가 꺼져 있어도 세션을 열어 둔다(대면 모드: 말할 차례가 오면 바로 보낼 수 있게). */
  private warm = false
  private micChangedAt = 0
  private disposed = false
  private unavailable = false
  private current: Slot | null = null
  private next: Slot | null = null
  private tokenPending = false
  private failures = 0
  private retryAt = 0
  private speaking = false
  /** 끝났는데 확정 결과를 아직 받지 못한 발화 수. 앞 발화의 확정이 다음 발화가 끝난 뒤에 와도 둘 다 받는다. */
  private pendingFinals = 0
  private lastActivityAt = 0
  /** 이번 구간에 마지막으로 보낸 텍스트(바뀐 경우에만 보낸다) */
  private sent = ''
  /** 이번 발화에서 길어서 먼저 확정해 보낸 앞부분. Gemini 결과는 발화 처음부터 누적이라 이만큼 떼고 보낸다. */
  private cut = ''
  private chunk: Int16Array[] = []
  private backlog: Msg[] = []
  private status: SttStatus | null = null

  constructor(deps: SttDeps, seg: SegCounter, lang: string) {
    this.deps = deps
    this.seg = seg
    this.lang = lang
    this.lastActivityAt = deps.now()
    this.update()
  }

  setMic(on: boolean) {
    if (this.micOn === on) return
    this.micOn = on
    this.micChangedAt = this.deps.now()
    if (!on && this.speaking) this.endUtterance()
    this.update()
  }

  setWarm(on: boolean) {
    this.warm = on
    this.update()
  }

  setLang(lang: string) {
    this.lang = lang
    this.update()
  }

  /** VAD 결과 하나. */
  vad(result: VadResult) {
    if (!result || !this.micOn || this.disposed) return
    if (result.kind === 'start') {
      this.speaking = true
      this.chunk = []
      this.emit({ kind: 'start' })
    }
    for (const frame of result.frames) {
      this.chunk.push(frame)
      if (this.chunk.length >= FRAMES_PER_CHUNK) this.flushChunk()
    }
    if (result.kind === 'end') this.endUtterance()
    else this.lastActivityAt = this.deps.now()
  }

  tick() {
    if (this.disposed) return
    const now = this.deps.now()
    // activityEnd 뒤 확정이 오지 않았거나, 확정 뒤에 중간 결과만 하나 더 왔다.
    const open = this.pendingFinals > 0 || this.sent !== ''
    // 확정 대기는 activityEnd가 실제로 Gemini에 간 때부터 센다. 세션이 아직 없어 오디오가 쌓여 있거나, 세션은 있어도
    // 아직 준비 전(보낸 것이 세션 안 큐에 있음)이면 기다린다(늦게 연결돼도 그 발화의 결과를 버리지 않게).
    // 준비되지 못하면 세션이 닫히며(설정 시간 초과) 그때 로컬로 확정한다.
    const delivered = !this.backlog.length && (this.current?.ready ?? true)
    if (!this.speaking && open && delivered && now - this.lastActivityAt >= FINAL_WAIT_MS) this.finalizeLocally()
    // 교체를 기다리는 세션도 입력이 60초 없으면 끊기므로 같이 살려 둔다.
    for (const slot of [this.current, this.next]) {
      if (slot?.ready && !(this.speaking && slot === this.current) && now - slot.lastSentAt >= KEEPALIVE_MS) {
        slot.session.audio(KEEPALIVE_SILENCE)
        slot.lastSentAt = now
      }
    }
    this.update()
  }

  dispose() {
    if (this.disposed) return
    if (this.speaking) this.endUtterance()
    // 세션을 바로 닫으므로 확정 결과는 오지 않는다. 받은 데까지 확정하고 구간 번호를 넘긴다.
    this.finalizeLocally()
    this.disposed = true
    this.current?.session.close()
    this.next?.session.close()
    this.current = this.next = null
    this.backlog = []
  }

  private update() {
    if (this.disposed) return
    const now = this.deps.now()
    const cur = this.current
    const quiet = !this.speaking && this.pendingFinals === 0 && this.sent === ''
    const wanted = this.micOn || this.warm

    if (!wanted && cur && quiet && now - this.micChangedAt >= IDLE_ROTATE_MS) {
      // 마이크가 오래 꺼져 있으면 세션을 열어 두지 않는다(다시 켜면 새로 연다).
      this.closeSlot('current')
      this.closeSlot('next')
    } else if (cur) {
      const age = now - cur.openedAt
      // 침묵은 이 세션이 열린 뒤부터 센다(교체 직후 다시 교체하지 않게). keepalive는 말이 아니다.
      const idle = now - Math.max(this.lastActivityAt, cur.openedAt) >= IDLE_ROTATE_MS
      const stale = cur.goAway || cur.lang !== this.lang || age >= MAX_AGE_MS || idle
      // 더 필요 없거나(언어를 되돌림) 언어가 맞지 않는 대기 세션은 닫는다.
      if (this.next && (!stale || this.next.lang !== this.lang)) this.closeSlot('next')
      if (stale && wanted) {
        if (!this.next) this.open()
        else if (this.next.ready && (quiet || age >= HARD_AGE_MS)) this.swap()
      }
    } else if (wanted || this.backlog.length) {
      this.open()
    }
    this.publishStatus()
  }

  private open() {
    if (this.unavailable || this.tokenPending || this.deps.now() < this.retryAt) return
    this.tokenPending = true
    // 서버는 요청을 받은 시점의 내 언어 + 상대 언어로 토큰을 잠근다(`lang` 메시지가 같은 연결에서 먼저 간다).
    // `lang`은 그 둘을 나타내는 세션 열쇠다(바뀌면 세션을 새로 연다).
    const lang = this.lang
    this.deps.requestToken().then(
      (grant) => {
        this.tokenPending = false
        if (this.disposed) return
        this.attach(grant, lang)
        this.update()
      },
      (err: TokenError) => {
        this.tokenPending = false
        if (this.disposed) return
        if (err === 'unavailable') {
          this.unavailable = true
          this.backlog = []
        } else this.fail()
        this.update()
      },
    )
  }

  private attach(grant: Grant, lang: string) {
    const now = this.deps.now()
    let slot: Slot | null = null
    const session = this.deps.openSession(grant, {
      onReady: () => {
        if (!slot) return
        slot.ready = true
        // 준비 전에 보낸 activityEnd는 지금 실제로 나간다. 확정 대기를 여기서부터 센다.
        if (slot === this.current && this.pendingFinals > 0) this.lastActivityAt = this.deps.now()
        this.failures = 0
        this.update()
      },
      onInterim: (text) => slot === this.current && this.onTranscript(text, false),
      onFinal: (text) => slot === this.current && this.onTranscript(text, true),
      onGoAway: () => {
        if (slot) slot.goAway = true
        this.update()
      },
      onClose: () => slot && this.onSlotClosed(slot),
    })
    slot = { session, lang, openedAt: now, lastSentAt: now, ready: false, goAway: false }
    if (!this.current) {
      this.current = slot
      for (const msg of this.backlog) this.deliver(slot, msg)
      this.backlog = []
    } else {
      const prev = this.next
      this.next = slot
      prev?.session.close()
    }
  }

  private swap() {
    const old = this.current
    const fresh = this.next
    this.current = fresh
    this.next = null
    // 말하는 중에 강제로 바꾸면(상한 직전) 받은 데까지 확정하고 새 세션에서 새 구간을 시작한다.
    this.finalizeLocally()
    if (this.speaking && fresh) this.deliver(fresh, { kind: 'start' })
    // 자리를 먼저 바꿔 두었으므로 옛 세션의 닫힘 이벤트는 무시된다.
    old?.session.close()
  }

  private closeSlot(which: 'current' | 'next') {
    const slot = this[which]
    this[which] = null
    slot?.session.close()
  }

  private onSlotClosed(slot: Slot) {
    if (this.disposed) return
    const now = this.deps.now()
    if (slot === this.next) {
      this.next = null
    } else if (slot === this.current) {
      // 말하던 중이었으면 받은 데까지만 확정한다. 이어지는 말은 새 세션의 새 구간이 된다.
      this.finalizeLocally()
      // 교체용으로 열어 둔 세션이 있으면 토큰을 새로 받지 않고 바로 쓴다.
      this.current = this.next
      this.next = null
      if (this.speaking) {
        if (this.current) this.deliver(this.current, { kind: 'start' })
        else this.backlog = [{ kind: 'start' }]
      }
    } else {
      return
    }
    if (!slot.ready || now - slot.openedAt < HEALTHY_MS) this.fail()
    this.update()
  }

  private fail() {
    this.failures++
    this.retryAt = this.deps.now() + BACKOFF_MS[Math.min(this.failures, BACKOFF_MS.length) - 1]
  }

  private onTranscript(text: string, final: boolean) {
    // 기다리는 결과가 없으면 버린다: 로컬 확정 뒤에 늦게 온 확정, 확정 뒤 덧붙은 중간 결과.
    // 받으면 같은 말이 새 구간으로 한 번 더 확정된다.
    if (!this.speaking && this.pendingFinals === 0) return
    const rest = this.afterCut(text)
    if (final) {
      this.pendingFinals = Math.max(0, this.pendingFinals - 1)
      this.cut = ''
      if (rest !== '' || this.sent !== '') {
        this.deps.sendStt(this.seg.next++, rest, true)
        this.sent = ''
      }
    } else if (rest.length > MAX_INTERIM_CHARS) {
      // 받은 데까지 이 구간으로 확정하고, 이어지는 말은 새 구간으로 보낸다(서버 한도에 걸려 글자를 잃지 않게).
      this.deps.sendStt(this.seg.next++, rest, true)
      this.sent = ''
      this.cut = text
    } else if (rest !== this.sent) {
      this.deps.sendStt(this.seg.next, rest, false)
      this.sent = rest
    }
    this.lastActivityAt = this.deps.now()
  }

  /** 먼저 확정해 보낸 앞부분을 뗀다. 그사이 Gemini가 앞쪽 글자를 고쳤으면 같은 길이만큼 뗀다. */
  private afterCut(text: string): string {
    if (this.cut === '') return text
    // 앞머리가 다르거나 더 짧으면 잘린 말의 확정이 사라지고 새 말이 시작된 것이다: 자르지 않는다.
    if (text.length < this.cut.length || !text.startsWith(this.cut.slice(0, 16))) {
      this.cut = ''
      return text
    }
    let at = this.cut.length
    // 서로게이트 쌍(이모지 등) 가운데서 자르지 않는다.
    const c = text.charCodeAt(at)
    if (c >= 0xdc00 && c <= 0xdfff) at++
    return text.slice(at).trimStart()
  }

  /** 확정 결과가 오지 않았다: 마지막 중간 결과를 이 구간의 확정으로 보낸다. */
  private finalizeLocally() {
    this.pendingFinals = 0
    this.cut = ''
    if (this.sent === '') return
    this.deps.sendStt(this.seg.next++, this.sent, true)
    this.sent = ''
  }

  private endUtterance() {
    this.flushChunk()
    this.emit({ kind: 'end' })
    this.speaking = false
    this.lastActivityAt = this.deps.now()
  }

  private flushChunk() {
    if (!this.chunk.length) return
    const len = this.chunk.reduce((n, f) => n + f.length, 0)
    const pcm = new Int16Array(len)
    let at = 0
    for (const f of this.chunk) {
      pcm.set(f, at)
      at += f.length
    }
    this.chunk = []
    this.emit({ kind: 'audio', pcm })
  }

  private emit(msg: Msg) {
    // 확정을 기다릴 발화는 activityEnd를 실제로 보내거나 쌓은 것만 센다(버려진 끝은 확정이 오지 않는다).
    if (this.current) {
      if (msg.kind === 'end') this.pendingFinals++
      return this.deliver(this.current, msg)
    }
    if (this.unavailable) return
    // 너무 오래 연결되지 않으면 가장 최근 발화만 남긴다. 버린 발화의 끝은 기다리지 않는다.
    if (this.backlog.length >= MAX_BACKLOG) {
      this.backlog = this.speaking ? [{ kind: 'start' }] : []
      this.pendingFinals = 0
    }
    if (msg.kind === 'end' && !this.backlog.length) return
    if (msg.kind === 'end') this.pendingFinals++
    this.backlog.push(msg)
    if (msg.kind === 'start') this.update()
  }

  private deliver(slot: Slot, msg: Msg) {
    if (msg.kind === 'start') slot.session.activityStart()
    else if (msg.kind === 'end') {
      slot.session.activityEnd()
      // 확정 대기는 실제로 보낸 때부터(쌓였다가 늦게 보내진 경우 포함).
      this.lastActivityAt = this.deps.now()
    } else {
      slot.session.audio(msg.pcm)
      slot.lastSentAt = this.deps.now()
    }
  }

  private publishStatus() {
    const status: SttStatus = this.unavailable
      ? 'unavailable'
      : this.current?.ready
        ? 'live'
        : this.failures > 0 && (this.micOn || this.warm)
          ? 'paused'
          : 'starting'
    if (status !== this.status) {
      this.status = status
      this.deps.onStatus(status)
    }
  }
}
