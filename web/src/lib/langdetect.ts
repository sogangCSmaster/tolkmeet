/**
 * 두 언어 중 어느 쪽으로 말했는지를 인식된 글자의 문자 체계로 가른다.
 * 실측(2026-10-17): Gemini Live 전사는 두 언어(내 언어 + 상대 언어)로 잠그면 각 발화를 그 언어 글자로 적고
 * (한국식 발음의 일본어도 가나로), 응답에 언어 정보는 없다. 그래서 글자로 판단한다. 같은 문자 체계끼리(예: 영어·스페인어)는 가를 수 없다.
 */

type Script = 'hangul' | 'kana' | 'han' | 'thai' | 'latin'

/** 언어마다 그 언어임을 보여 주는 문자 체계. 일본어는 가나가 결정적이고 한자도 쓴다. */
const SCRIPTS: Record<string, Script[]> = {
  ko: ['hangul'],
  ja: ['kana', 'han'],
  'zh-CN': ['han'],
  en: ['latin'],
  es: ['latin'],
  fr: ['latin'],
  de: ['latin'],
  vi: ['latin'],
  th: ['thai'],
}

/** 비라틴 문자(한글·가나·한자·태국 문자)는 이만큼이면 그 언어로 확신한다(라틴 문자 언어 화자의 전사에는 거의 나오지 않는다). */
const SURE_NON_LATIN = 2
/** 라틴 문자 언어는 이만큼 쌓이고 상대 언어 글자가 하나도 없어야 확신한다("OK", 제품명으로 시작하는 한국어 문장 대비). */
const SURE_LATIN = 8
/** 일본어와 함께일 때 중국어는 가나 없는 한자가 이만큼이어야 확신한다(일본어 문장도 한자로 시작할 수 있다). */
const SURE_HAN_VS_JA = 6

function scriptOf(cp: number): Script | null {
  if ((cp >= 0xac00 && cp <= 0xd7a3) || (cp >= 0x1100 && cp <= 0x11ff) || (cp >= 0x3130 && cp <= 0x318f)) return 'hangul'
  if ((cp >= 0x3040 && cp <= 0x30ff) || (cp >= 0x31f0 && cp <= 0x31ff) || (cp >= 0xff66 && cp <= 0xff9d)) return 'kana'
  if ((cp >= 0x4e00 && cp <= 0x9fff) || (cp >= 0x3400 && cp <= 0x4dbf)) return 'han'
  if (cp >= 0x0e00 && cp <= 0x0e7f) return 'thai'
  if ((cp >= 0x41 && cp <= 0x5a) || (cp >= 0x61 && cp <= 0x7a) || (cp >= 0xc0 && cp <= 0x24f) || (cp >= 0x1e00 && cp <= 0x1eff)) return 'latin'
  return null
}

/** 두 언어를 글자만으로 가를 수 있는지(문자 체계가 겹치지 않거나, 일본어·중국어처럼 가나로 갈리는지). */
export function canAutoDetect(a: string, b: string): boolean {
  const sa = SCRIPTS[a]
  const sb = SCRIPTS[b]
  if (!sa || !sb || a === b) return false
  // 첫 문자 체계(결정적인 것)가 서로 다르면 된다. ja(가나) ↔ zh(한자)도 가나 유무로 갈린다.
  return sa[0] !== sb[0]
}

export type Detection = { lang: string; sure: boolean }

/**
 * `text`가 `a`와 `b` 중 어느 언어인지. 글자가 없으면 null.
 * `sure`가 false면 짐작이다: 중간 결과로는 정하지 말고 더 기다리고, 확정 결과에서만 쓴다.
 */
export function detectLang(text: string, a: string, b: string): Detection | null {
  const count: Record<Script, number> = { hangul: 0, kana: 0, han: 0, thai: 0, latin: 0 }
  for (const ch of text) {
    const s = scriptOf(ch.codePointAt(0)!)
    if (s) count[s]++
  }
  if (!canAutoDetect(a, b)) return null
  const jaZh = (a === 'ja' && b === 'zh-CN') || (a === 'zh-CN' && b === 'ja')
  // 그 언어 화자만 낼 법한 글자 수(증거).
  const evidence = (lang: string): number => {
    if (lang === 'ko') return count.hangul
    if (lang === 'th') return count.thai
    if (lang === 'ja') return jaZh ? count.kana : count.kana + count.han
    if (lang === 'zh-CN') return jaZh ? (count.kana ? 0 : count.han) : count.han
    return count.latin
  }
  const isLatin = (lang: string) => SCRIPTS[lang]?.[0] === 'latin'
  const sure = (lang: string, other: string): boolean => {
    const e = evidence(lang)
    if (jaZh) return lang === 'ja' ? count.kana > 0 && e + count.han >= SURE_NON_LATIN : e >= SURE_HAN_VS_JA
    if (isLatin(lang)) return e >= SURE_LATIN && evidence(other) === 0
    // 상대가 라틴 문자 언어면 한글·가나·태국 문자 몇 글자로 충분하다(한국어 문장 속 영어 단어는 흔하다).
    if (isLatin(other)) return e >= SURE_NON_LATIN
    // 둘 다 비라틴(한국어·일본어 등)이면 섞여 적히기도 한다(한국식 발음의 일본어 속 "여보세요"):
    // 상대 글자보다 두 배 넘게 많아야 확신한다.
    return e >= SURE_NON_LATIN && e > 2 * evidence(other)
  }
  if (sure(a, b)) return { lang: a, sure: true }
  if (sure(b, a)) return { lang: b, sure: true }
  // 짐작: 증거가 더 많은 쪽. 같으면(글자 없음 포함) 모른다.
  const ea = evidence(a)
  const eb = evidence(b)
  if (ea === eb) return null
  return { lang: ea > eb ? a : b, sure: false }
}
