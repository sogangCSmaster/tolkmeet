import type { Meeting } from './api'
import { formatDate, formatTime, zoneName } from './format'
import type { Locale, MessageKey } from './i18n'
import { languageName } from './languages'

/** 회의 종류에 맞는 주소(대면 대화·자막 화면은 주최자 기기에서 연다). */
export function meetingPath(m: Pick<Meeting, 'code' | 'kind'>): string {
  return m.kind === 'inPerson' ? `/talk/${m.code}` : m.kind === 'live' ? `/live/${m.code}` : `/${m.code}`
}

export function meetingUrl(m: Pick<Meeting, 'code'>): string {
  return `${location.origin}/${m.code}`
}

/** 다른 사람에게 보낼 초대 글(메일·메신저에 그대로 붙여 넣는다). */
export function inviteText(m: Meeting, t: (key: MessageKey, p?: Record<string, string>) => string, locale: Locale): string {
  const start = m.startsAt ?? Math.floor(Date.now() / 1000)
  const zone = zoneName(start, locale)
  const when = `${formatDate(start, locale, true)} ${formatTime(start, locale)} – ${formatTime(start + m.durationMin * 60, locale)}${zone ? ` (${zone})` : ''}`
  const langs = m.langA && m.langB ? `${languageName(m.langA)} ⇄ ${languageName(m.langB)}` : ''
  return t('invite.text', { title: m.title, when, url: meetingUrl(m), langs })
}

