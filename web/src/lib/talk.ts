import type { ErrorCode } from '../protocol/ErrorCode'
import type { ServerMsg } from '../protocol/ServerMsg'
import type { CaptionMsg } from './captions'
import { languageName } from './languages'
import { Signaling } from './signaling'
import { startSpeech, type Speech } from './speech'
import type { Grant, SegCounter, SttStatus, TokenError } from './stt'

/**
 * 대면 모드의 한쪽 사람. 한 기기에서 두 사람을 같은 방의 두 참가자(서버 연결 둘)로 나타낸다.
 * 서버의 번역 파이프라인을 그대로 쓰고, 영상·음성 연결(WebRTC)은 없다.
 * 대면 모드(버튼): 음성 인식 세션은 양쪽 다 미리 열어 두고, 마이크 입력만 말하는 쪽으로 돌린다(전환 지연 없음).
 * 자막 화면(자동 판별): `track` 없이 연결만 쓰고, 바깥의 음성 인식 하나가 발화마다 맞는 쪽으로 보낸다.
 */
export type TalkEvents = {
  onJoined: (selfId: string) => void
  onCaption: (msg: CaptionMsg) => void
  onSttStatus: (status: SttStatus | null) => void
  /** 주최자의 번역 시간이 떨어졌다(true) / 다시 생겼다(false). 그동안 음성 인식을 멈춘다. */
  onQuota: (outOfTime: boolean) => void
  /** 서비스 전체가 오늘 지출 상한에 닿았다(true) / 풀렸다(false). 그동안 음성 인식을 멈춘다. */
  onSpendPaused: (paused: boolean) => void
  /** 입장 실패·연결 끊김. 이 연결은 이미 닫혔다. 다시 시도하려면 새로 만든다. */
  onFailed: (reason: TalkFailure) => void
}

export type TalkFailure = ErrorCode | 'disconnected' | 'server_down' | 'not_talk_room' | 'in_use'

const TOKEN_TIMEOUT_MS = 10_000

export class TalkSide {
  private sig: Signaling
  private speech: Speech | null = null
  private tokens = new Map<number, { resolve: (g: Grant) => void; reject: (e: TokenError) => void }>()
  private tokenId = 0
  private seg: SegCounter = { next: 0 }
  private active = false
  private warm = true
  /** 음성 인식 세션 열쇠("내 언어|상대 언어") */
  private lang: string
  private closed = false
  private joined = false
  private outOfTime = false
  private spendPaused = false
  private track: MediaStreamTrack | null
  private events: TalkEvents

  /** `track`이 null이면 자기 음성 인식을 띄우지 않는다(`requestToken`·`sendStt`를 바깥에서 쓴다). */
  constructor(code: string, lang: string, partnerLang: string, track: MediaStreamTrack | null, events: TalkEvents) {
    this.lang = `${lang}|${partnerLang}`
    this.track = track
    this.events = events
    let joined = false
    this.sig = new Signaling(
      (msg) => {
        if (msg.t === 'welcome') joined = true
        this.onMessage(msg, joined)
      },
      (opened) => {
        if (this.closed) return
        this.fail(opened ? 'disconnected' : 'server_down')
      },
    )
    // 이름은 그 사람의 언어 이름(자기 언어로)으로 둔다. 입력받을 필요가 없다.
    this.sig.send({
      t: 'join',
      room: code,
      name: languageName(lang),
      lang,
      partner_lang: partnerLang,
      media: { mic: false, cam: false, screen: false },
    })
  }

  /** 이 사람이 말할 차례면 true. 마이크 입력은 한쪽에만 간다. */
  setActive(on: boolean) {
    this.active = on
    this.speech?.setMic(on)
  }

  /** 세션을 미리 열어 둘지. 오래 아무도 말하지 않으면 끈다(빈 세션 교체 비용을 줄인다). */
  setWarm(on: boolean) {
    this.warm = on
    this.speech?.setWarm(on)
  }

  setLangs(lang: string, partnerLang: string) {
    // 토큰은 내 언어 + 상대 언어로 잠긴다. 어느 쪽이 바뀌어도 세션을 새로 열도록 둘을 열쇠로 쓴다.
    this.lang = `${lang}|${partnerLang}`
    this.sig.send({ t: 'lang', lang, partner_lang: partnerLang })
    this.speech?.setLang(this.lang)
  }

  close() {
    this.closed = true
    this.stopSpeech()
    this.sig.close()
  }

