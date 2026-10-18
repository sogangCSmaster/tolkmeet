import { en, type Dict } from './i18n/en'
import { ja } from './i18n/ja'
import { ko } from './i18n/ko'

export const SITE_URL = 'https://tolkmeet.com'
/** 앱(회의). 로그인·가입·결제는 모두 여기서 한다. 소개 페이지는 쿠키를 쓰지 않는다. */
export const APP_URL = 'https://meet.tolkmeet.com'
export const SIGN_IN_URL = `${APP_URL}/login`
// 가입은 앱의 /signup, 결제는 앱 안의 Paddle 결제창. 이 사이트는 main에 올리면 GitHub Pages로 배포된다(docs/LAUNCH_PLAN.md).
export const SIGNUP_URL = `${APP_URL}/signup`
export const GITHUB_URL = 'https://github.com/sogangCSmaster/tolkmeet'

/** 운영 사업자(약관·처리방침 본문과 같은 값). 전화번호·통신판매업 신고번호는 받으면 더한다. */
export const COMPANY = {
  name: '휘슬리',
  rep: '박혜연',
  regNo: '155-03-03588',
  address: '서울특별시 구로구 구일로4길 65',
  email: 'support@tolkmeet.com',
}

/** 언어 선택에 보이는 순서 */
export const LOCALES: Dict[] = [en, ko, ja]

/** 법적 고지는 영어와 한국어 두 벌. 한국어 페이지에서는 한국어판, 나머지는 영어판으로 건다. */
export function legalLinks(lang: string): { privacy: string; terms: string; refund: string; lang: string } {
  const ko = lang === 'ko'
  const base = ko ? '/ko' : ''
  return { privacy: `${base}/privacy/`, terms: `${base}/terms/`, refund: `${base}/refund/`, lang: ko ? 'ko' : 'en' }
}
