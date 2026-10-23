import assert from 'node:assert/strict'
import test from 'node:test'
import { loadModule } from './load-module.mjs'

const { Vad, FRAME_SAMPLES } = loadModule('lib/vad.ts')
const { SttController } = loadModule('lib/stt.ts')

const frame = (amp) => Int16Array.from({ length: FRAME_SAMPLES }, (_, i) => (i % 2 ? amp : -amp))
const silence = () => frame(20)
const speech = () => frame(3000)

test('vad starts after two loud frames with preroll and ends after the hangover', () => {
  const vad = new Vad()
  for (let i = 0; i < 50; i++) assert.equal(vad.push(silence()), null)
  assert.equal(vad.push(speech()), null)
  const start = vad.push(speech())
  assert.equal(start.kind, 'start')
  // 300ms 프리롤 = 15프레임(마지막 두 프레임이 말소리)
  assert.equal(start.frames.length, 15)
  assert.equal(vad.push(speech()).kind, 'speech')
  const results = []
  for (let i = 0; i < 30; i++) results.push(vad.push(silence()).kind)
  assert.deepEqual(results.slice(0, 29), Array(29).fill('speech'))
  assert.equal(results[29], 'end')
  assert.equal(vad.push(silence()), null)
})

test('vad learns steady noise as the floor and ignores short clicks', () => {
  const vad = new Vad()
  for (let i = 0; i < 100; i++) vad.push(silence())
  // 조용하던 방에서 선풍기가 켜짐: 말로 시작되더라도 몇 초 안에 끝나고 다시 시작하지 않는다.
  const kinds = []
  for (let i = 0; i < 500; i++) kinds.push(vad.push(frame(600))?.kind ?? null)
  assert.ok(kinds.filter((k) => k === 'start').length <= 1)
  const end = kinds.indexOf('end')
  assert.ok(end > 0 && end * 20 < 6000, `ended after ${end * 20}ms`)
  assert.ok(kinds.slice(end + 1).every((k) => k === null))
  assert.equal(vad.push(frame(8000)), null, 'a single click is not speech')
  assert.equal(vad.push(frame(600)), null)
  assert.equal(vad.push(frame(8000)), null)
})

test('vad keeps real speech with syllable dips going over a noisy floor', () => {
  const vad = new Vad()
  for (let i = 0; i < 300; i++) vad.push(frame(300))
  const kinds = []
  // 10초 동안 200ms마다 60ms씩 잦아드는 말
  for (let i = 0; i < 500; i++) kinds.push(vad.push(frame(i % 10 < 7 ? 4000 : 350))?.kind ?? null)
  assert.equal(kinds.filter((k) => k === 'start').length, 1)
  assert.ok(!kinds.includes('end'), 'speech is not cut while talking')
})

test('vad does not treat the room noise at join as speech', () => {
  const vad = new Vad()
  const kinds = []
  for (let i = 0; i < 100; i++) kinds.push(vad.push(frame(700))?.kind ?? null)
  assert.ok(kinds.every((k) => k === null))
})

test('vad ignores residual echo while the other side talks, but not my own voice', () => {
  const vad = new Vad()
  for (let i = 0; i < 50; i++) vad.push(silence())
  // 상대가 말하는 동안 마이크에 들어온 잔여 에코(평소라면 말로 시작할 크기)
  const echo = []
  for (let i = 0; i < 20; i++) echo.push(vad.push(frame(400), true)?.kind ?? null)
  assert.ok(echo.every((k) => k === null), 'echo is not sent')
  for (let i = 0; i < 5; i++) vad.push(silence())
  assert.equal(vad.echoBlocked, 1, 'one blocked burst, counted when it ends')
  // 같은 크기라도 상대가 조용하면 내 말이다.
  assert.equal(vad.push(frame(400)), null)
  assert.equal(vad.push(frame(400)).kind, 'start')
  for (let i = 0; i < 40; i++) vad.push(silence())
  // 상대와 동시에 말해도 가까이서 말하는 내 목소리는 잡힌다.
  assert.equal(vad.push(speech(), true), null)
  assert.equal(vad.push(speech(), true).kind, 'start')
})