  private onMessage(msg: ServerMsg, joined: boolean) {
    switch (msg.t) {
      case 'welcome':
        // 대면 대화용 방이 아니다(번역이 꺼진 방이거나, 다른 사람이 들어와 있는 회의).
        // 같은 기기의 다른 한쪽(최대 1명) 말고 누가 있으면 회의에 끼어들지 않고 나간다.
        if (!msg.settings.translation) return this.fail('not_talk_room')
        // 대면 대화 연결(같은 계정, 마이크·카메라·화면 모두 끔: 다른 한쪽이나 끊긴 뒤 아직 정리되지 않은 내 옛 연결)만
        // 있어야 한다. 게스트가 있거나 같은 계정이라도 미디어를 켠 연결(회의 탭)이 있으면 진짜 회의다.
        if (msg.peers.some((p) => !p.host || p.media.mic || p.media.cam || p.media.screen)) return this.fail('in_use')
        this.joined = true
        // 새 연결은 막히지 않은 상태로 시작한다. 화면의 지난 상태(다시 연결하기 전)를 지우고, 아직 막혀 있으면 서버가 입장 직후 다시 알린다.
        this.events.onQuota(false)
        this.events.onSpendPaused(false)
        this.events.onJoined(msg.you)
        this.startSpeech()
        break
      case 'notice':
        if (msg.code === 'quota_exceeded') this.setBlocked('quota', true)
        else if (msg.code === 'quota_restored') this.setBlocked('quota', false)
        else if (msg.code === 'spend_limit') this.setBlocked('spend', true)
        else if (msg.code === 'spend_restored') this.setBlocked('spend', false)
        break
      case 'caption':
        this.events.onCaption(msg)
        break
      case 'stt_token':
        this.tokens.get(msg.id)?.resolve({ token: msg.token, model: msg.model })
        this.tokens.delete(msg.id)
        break
      case 'stt_token_denied':
        this.tokens.get(msg.id)?.reject(msg.retry ? 'failed' : 'unavailable')
        this.tokens.delete(msg.id)
        break
      case 'waiting':
        // 자기 방이 아니거나 로그인이 풀렸다. 남의 주최자에게 입장 요청이 남지 않게 바로 끊는다.
        this.fail('forbidden')
        break
      case 'error':
        if (!joined) this.fail(msg.code)
        else console.warn('server error', msg.code, msg.message)
        break
    }
  }

  /** 번역 시간이 떨어지거나 지출 상한으로 멈추면 음성 인식을 멈추고(세션을 열어 둬도 자막이 나오지 않는다), 둘 다 풀리면 다시 시작한다. */
  private setBlocked(kind: 'quota' | 'spend', on: boolean) {
    if ((kind === 'quota' ? this.outOfTime : this.spendPaused) === on || this.closed) return
    if (kind === 'quota') this.outOfTime = on
    else this.spendPaused = on
    if (on) this.stopSpeech()
    else if (this.joined) this.startSpeech()
    if (kind === 'quota') this.events.onQuota(on)
    else this.events.onSpendPaused(on)
  }

  private fail(reason: TalkFailure) {
    this.close()
    this.events.onFailed(reason)
  }

  /** 이 사람 언어로 잠근 음성 인식 토큰. 발급은 비동기라 요청 id로 짝을 맞춘다. */
  requestToken(): Promise<Grant> {
    return new Promise<Grant>((resolve, reject) => {
      if (this.closed) return reject('failed')
      const id = ++this.tokenId
      this.tokens.set(id, { resolve, reject })
      this.sig.send({ t: 'stt_token', id })
      setTimeout(() => {
        if (this.tokens.delete(id)) reject('failed')
      }, TOKEN_TIMEOUT_MS)
    })
  }

  /** 이 사람이 한 말로 서버에 보낸다(구간 번호는 연결 하나 안에서 늘어나기만 하면 건너뛰어도 된다). */
  sendStt(seg: number, text: string, final: boolean) {
    this.sig.send({ t: 'stt', seg, text, final })
  }

  private startSpeech() {
    if (this.speech || !this.track || this.outOfTime || this.spendPaused) return
    this.speech = startSpeech({
      track: this.track,
      lang: this.lang,
      mic: this.active,
      seg: this.seg,
      requestToken: () => this.requestToken(),
      sendStt: (seg, text, final) => this.sendStt(seg, text, final),
      onStatus: (s) => this.events.onSttStatus(s),
      // 상대 목소리가 스피커로 나오지 않는다(같은 자리에서 직접 말한다).
      farEnd: () => [],
      onEcho: () => {},
      // 두 사람 세션을 다 열어 두고 마이크만 돌린다: 차례를 바꿔도 연결 지연이 없다.
      warm: this.warm,
    })
  }

  private stopSpeech() {
    this.speech?.close()
    this.speech = null
    for (const waiter of this.tokens.values()) waiter.reject('failed')
    this.tokens.clear()
    this.events.onSttStatus(null)
  }
}
