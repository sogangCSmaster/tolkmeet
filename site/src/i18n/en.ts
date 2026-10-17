// 소개 페이지 문구(영어가 기준). 키를 더하면 ko·ja에 빠진 키가 타입 오류로 드러난다.

/** 스크롤 장면의 한 발화: 누가(상대/나), 말한 언어, 원문, 번역 중간 결과들, 확정 번역과 그 언어 */
export type Utterance = { who: 'other' | 'me'; lang: string; text: string; drafts: string[]; tr: string; trLang: string }
export type FaceName = 'sakura' | 'minjun'

/** 개인정보 구역의 한 줄: 나타났다가 흩어져 사라지는 자막 */
export type PrivacyLine = { who: 'other' | 'me'; lang: string; text: string; tr: string; trLang: string }

export const en = {
  lang: 'en',
  htmlLang: 'en',
  path: '/',
  langName: 'English',

  metaTitle: 'Tolkmeet: video meetings with live translated captions',
  metaDescription:
    'Everyone speaks their own language and reads everyone else in theirs. Tolkmeet turns each sentence into captions in the listener’s language as it is spoken. Korean, Japanese, English and more. Guests join from a link.',

  navStory: 'How it works',
  navModes: 'Ways to meet',
  navPricing: 'Pricing',
  navFaq: 'FAQ',
  navLabel: 'Main',
  signIn: 'Sign in',
  getStarted: 'Get started',
  languageLabel: 'Language',
  themeLabel: 'Theme',
  themeSystem: 'System',
  themeLight: 'Light',
  themeDark: 'Dark',
  skipToContent: 'Skip to content',

  // 제목은 로고처럼 두 줄: 둘째 줄이 비켜 서고 앞에 주황 막대가 붙는다.
  heroLine1: 'You speak English.',
  heroLine2: 'They read it in Japanese.',
  heroLead:
    'Live translated captions for video meetings, in Korean, Japanese, English and five more languages. Everyone speaks their own language and reads the others in theirs.',
  heroPrice: 'Basic $19 a month, 10 hours of translated meetings included',
  heroSecondary: 'See an example meeting',
  // 기능 — 결과: 로고처럼 둘째 줄이 비켜 선다
  heroPoints: [
    ['Send a link', 'Guests join without an account'],
    ['Speak your own language', 'Captions show up in theirs'],
    ['One phone on the table', 'Works face to face, too'],
  ] as [string, string][],

  // 문제 제기: 스크롤하면 낱말이 하나씩 진해진다. 띄어쓰기로 나누고, 띄어 쓰지 않는 언어는 | 로 나눈다.
  statement: 'Too small to book an interpreter. Too fast for a translation app. The overseas meetings you kept putting off can happen today, each in your own language.',

  // 스크롤 장면: 보는 사람(나)과 상대. 상대가 왼쪽, 내가 오른쪽(실제 회의 화면과 같다).
  meName: 'Alex',
  meFace: 'minjun' as FaceName,
  otherName: 'さくら',
  otherNameLang: 'ja',
  otherFace: 'sakura' as FaceName,
  you: '{name} (you)',
  panelTitle: 'Captions',
  panelLangs: [
    ['en', 'English'],
    ['ja', '日本語'],
  ] as [string, string][],
  translating: 'Translating…',
  storyLabel: 'Example meeting: Alex speaks English and Sakura speaks Japanese. Each reads the other in their own language.',
  story: [
    { who: 'other', lang: 'ja', text: '来週のデモ、木曜日でも大丈夫ですか？', drafts: ['Next week', 'Next week’s demo'], tr: 'Would Thursday work for next week’s demo?', trLang: 'en' },
    { who: 'me', lang: 'en', text: 'Thursday works. Should I start with plan A or plan B?', drafts: ['木曜日', '木曜日で大丈夫です。'], tr: '木曜日で大丈夫です。A案とB案、どちらから始めましょうか？', trLang: 'ja' },
    { who: 'other', lang: 'ja', text: 'Bからでお願いします。', drafts: ['B'], tr: 'Let’s start with plan B, please.', trLang: 'en' },
  ] as Utterance[],
  storySteps: [
    { title: 'Words appear while you talk', body: 'Nobody waits for you to finish. Each sentence is written down as you say it.' },
    { title: 'The translation follows right behind', body: 'A first draft shows in gray while the sentence is still going, then settles once it ends.' },
    { title: 'Short replies still make sense', body: 'Each sentence is translated with the conversation before it, so “B, please” comes out as plan B.' },
  ],

  modesTitle: 'Three ways to meet',
  modes: [
    { title: 'Video meeting', body: 'Up to four people on a call, each in their own language. A browser on a computer or phone is all it takes.' },
    { title: 'In-person conversation', body: 'Two people, one phone between them. Each half of the screen faces one person and shows the other’s words in their language.' },
    { title: 'Live captions screen', body: 'Put a tablet on the table. It hears the room, tells which language each sentence is in, and shows the translation in large type.' },
  ],

  detailsTitle: 'The details that matter in a meeting',
  details: [
    { title: 'Accents are fine', body: 'Japanese spoken with a Korean accent is still written as Japanese, not as Korean sounds.' },
    { title: 'Guests need nothing', body: 'They open the link in Chrome, Safari or Edge and ask to join. You let them in.' },
    { title: 'Screen sharing', body: 'While you present, a small caption window stays on top of your screen (Chrome and Edge).' },
    { title: 'Keep the conversation', body: 'Anyone can save their own captions, original and translation, as a text file.' },
    { title: 'Eight languages', body: 'Korean, Japanese, English, Chinese (Simplified), Spanish, French, German and Vietnamese. Each person picks their own.' },
    { title: 'Open source', body: 'The code is public under AGPL-3.0, so you can check exactly what it does.' },
  ],

  pricingTitle: 'Pricing',
  pricingLead: 'Plans are set by hours of translated meetings each month. Guests are always free.',
  plans: [
    { name: 'Basic', price: '$19', unit: '/month', hours: '10 hours of translated meetings a month', note: 'Monthly subscription. Cancel anytime.', cta: 'Start with Basic' },
    { name: 'Pro', price: '$39', unit: '/month', hours: '30 hours of translated meetings a month', note: 'Monthly subscription. Cancel anytime.', cta: 'Start with Pro' },
  ],
  includesTitle: 'Every plan includes',
  includes: ['Video meetings, in-person conversations and the live captions screen', 'All eight languages', 'Guests join free, without an account', 'Captions you can save as a text file'],
  pricingExtra: 'Need more in a busy month? Add hours for $3 each.',
  pricingFine: 'Prices in US dollars. Tax is added at checkout where it applies.',

  faqTitle: 'Questions',
  faq: [
    { q: 'Do guests need an account or an app?', a: 'No. Guests open the meeting link in a browser and ask to join. Only the person who creates meetings needs an account.' },
    { q: 'How many people can join a meeting?', a: 'Up to four. Each person can speak a different language.' },
    { q: 'Can it replace a professional interpreter?', a: 'For everyday business conversation it works well. For contracts, medical or legal decisions, confirm the important points with a qualified interpreter.' },
    { q: 'What happens to my voice?', a: 'Your speech is sent to Google Gemini through its paid API to be turned into text and translated. Google does not use paid API data to train its models, and Tolkmeet does not store audio or captions.' },
    { q: 'Does the live captions screen work with any two languages?', a: 'It tells languages apart by their writing system, so it works for pairs like Korean and Japanese or Japanese and English. For two languages that share an alphabet, such as English and Spanish, use an in-person conversation instead.' },
    { q: 'What if I use up my hours?', a: 'Add hours for $3 each. Extra hours stay on your account until you use them. Or move to Pro for 30 hours a month.' },
  ],

  closingLine1: 'Your next meeting,',
  closingLine2: 'in everyone’s language.',

  navPrivacy: 'Privacy',
  poweredBy: 'Powered by',

  // 번역 엔진: 속도(스트리밍)와 품질(맥락). 숫자 대신 "기다리지 않는다"로 말한다.
  engineTitle1: 'Fast,',
  engineTitle2: 'and accurate.',
  engineLead: 'Google’s Gemini starts streaming the translation the moment it hears you. That is a different pace from interpreter apps that wait for the sentence to end.',
  speedEyebrow: 'Streaming translation',
  speedTitle: 'No waiting.',
  speedBody: 'It doesn’t wait for the sentence to end. A draft follows in gray while you speak, then settles the moment you stop.',
  speedSrcLang: 'ja',
  speedRows: [
    ['Starts speaking', '来週のデモ、木曜日でも…'],
    ['Still speaking', 'Next week’s demo, Thursday…'],
    ['Done speaking', 'Would Thursday work for next week’s demo?'],
  ] as [string, string][],
  contextEyebrow: 'Context-aware',
  contextTitle1: 'It translates the conversation,',
  contextTitle2: 'not just the sentence.',
  contextBody: 'It reads what was said before, so short replies, politeness and names stay consistent for the whole meeting.',
  contextQ: ['en', 'Should I start with plan A or plan B?'] as [string, string],
  contextA: ['ja', 'Bからで。'] as [string, string],
  contextTr: ['en', 'Plan B first, please.'] as [string, string],

  // 개인정보: 자막이 나타났다가 흩어져 사라진다(저장하지 않음). 두 사람 사이엔 암호화된 신호만 오간다.
  privacyTitle1: 'What’s said in the meeting',
  privacyTitle2: 'stays in the meeting.',
  privacyLead: 'Video and audio are encrypted between participants. What you say is sent to Google Gemini only to be translated. We store neither audio nor captions.',
  privacyStageLabel: 'Captions appear and scatter away, while only encrypted signals travel between the two participants.',
  privacyLines: [
    { who: 'other', lang: 'ja', text: '来週のデモ、木曜日でも大丈夫ですか？', tr: 'Would Thursday work for next week’s demo?', trLang: 'en' },
    { who: 'me', lang: 'en', text: 'Sure. I’ll send the slides ahead.', tr: 'はい、大丈夫です。資料は事前にお送りします。', trLang: 'ja' },
    { who: 'other', lang: 'ja', text: '価格の条件も確認させてください。', tr: 'I’d also like to go over the pricing terms.', trLang: 'en' },
  ] as PrivacyLine[],
  privacyWire: 'Encrypted between participants',
  privacyChips: [
    ['', '0', ' seconds recorded'],
    ['', '0', ' captions left on our servers'],
    ['Saved only ', 'on your own device', ''],
  ] as [string, string, string][],
  privacyPause: 'Pause animation',
  privacyPlay: 'Play animation',
  privacyLink: 'Privacy Policy',

  footerLegal: 'Legal',
  legalPrivacy: 'Privacy Policy',
  legalTerms: 'Terms of Service',
  legalRefund: 'Refund Policy',
  footerDisclaimer: 'Translations are made by AI and can be wrong. Confirm anything important, such as contracts or medical and legal matters, with a professional. Meeting audio is sent to Google Gemini’s paid API to be translated; Tolkmeet doesn’t store it. Prices are in US dollars; tax may be added at checkout.',

  footerProduct: 'Product',
  footerOpenSource: 'Open source under AGPL-3.0',
  footerGithub: 'Source code on GitHub',
  footerCopyright: '© 2026 Tolkmeet',
  /** 사업자 정보 줄(전자상거래법상 첫 화면 표시). 값은 site.ts의 COMPANY. */
  biz: { name: 'Operator', rep: 'Representative', regNo: 'Business registration no.', address: 'Address', email: 'Contact', country: ', Republic of Korea' },
}

export type Dict = typeof en