test('vad ends my turn when only echo keeps the mic busy', () => {
  const vad = new Vad()
  for (let i = 0; i < 50; i++) vad.push(silence())
  vad.push(speech())
  assert.equal(vad.push(speech()).kind, 'start')
  // 내가 말을 멈춘 뒤 상대 목소리의 에코만 들어온다: 에코가 내 구간을 늘이지 않는다.
  const kinds = []
  for (let i = 0; i < 40; i++) kinds.push(vad.push(frame(400), true)?.kind ?? null)
  assert.equal(kinds.indexOf('end'), 29)
})

test('vad does not count my own speech rising through the raised threshold as echo', () => {
  const vad = new Vad()
  for (let i = 0; i < 50; i++) vad.push(silence())
  vad.push(frame(400), true)
  vad.push(frame(400), true)
  vad.push(speech(), true)
  assert.equal(vad.push(speech(), true).kind, 'start')
  assert.equal(vad.echoBlocked, 0)
})

const farEndModule = () =>
  loadModule('lib/farend.ts', {}, { WeakMap, RTCRtpReceiver: class { getSynchronizationSources() {} } })

test('far end is active while a receiver reports speech level, with a hold and stale packets ignored', () => {
  const { FarEnd } = farEndModule()
  const source = { audioLevel: 0, timestamp: 0 }
  const receiver = { getSynchronizationSources: () => [{ ...source }] }
  const far = new FarEnd(() => [receiver])
  assert.equal(far.active(0), false)
  Object.assign(source, { audioLevel: 0.3, timestamp: 1 })
  assert.equal(far.active(100), true)
  Object.assign(source, { audioLevel: 0.001, timestamp: 2 })
  assert.equal(far.active(300), true, 'held right after the other side stops')
  assert.equal(far.active(600), false)
  // 패킷이 끊겨 마지막(큰) 레벨이 남아 있어도 오래되면 무시한다.
  Object.assign(source, { audioLevel: 0.3, timestamp: 3 })
  assert.equal(far.active(1000), true)
  assert.equal(far.active(1200), true)
  assert.equal(far.active(1700), false)
  assert.equal(new FarEnd(() => []).active(0), false)
})

test('far end reads every receiver at most every 100ms and ignores nonstop noise', () => {
  const { FarEnd } = farEndModule()
  const empty = { getSynchronizationSources: () => [] }
  const noLevel = { getSynchronizationSources: () => [{ timestamp: 1 }] }
  let ts = 0
  const noisy = { getSynchronizationSources: () => [{ audioLevel: 0.1, timestamp: ++ts }] }
  let polls = 0
  const far = new FarEnd(() => (polls++, [empty, noLevel, noisy]))
  assert.equal(far.active(0), true)
  far.active(20)
  far.active(40)
  assert.equal(polls, 1, 'not polled on every 20ms frame')
  // 10초 넘게 한 번도 조용해지지 않으면 말이 아니라 잡음이다.
  for (let t = 100; t <= 10_000; t += 100) far.active(t)
  assert.equal(far.active(10_500), false)
})

test('far end stays off when the browser has no synchronization sources API', () => {
  const { FarEnd } = loadModule('lib/farend.ts', {}, { WeakMap })
  const receiver = { getSynchronizationSources: () => { throw new Error('unsupported') } }
  assert.equal(new FarEnd(() => [receiver]).active(0), false)
})

