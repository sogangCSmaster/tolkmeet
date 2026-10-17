import assert from 'node:assert/strict'
import test from 'node:test'
import { loadModule } from './load-module.mjs'

function autoTalk(a = 'ko', b = 'en', { join = true } = {}) {
  const sides = []
  let speech
  let started = 0
  const quota = []
  class TalkSide {
    constructor(code, lang, partner, track, events) {
      this.lang = lang
      this.sent = []
      this.events = events
      sides.push(this)
    }
    sendStt(seg, text, final) { this.sent.push([seg, text, final]) }
    requestToken() { return new Promise(() => {}) }
    setLangs() {}
    close() {}
  }
  const startSpeech = (o) => {
    started++
    speech = { o, mic: o.mic, warm: o.warm, setMic(on) { this.mic = on }, setWarm(on) { this.warm = on }, setLang() {}, close() {} }
    return speech
  }
  const detect = loadModule('lib/langdetect.ts')
  const { AutoTalk } = loadModule('lib/autotalk.ts', { './talk': { TalkSide }, './speech': { startSpeech }, './langdetect': detect }, { Map })
  const events = { onJoined() {}, onCaption() {}, onSttStatus() {}, onQuota: (out) => quota.push(out), onFailed() {} }
  const talk = new AutoTalk('c', a, b, {}, events)
  if (join) {
    sides[0].events.onJoined('A')
    sides[1].events.onJoined('B')
  }
  return { talk, a: sides[0], b: sides[1], send: (...args) => speech.o.sendStt(...args), speech: () => speech, started: () => started, quota }
}

test('auto talk holds unclear interims and sends each utterance to its language', () => {
  const t = autoTalk('ko', 'en')
  t.send(0, 'OK', false)
  t.send(0, 'OK, 오늘', false)
  t.send(0, 'OK, 오늘 미팅 시작할게요', true)
  t.send(1, 'We', false)
  t.send(1, 'We focus on deep tech', true)
  assert.deepEqual(JSON.parse(JSON.stringify(t.a.sent)), [[0, 'OK, 오늘', false], [0, 'OK, 오늘 미팅 시작할게요', true]])
  assert.deepEqual(JSON.parse(JSON.stringify(t.b.sent)), [[1, 'We focus on deep tech', true]])
})

test('a final in the other language retracts what was sent to the wrong side', () => {
  const t = autoTalk('ja', 'zh-CN')
  t.send(0, '我们专注于人工智能', false) // 한자 9자: 중국어로 확신
  t.send(0, '我们专注于人工智能です', true) // 확정에 가나: 일본어
  assert.deepEqual(JSON.parse(JSON.stringify(t.b.sent)), [[0, '我们专注于人工智能', false], [0, '', true]])
  assert.deepEqual(JSON.parse(JSON.stringify(t.a.sent)), [[0, '我们专注于人工智能です', true]])
})

test('pausing stops listening and lets the warm session close', () => {
  const t = autoTalk('ko', 'ja')
  assert.equal(t.speech().mic, true)
  t.talk.setListening(false)
  assert.equal(t.speech().mic, false)
  assert.equal(t.speech().warm, false)
})

test('a sure interim in the other language moves the utterance mid-way', () => {
  const t = autoTalk('ko', 'en')
  t.send(0, 'Google Cloud', false) // 라틴 11자, 한글 없음: 영어로 확신
  t.send(0, 'Google Cloud에서 배포한', false) // 한글: 한국어로 확신 → 옮김
  t.send(0, 'Google Cloud에서 배포한 서비스입니다', true)
  assert.deepEqual(JSON.parse(JSON.stringify(t.b.sent)), [[0, 'Google Cloud', false], [0, '', false]])
  assert.deepEqual(JSON.parse(JSON.stringify(t.a.sent)), [
    [0, 'Google Cloud에서 배포한', false],
    [0, 'Google Cloud에서 배포한 서비스입니다', true],
  ])
})

test('switching back within one utterance keeps one seg and an unsure final follows the latest route', () => {
  const t = autoTalk('ko', 'en')
  t.send(0, 'Google Cloud', false) // 영어로 확신
  t.send(0, 'Google Cloud에서', false) // 한국어로 옮김
  t.send(0, 'Google Cloud에서 네', true) // 확정도 한국어
  t.send(1, 'Deploy pipeline', false) // 영어로 확신
  t.send(1, 'OK', true) // 확정은 짐작도 어려움(라틴 2자) → 마지막으로 보낸 영어 쪽
  assert.deepEqual(JSON.parse(JSON.stringify(t.b.sent)), [[0, 'Google Cloud', false], [0, '', false], [1, 'Deploy pipeline', false], [1, 'OK', true]])
  assert.deepEqual(JSON.parse(JSON.stringify(t.a.sent)), [[0, 'Google Cloud에서', false], [0, 'Google Cloud에서 네', true]])
})

test('the second connection joining does not lift the out-of-time state the first one reported', () => {
  const t = autoTalk('ko', 'en', { join: false })
  // TalkSide 입장 순서: 처음 상태(false) → 입장 → 서버가 바로 막혔다고 알림
  t.a.events.onQuota(false)
  t.a.events.onJoined('A')
  t.a.events.onQuota(true)
  t.b.events.onQuota(false)
  t.b.events.onJoined('B')
  assert.equal(t.started(), 0) // 토큰을 받으려고 음성 인식을 열지 않는다
  assert.equal(t.quota.at(-1), true)
  t.b.events.onQuota(true)
  t.a.events.onQuota(false) // 한쪽만 풀렸다
  assert.equal(t.started(), 0)
  t.b.events.onQuota(false) // 둘 다 풀렸다
  assert.equal(t.started(), 1)
  assert.equal(t.quota.at(-1), false)
})
