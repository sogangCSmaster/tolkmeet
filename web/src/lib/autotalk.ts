import type { CaptionMsg } from './captions'
import { detectLang } from './langdetect'
import { startSpeech, type Speech } from './speech'
import type { SegCounter, SttStatus } from './stt'
import { TalkSide, type TalkFailure } from './talk'

/**
 * 자막 화면: 기기 하나의 마이크로 두 언어를 자동으로 가려 번역 자막을 띄운다.
 * 두 언어 = 같은 방의 두 참가자(연결 둘). 음성 인식 세션은 하나만 쓰고(토큰은 두 언어로 잠겨 각 발화가 그 언어
 * 글자로 적힌다는 실측), 발화마다 글자로 언어를 판단해 그 언어 참가자의 연결로 보낸다.
 */
export type AutoTalkEvents = {
  onJoined: (side: 'a' | 'b', selfId: string) => void
  /** 첫 번째 언어 참가자 연결로 받은 자막. 양쪽 발화가 모두 원문 + 다른 언어 번역으로 들어 있다. */
  onCaption: (msg: CaptionMsg) => void
  onSttStatus: (status: SttStatus | null) => void
  /** 주최자의 번역 시간이 떨어졌다(true) / 다시 생겼다(false). 그동안 듣지 않는다. */
  onQuota: (outOfTime: boolean) => void
  onFailed: (reason: TalkFailure) => void
}

type Side = 'a' | 'b'

export class AutoTalk {
  private sides: Record<Side, TalkSide>
  private langs: Record<Side, string>
  private speech: Speech | null = null
  private track: MediaStreamTrack
  private events: AutoTalkEvents
  private joined = { a: false, b: false }
  private listening = true
  private closed = false
  /** 연결마다 따로 받은 번역 시간 상태. 어느 한쪽이라도 막혔으면 듣지 않는다. */
  private blocked = { a: false, b: false }
  private outOfTime = false
  // 두 연결이 구간 번호를 나눠 쓴다. 각 연결에서는 늘어나기만 하면 된다(건너뛰어도 된다).
  private seg: SegCounter = { next: 0 }
  /** 진행 중인 발화(구간)를 지금 어느 쪽으로 보내고 있는지. 확실한 판단이 바뀌면 옮긴다. */
  private routes = new Map<number, Side>()
  private last: Side = 'a'

  constructor(code: string, a: string, b: string, track: MediaStreamTrack, events: AutoTalkEvents) {
    this.langs = { a, b }
    this.track = track
    this.events = events
    const side = (s: Side) =>
      new TalkSide(code, this.langs[s], this.langs[s === 'a' ? 'b' : 'a'], null, {
        onJoined: (id) => {
          this.joined[s] = true
          events.onJoined(s, id)
          if (this.joined.a && this.joined.b) this.startSpeech()
        },
        onCaption: (msg) => {
          if (s === 'a') events.onCaption(msg)
        },
        onSttStatus: () => {},
        // 두 연결이 같은 알림을 따로 받는다. 연결마다 기억해 두고 합쳐서 판단한다
        // (뒤에 들어온 쪽의 입장 직후 상태(false)가 먼저 막힌 쪽을 풀지 않게).
        onQuota: (out) => this.setOutOfTime(s, out),
        onFailed: (reason) => {
          if (this.closed) return
          this.close()
          events.onFailed(reason)
        },
      })
    this.sides = { a: side('a'), b: side('b') }
  }

  /** 듣기를 멈추거나 다시 듣는다. 멈춘 동안은 세션을 미리 열어 두지 않는다(3분 뒤 닫힘, 다시 들으면 새로 연다). */
  setListening(on: boolean) {
    this.listening = on
    this.speech?.setMic(on)
    this.speech?.setWarm(on)
  }

  setLangs(a: string, b: string) {
    this.langs = { a, b }
    // 각 쪽의 언어가 바뀌었으므로 "바로 앞 발화의 쪽"은 더 이상 같은 언어를 뜻하지 않는다.
    this.last = 'a'
    this.sides.a.setLangs(a, b)
    this.sides.b.setLangs(b, a)
    // 토큰은 첫 번째 쪽 연결(내 언어 a + 상대 언어 b)로 받아 두 언어로 잠긴다. 바뀌면 세션을 새로 연다.
    this.speech?.setLang(`${a}|${b}`)
  }

  close() {
    if (this.closed) return
    this.closed = true
    this.speech?.close()
    this.speech = null
    this.sides.a.close()
    this.sides.b.close()
    this.events.onSttStatus(null)
  }

  private setOutOfTime(side: Side, blocked: boolean) {
    if (this.closed) return
    this.blocked[side] = blocked
    const out = this.blocked.a || this.blocked.b
    if (this.outOfTime !== out) {
      this.outOfTime = out
      if (out) {
        this.speech?.close()
        this.speech = null
        this.events.onSttStatus(null)
      } else if (this.joined.a && this.joined.b) this.startSpeech()
    }
    // 같은 값이어도 알린다: 새로 연결한 쪽의 처음 상태(false)로 화면의 지난 상태를 지운다.
    this.events.onQuota(out)
  }

  private startSpeech() {
    if (this.speech || this.closed || this.outOfTime) return
    this.speech = startSpeech({
      track: this.track,
      lang: `${this.langs.a}|${this.langs.b}`,
      mic: this.listening,
      seg: this.seg,
      requestToken: () => this.sides.a.requestToken(),
      sendStt: (seg, text, final) => this.route(seg, text, final),
      onStatus: (s) => this.events.onSttStatus(s),
      // 스피커로 나오는 상대 목소리가 없다(모두 이 마이크 앞에서 직접 말한다).
      farEnd: () => [],
      onEcho: () => {},
      warm: this.listening,
    })
  }

  /**
   * 발화 하나를 그 언어 참가자의 연결로 보낸다. 중간 결과는 언어가 확실해질 때까지 보내지 않고,
   * 확실한 판단이 바뀌면 옮긴다(옛 쪽에는 빈 중간 결과). 확정 결과로 다시 판단해 마지막으로 보낸 쪽과 다르면,
   * 그쪽에는 빈 확정(그 발화의 중간 결과만 지운다)을 보내고 맞는 쪽으로 보낸다.
   */
  private route(seg: number, text: string, final: boolean) {
    const routed = this.routes.get(seg)
    const found = detectLang(text, this.langs.a, this.langs.b)
    const sideOf = (lang: string): Side => (lang === this.langs.a ? 'a' : 'b')
    if (!final) {
      // 확실한 판단이 바뀌면(예: 제품명 "Google Cloud"로 시작한 한국어 문장) 그 자리에서 옮긴다.
      // 옛 쪽에는 빈 중간 결과를 보내 이 발화의 글자만 지운다(이미 확정된 앞 문장은 그대로).
      const side = found?.sure ? sideOf(found.lang) : routed
      if (!side) return
      if (routed && routed !== side) this.sides[routed].sendStt(seg, '', false)
      this.routes.set(seg, side)
      this.sides[side].sendStt(seg, text, false)
      return
    }
    this.routes.delete(seg)
    // 확정에서는 짐작도 쓴다. 그래도 모르면(짧은 "네", 숫자만) 이미 보낸 쪽, 없으면 바로 앞 발화와 같은 쪽.
    const side = found && (found.sure || !routed) ? sideOf(found.lang) : (routed ?? this.last)
    if (routed && routed !== side) this.sides[routed].sendStt(seg, '', true)
    this.last = side
    this.sides[side].sendStt(seg, text, true)
  }
}