function setup({ lang = 'ko', mic = true, seg = { next: 0 } } = {}) {
  const clock = { t: 0 }
  const sessions = []
  const sent = []
  const statuses = []
  const tokens = []
  const deps = {
    now: () => clock.t,
    requestToken: () =>
      new Promise((resolve, reject) => tokens.push({ resolve, reject })),
    openSession: (grant, events) => {
      const s = { grant, events, log: [], closed: false }
      s.activityStart = () => s.log.push('start')
      s.audio = (pcm) => s.log.push(`audio:${pcm.length}`)
      s.activityEnd = () => s.log.push('end')
      s.close = () => {
        if (s.closed) return
        s.closed = true
        events.onClose()
      }
      sessions.push(s)
      return s
    },
    sendStt: (seg, text, final) => sent.push([seg, text, final]),
    onStatus: (s) => statuses.push(s),
  }
  const c = new SttController(deps, seg, lang)
  c.setMic(mic)
  const grant = async () => {
    tokens.shift().resolve({ token: `t${sessions.length}`, model: 'models/m' })
    await Promise.resolve()
  }
  const ready = (i = sessions.length - 1) => sessions[i].events.onReady()
  const speak = (frames = 10) => {
    c.vad({ kind: 'start', frames: [speech(), speech()] })
    for (let i = 0; i < frames; i++) c.vad({ kind: 'speech', frames: [speech()] })
    c.vad({ kind: 'end', frames: [silence()] })
  }
  const advance = (ms) => {
    for (let left = ms; left > 0; left -= 1000) {
      clock.t += Math.min(1000, left)
      c.tick()
    }
  }
  return { c, clock, sessions, sent, statuses, tokens, grant, ready, speak, advance }
}

test('opens a session up front, streams one utterance in 100ms chunks and forwards transcripts', async () => {
  const h = setup()
  assert.equal(h.tokens.length, 1, 'session is requested before anyone speaks')
  await h.grant()
  h.ready()
  assert.equal(h.statuses.at(-1), 'live')
  h.speak(10)
  // 프레임 13개 = 100ms 조각 2개 + 남은 3프레임
  assert.deepEqual(h.sessions[0].log, ['start', `audio:${FRAME_SAMPLES * 5}`, `audio:${FRAME_SAMPLES * 5}`, `audio:${FRAME_SAMPLES * 3}`, 'end'])
  const ev = h.sessions[0].events
  ev.onInterim('안녕')
  ev.onInterim('안녕')
  ev.onInterim('안녕하세요')
  ev.onFinal('안녕하세요.')
  h.c.vad({ kind: 'start', frames: [speech()] })
  ev.onInterim('다음')
  assert.deepEqual(h.sent, [
    [0, '안녕', false],
    [0, '안녕하세요', false],
    [0, '안녕하세요.', true],
    [1, '다음', false],
  ])
})

test('speech before the session exists is buffered and delivered in order', async () => {
  const h = setup()
  h.speak(4)
  await h.grant()
  assert.deepEqual(h.sessions[0].log, ['start', `audio:${FRAME_SAMPLES * 5}`, 'audio:' + FRAME_SAMPLES * 2, 'end'])
})

test('missing final: the last interim is finalized after a short wait', async () => {
  const seg = { next: 7 }
  const h = setup({ seg })
  await h.grant()
  h.ready()
  h.speak()
  h.sessions[0].events.onInterim('hello there')
  h.advance(1000)
  assert.equal(h.sent.length, 1)
  h.advance(2000)
  assert.deepEqual(h.sent, [[7, 'hello there', false], [7, 'hello there', true]])
  assert.equal(seg.next, 8, 'the counter is shared with later controllers on the same connection')
})

test('keepalive silence every 30s while quiet', async () => {
  const h = setup()
  await h.grant()
  h.ready()
  h.advance(29_000)
  assert.deepEqual(h.sessions[0].log, [])
  h.advance(1000)
  assert.deepEqual(h.sessions[0].log, ['audio:1600'])
  h.advance(30_000)
  assert.deepEqual(h.sessions[0].log, ['audio:1600', 'audio:1600'])
})

test('after 3 minutes of silence a fresh session is opened in the background and swapped in', async () => {
  const h = setup()
  await h.grant()
  h.ready()
  h.advance(179_000)
  assert.equal(h.tokens.length, 0)
  h.advance(1000)
  assert.equal(h.tokens.length, 1)
  await h.grant()
  assert.equal(h.sessions[0].closed, false, 'old session stays until the new one is ready')
  h.ready(1)
  assert.equal(h.sessions[0].closed, true)
  assert.equal(h.statuses.at(-1), 'live')
  assert.ok(!h.statuses.includes('paused'))
  h.speak(4)
  assert.equal(h.sessions[1].log[0], 'start')
  // 교체 직후에는 다시 교체하지 않는다.
  h.advance(60_000)
  assert.equal(h.tokens.length, 0)
})

