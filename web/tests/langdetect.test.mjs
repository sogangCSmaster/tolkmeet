import assert from 'node:assert/strict'
import test from 'node:test'
import { loadModule } from './load-module.mjs'

const { detectLang, canAutoDetect } = loadModule('lib/langdetect.ts')
const lang = (text, a, b) => detectLang(text, a, b)?.lang ?? null
const sure = (text, a, b) => detectLang(text, a, b)?.sure ?? false

test('korean and japanese are told apart by script, even with latin words mixed in', () => {
  assert.equal(lang('저희 투자팀은 AI에 집중하고 있습니다', 'ko', 'ja'), 'ko')
  assert.equal(lang('主にAI、それから最近だとディープテック', 'ko', 'ja'), 'ja')
  assert.equal(lang('予算', 'ko', 'ja'), 'ja', 'han alone is japanese in a ko/ja pair')
  assert.ok(sure('はい', 'ja', 'ko'))
})

test('too little text is not decided on an interim', () => {
  assert.equal(detectLang('', 'ko', 'ja'), null)
  assert.equal(detectLang('123 !?', 'ko', 'ja'), null)
  assert.equal(sure('네', 'ko', 'ja'), false)
  assert.equal(lang('네', 'ko', 'ja'), 'ko', 'but a final can use the guess')
  assert.ok(sure('네네', 'ko', 'ja'))
})

test('a korean sentence starting with english words is korean, not english', () => {
  assert.equal(sure('OK', 'ko', 'en'), false, 'two latin letters are not enough')
  assert.equal(lang('OK, 미팅', 'ko', 'en'), 'ko')
  assert.ok(sure('Zoom 미팅', 'ko', 'en'))
  assert.equal(lang('GitHub PR 리뷰 부탁해요', 'ko', 'en'), 'ko')
  assert.equal(sure('Let us', 'ko', 'en'), false)
  assert.equal(lang('We focus on deep tech', 'ko', 'en'), 'en')
  assert.ok(sure('We focus on deep tech', 'ko', 'en'))
})

test('japanese vs chinese waits for kana instead of deciding on a few han characters', () => {
  assert.equal(sure('今日', 'ja', 'zh-CN'), false)
  assert.equal(lang('今日は', 'ja', 'zh-CN'), 'ja')
  assert.ok(sure('今日は', 'ja', 'zh-CN'))
  assert.equal(lang('人工知能に注力しています', 'ja', 'zh-CN'), 'ja')
  assert.ok(sure('我们专注于人工智能', 'ja', 'zh-CN'))
})

test('pairs sharing a script cannot be auto-detected', () => {
  assert.equal(canAutoDetect('ko', 'ja'), true)
  assert.equal(canAutoDetect('ja', 'zh-CN'), true)
  assert.equal(canAutoDetect('en', 'ko'), true)
  assert.equal(canAutoDetect('en', 'es'), false)
  assert.equal(canAutoDetect('fr', 'de'), false)
  assert.equal(canAutoDetect('ko', 'ko'), false)
  assert.equal(detectLang('hello there friends', 'en', 'es'), null)
})

test('mixed korean and japanese text goes to the language with clearly more letters', () => {
  // 실제 전사: 한국식 발음의 일본어 발화에 한글이 섞였다.
  const text = 'よろしくおねがいします。여보세요.私は승지です。本日はお忙しい中、ありがとうございます。'
  assert.equal(lang(text, 'ko', 'ja'), 'ja')
  assert.ok(sure(text, 'ko', 'ja'))
  assert.equal(sure('여보세요 はい', 'ko', 'ja'), false, 'close counts are not sure on an interim')
  assert.ok(sure('네, 그건 寿司가 맞아요', 'ko', 'ja'), 'a few kanji in korean stay korean')
})

test('thai is told apart by its own script from korean, japanese and latin languages', () => {
  assert.equal(canAutoDetect('th', 'ko'), true)
  assert.equal(canAutoDetect('th', 'en'), true)
  assert.equal(lang('สวัสดีครับ ยินดีที่ได้รู้จัก', 'ko', 'th'), 'th')
  assert.equal(sure('สวัสดีครับ', 'th', 'en'), true)
  assert.equal(lang('안녕하세요 반갑습니다', 'th', 'ko'), 'ko')
  // 태국어 문장 속 영어 제품명은 영어로 보지 않는다.
  assert.equal(lang('ใช้ Tolkmeet ประชุมได้เลย', 'en', 'th'), 'th')
})
