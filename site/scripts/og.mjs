#!/usr/bin/env node
// 링크 미리보기 이미지(public/og-{en,ko,ja}.png, 1200×630)를 만든다. 랜딩 문구를 바꾸면 다시 돌린다.
//
//   node site/scripts/og.mjs
//
// 문구는 src/i18n/*.ts에서 읽는다(Node 24가 타입만 지우고 실행한다). 브라우저는 web/의 Playwright를 빌려 쓴다
// (사이트 배포 빌드에 무거운 의존성을 더하지 않으려고). 처음이면 `pnpm --dir web exec playwright install chromium`.
// 구성: 흰 바탕 가운데에 로고(마크+글자), 그 아래 소개 한 줄(metaTitle에서 'Tolkmeet:' 뒤). 장식은 넣지 않는다.
// 메신저가 가운데를 잘라 보여 줘도 다 보이도록 가운데에 모은다.
import { createRequire } from 'node:module'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { en } from '../src/i18n/en.ts'
import { ja } from '../src/i18n/ja.ts'
import { ko } from '../src/i18n/ko.ts'

const SITE = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const { chromium } = createRequire(resolve(SITE, '../web/package.json'))('playwright')

const FONT = 'https://cdn.jsdelivr.net/gh/orioncactus/pretendard@v1.3.9/dist/web/variable/pretendardvariable-jp-dynamic-subset.min.css'
const esc = (s) => s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c])

function html(t) {
  const line = t.metaTitle.split(/[:：]\s*/).slice(1).join(': ')
  return `<!doctype html><html lang="${t.htmlLang}"><head><meta charset="utf-8"><link rel="stylesheet" href="${FONT}">
<style>
  * { box-sizing: border-box; margin: 0; }
  body { display: grid; place-items: center; width: 1200px; height: 630px; overflow: hidden; background: #FFFFFF; color: #1D1D1F;
    font-family: 'Pretendard JP Variable', 'Pretendard JP', sans-serif; -webkit-font-smoothing: antialiased; text-align: center; }
  .brand { display: inline-flex; align-items: center; gap: 22px; font-size: 104px; font-weight: 700; letter-spacing: -0.04em; line-height: 1; }
  .brand svg { width: 112px; height: 112px; }
  p { margin-top: 40px; color: #6E6E73; font-size: 36px; font-weight: 500; letter-spacing: -0.015em; }
</style></head><body><div>
  <div class="brand"><svg viewBox="0 0 96 96"><rect x="8" y="28" width="62" height="16" rx="8" fill="#1D1D1F"/><rect x="26" y="52" width="62" height="16" rx="8" fill="#F2542D"/></svg>tolkmeet</div>
  <p>${esc(line)}</p>
</div></body></html>`
}

const browser = await chromium.launch()
const page = await browser.newPage({ viewport: { width: 1200, height: 630 } })
for (const t of [en, ko, ja]) {
  await page.setContent(html(t), { waitUntil: 'networkidle' })
  await page.evaluate(() => document.fonts.ready)
  const out = resolve(SITE, `public/og-${t.lang}.png`)
  await page.screenshot({ path: out })
  console.log(out)
}
await browser.close()