test('language change rotates the session, but never in the middle of an utterance', async () => {
  const h = setup()
  await h.grant()
  h.ready()
  h.c.vad({ kind: 'start', frames: [speech()] })
  h.c.setLang('ja')
  await h.grant()
  h.ready(1)
  assert.equal(h.sessions[0].closed, false, 'still speaking')
  h.c.vad({ kind: 'end', frames: [] })
  h.c.tick()
  assert.equal(h.sessions[0].closed, false, 'waiting for the final')
  h.sessions[0].events.onFinal('끝.')
  h.c.tick()
  assert.equal(h.sessions[0].closed, true)
  h.speak(1)
  assert.equal(h.sessions[1].log[0], 'start')
})

test('a dropped session is reopened with backoff and shows paused meanwhile', async () => {
  const h = setup()
  await h.grant()
  h.ready()
  h.advance(40_000)
  h.sessions[0].close() // 오래 잘 동작하던 세션: 바로 다시 연다
  assert.equal(h.tokens.length, 1)
  await h.grant()
  h.sessions[1].close() // 준비되기 전에 끊김 → 실패
  assert.equal(h.statuses.at(-1), 'paused')
  assert.equal(h.tokens.length, 0)
  h.advance(1000)
  assert.equal(h.tokens.length, 1)
  h.tokens.shift().reject('failed')
  await Promise.resolve()
  h.advance(2000)
  assert.equal(h.tokens.length, 0, 'second failure waits longer')
  h.advance(1000)
  assert.equal(h.tokens.length, 1)
  await h.grant()
  h.ready()
  assert.equal(h.statuses.at(-1), 'live')
})

test('unavailable stops asking for tokens', async () => {
  const h = setup()
  h.tokens.shift().reject('unavailable')
  await Promise.resolve()
  h.speak()
  h.advance(120_000)
  assert.equal(h.tokens.length, 0)
  assert.equal(h.statuses.at(-1), 'unavailable')
})

test('muting ends the utterance at once; a long mute closes the session and unmuting reopens', async () => {
  const h = setup()
  await h.grant()
  h.ready()
  h.c.vad({ kind: 'start', frames: [speech()] })
  h.c.setMic(false)
  assert.deepEqual(h.sessions[0].log, ['start', `audio:${FRAME_SAMPLES}`, 'end'])
  h.sessions[0].events.onFinal('')
  h.advance(179_000)
  assert.equal(h.sessions[0].closed, false)
  h.advance(1000)
  assert.equal(h.sessions[0].closed, true)
  assert.equal(h.tokens.length, 0, 'no session while muted')
  h.c.setMic(true)
  assert.equal(h.tokens.length, 1)
})

test('dispose ends speech and closes every session', async () => {
  const h = setup()
  await h.grant()
  h.ready()
  h.c.vad({ kind: 'start', frames: [speech()] })
  h.c.dispose()
  assert.equal(h.sessions[0].log.at(-1), 'end')
  assert.equal(h.sessions[0].closed, true)
  h.advance(60_000)
  assert.equal(h.tokens.length, 0)
})

test('a final arriving after the local finalize, or a trailing interim, is not committed twice', async () => {
  const h = setup()
  await h.grant()
  h.ready()
  h.speak()
  const ev = h.sessions[0].events
  ev.onInterim('hello world')
  h.advance(3000)
  ev.onFinal('Hello world.')
  ev.onInterim('world')
  h.advance(5000)
  assert.deepEqual(h.sent, [[0, 'hello world', false], [0, 'hello world', true]])
  h.speak()
  ev.onFinal('Next.')
  ev.onInterim('Next')
  h.advance(5000)
  assert.deepEqual(h.sent.slice(2), [[1, 'Next.', true]])
})

