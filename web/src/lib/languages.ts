export const LANGUAGES = [
  { code: 'ko', name: '한국어' },
  { code: 'ja', name: '日本語' },
  { code: 'en', name: 'English' },
  { code: 'zh-CN', name: '中文(简体)' },
  { code: 'es', name: 'Español' },
  { code: 'fr', name: 'Français' },
  { code: 'de', name: 'Deutsch' },
  { code: 'de-CH', name: 'Schweizerdeutsch' },
  { code: 'it', name: 'Italiano' },
  { code: 'vi', name: 'Tiếng Việt' },
  { code: 'th', name: 'ไทย' },
] as const

export function languageName(code: string): string {
  return LANGUAGES.find((l) => l.code === code)?.name ?? code
}
