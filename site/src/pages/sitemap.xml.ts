import { LOCALES, SITE_URL } from '../site'

// 언어별 주소와 서로의 hreflang을 담은 사이트맵(페이지가 몇 개뿐이라 플러그인 없이 만든다).
// 한 묶음은 같은 페이지의 언어판들이다: 소개 페이지(영·한·일), 법적 고지(영·한).
const groups: [string, string][][] = [
  LOCALES.map((l) => [l.lang, l.path]),
  ...['privacy', 'terms', 'refund'].map((p): [string, string][] => [['en', `/${p}/`], ['ko', `/ko/${p}/`]]),
]

export function GET() {
  const urls = groups.flatMap((group) => {
    const links = [...group, ['x-default', group[0][1]]]
      .map(([lang, path]) => `    <xhtml:link rel="alternate" hreflang="${lang}" href="${SITE_URL}${path}"/>`)
      .join('\n')
    return group.map(([, path]) => `  <url>\n    <loc>${SITE_URL}${path}</loc>\n${links}\n  </url>`)
  })
  const body = `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">\n${urls.join('\n')}\n</urlset>\n`
  return new Response(body, { headers: { 'Content-Type': 'application/xml' } })
}