test('when the current session drops, a standby session is promoted without a new token', async () => {
  const h = setup()
  await h.grant()
  h.ready()
  h.sessions[0].events.onGoAway()
  h.c.vad({ kind: 'start', frames: [speech()] })
  await h.grant()
  h.ready(1)
  h.c.tick()
  assert.equal(h.sessions[0].closed, false, 'no swap mid-utterance')
  h.sessions[0].close()
  assert.equal(h.tokens.length, 0)
  assert.deepEqual(h.sessions[1].log, ['start'])
  h.c.vad({ kind: 'end', frames: [] })
  assert.equal(h.sessions[1].log.at(-1), 'end')
})

test('a standby session that is no longer needed is closed', async () => {
  const h = setup()
  await h.grant()
  h.ready()
  h.c.setLang('ja')
  h.c.setLang('ko')
  await h.grant()
  h.c.tick()
  assert.equal(h.sessions[1].closed, true)
  assert.equal(h.sessions[0].closed, false)
  // 반대로 대기 세션의 언어가 또 바뀌면 새로 연다.
  h.c.setLang('ja')
  await h.grant()
  h.c.setLang('en')
  assert.equal(h.sessions[2].closed, true)
  assert.equal(h.tokens.length, 1)
})

test('rotation uses the language that was requested, so a server-side mismatch cannot loop', async () => {
  const h = setup({ lang: 'ko' })
  await h.grant()
  h.ready()
  h.advance(600_000)
  // 교체는 9분 상한 한 번뿐이다.
  assert.equal(h.sessions.length + h.tokens.length, 2)
})

test('dispose mid-utterance commits what was heard and advances the shared counter', async () => {
  const seg = { next: 0 }
  const h = setup({ seg })
  await h.grant()
  h.ready()
  h.c.vad({ kind: 'start', frames: [speech()] })
  h.sessions[0].events.onInterim('we should ship on')
  h.c.dispose()
  assert.deepEqual(h.sent, [[0, 'we should ship on', false], [0, 'we should ship on', true]])
  assert.equal(seg.next, 1)
})

test('paused is not shown while the mic is off and nothing is reconnecting', async () => {
  const h = setup()
  await h.grant()
  h.ready()
  h.c.setMic(false)
  h.sessions[0].close()
  assert.notEqual(h.statuses.at(-1), 'paused')
  h.c.setMic(true)
  assert.equal(h.statuses.at(-1), 'paused')
})

test('a warm controller opens a session with the mic off and keeps one open through idle rotation', async () => {
  const h = setup({ mic: false })
  assert.equal(h.tokens.length, 0, 'cold: nothing opens while the mic is off')
  h.c.setWarm(true)
  assert.equal(h.tokens.length, 1, 'warm: opens right away')
  await h.grant()
  h.ready()
  // 마이크가 꺼진 채 10분: 닫지 않고 교체하며 계속 열어 둔다.
  for (let i = 0; i < 10; i++) {
    h.advance(60_000)
    while (h.tokens.length) {
      await h.grant()
      h.ready()
    }
  }
  assert.ok(h.sessions.some((s) => !s.closed), 'a session is still open')
  assert.ok(h.sessions.filter((s) => !s.closed).length <= 2)
  // 켜자마자 새로 열지 않고 열린 세션으로 바로 보낸다.
  const opened = h.sessions.length
  h.c.setMic(true)
  h.speak(3)
  assert.equal(h.sessions.length, opened)
  const live = h.sessions.filter((s) => !s.closed)
  assert.ok(live.some((s) => s.log.includes('start')), 'speech goes to the already-open session')
})

test('turning warm off closes idle sessions and the next speech reopens without losing audio', async () => {
  const h = setup({ mic: false })
  h.c.setWarm(true)
  await h.grant()
  h.ready()
  h.advance(4 * 60_000)
  while (h.tokens.length) {
    await h.grant()
    h.ready()
  }
  h.c.setWarm(false)
  assert.ok(h.sessions.every((s) => s.closed), 'no idle session is kept once warm is off')
  // 다시 말하면 새로 연다. 그사이 말은 모았다가 보낸다.
  h.c.setMic(true)
  h.speak(3)
  assert.equal(h.tokens.length, 1)
  await h.grant()
  h.ready()
  const s = h.sessions.at(-1)
  assert.equal(s.closed, false)
  assert.ok(s.log[0] === 'start' && s.log.includes('end'), s.log.join(','))
})

test('a slow session connection does not drop the first utterance', async () => {
  const h = setup()
  // 토큰이 3초 동안 오지 않는다: 말은 쌓여 있고 아직 보내지 못했다.
  h.speak(5)
  h.advance(3000)
  assert.equal(h.sent.length, 0)
  await h.grant()
  h.ready()
  const log = h.sessions[0].log
  assert.equal(log[0], 'start', 'the buffered utterance is delivered once connected')
  assert.ok(log.includes('end'))
  h.sessions[0].events.onFinal('늦게 연결돼도 남아요.')
  assert.deepEqual(JSON.parse(JSON.stringify(h.sent)), [[0, '늦게 연결돼도 남아요.', true]])
})

test('a late final for the previous utterance does not drop the next one', async () => {
  const h = setup()
  await h.grant()
  h.ready()
  h.speak(3) // 첫 발화
  h.speak(3) // 둘째 발화(첫 발화 확정이 아직 안 옴)
  h.sessions[0].events.onFinal('첫 문장.')
  h.sessions[0].events.onFinal('둘째 문장.')
  assert.deepEqual(JSON.parse(JSON.stringify(h.sent)), [
    [0, '첫 문장.', true],
    [1, '둘째 문장.', true],
  ])
})

test('a session that takes long to become ready does not drop the utterance sent before it', async () => {
  const h = setup()
  h.speak(4)
  await h.grant() // 세션 객체는 생겼지만 Gemini 준비(setupComplete)가 3초 걸린다.
  h.advance(3000)
  h.ready()
  h.advance(1000) // 준비 뒤 확정까지 1초: 여기서 2초 대기가 새로 시작된다.
  h.sessions[0].events.onFinal('준비가 늦어도 남아요.')
  assert.deepEqual(JSON.parse(JSON.stringify(h.sent)), [[0, '준비가 늦어도 남아요.', true]])
})

test('a long utterance whose final never came does not swallow the next utterance', async () => {
  const h = setup({ lang: 'en' })
  await h.grant()
  h.ready()
  const ev = h.sessions[0].events
  const long = 'x'.repeat(801)
  h.c.vad({ kind: 'start', frames: [speech(), speech()] })
  ev.onInterim(long) // 800자를 넘어 앞부분을 먼저 확정한다
  ev.onInterim(`${long} tail`)
  h.c.vad({ kind: 'end', frames: [silence()] })
  // 첫 발화의 확정이 오지 않은 채 다음 발화가 시작된다.
  h.c.vad({ kind: 'start', frames: [speech(), speech()] })
  ev.onInterim('hello world')
  h.c.vad({ kind: 'end', frames: [silence()] })
  ev.onFinal('hello world.')
  h.advance(4000)
  assert.ok(h.sent.some(([, text, final]) => final && text === 'hello world.'), JSON.stringify(h.sent.map(([s, t, f]) => [s, t.slice(0, 20), f])))
})

test('nonstop speech closes finished sentences so the speaking line stays at most two sentences', async () => {
  const h = setup({ lang: 'en' })
  await h.grant()
  h.ready()
  const ev = h.sessions[0].events
  h.c.vad({ kind: 'start', frames: [speech(), speech()] })
  ev.onInterim('One. Two')
  ev.onInterim('One. Two. Three')
  ev.onInterim('One. Two. Three. Version 3.5 is')
  h.c.vad({ kind: 'end', frames: [silence()] })
  ev.onFinal('One. Two. Three. Version 3.5 is out.')
  assert.deepEqual(JSON.parse(JSON.stringify(h.sent)), [
    [0, 'One. Two', false],
    [0, 'One.', true],
    [1, 'Two. Three', false],
    [1, 'Two.', true],
    [2, 'Three. Version 3.5 is', false],
    [2, 'Three. Version 3.5 is out.', true],
  ])
})
