// 소개 페이지 문구(영어가 기준). 키를 더하면 ko·ja에 빠진 키가 타입 오류로 드러난다.
// 화면 구성은 design/v2/source/MainFull.dc.html(한국어)을 따른다.

/** [언어 코드, 문장] */
export type Said = [string, string]
/** 원문(작게)과 번역(크게) 한 쌍 */
export type Pair = { src: Said; tr: Said }
/** 대화 기록 한 줄: 누가(나/상대), 시각, 원문과 번역 */
export type HistoryLine = Pair & { who: 'me' | 'other'; time: string }
/** 개인정보 구역의 한 줄: 나타났다가 흩어져 사라지는 자막 */
export type PrivacyLine = Pair & { who: 'me' | 'other' }
/** 세 가지 자리 카드 */
export type Mode = { eyebrow: string; title1: string; title2: string; body: string }

export const en = {
  lang: 'en',
  htmlLang: 'en',
  path: '/',
  langName: 'English',

  metaTitle: 'Tolkmeet: video meetings with live translated captions',
  metaDescription:
    'Everyone speaks their own language and reads everyone else in theirs. Tolkmeet turns each sentence into captions in the listener’s language as it is spoken. Korean, Japanese, English and more. Guests join from a link.',

  skipToContent: 'Skip to content',
  navLabel: 'Main',
  navHow: 'How it works',
  navModes: 'Ways to meet',
  navPrivacy: 'Privacy',
  navPricing: 'Pricing',
  navFaq: 'FAQ',
  signIn: 'Sign in',
  getStarted: 'Get started',
  languageLabel: 'Language',

  // 예시 대화의 두 사람: 보는 사람(나)과 상대
  meName: 'Alex',
  otherName: 'さくら',
  otherLang: 'ja',

  heroTitle1: 'Speak your own language.',
  heroTitle2: 'Share one conversation.',
  heroLead: 'The moment you speak, captions appear on the other person’s screen in their language. Nine languages, including English, Japanese and Korean.',
  heroSecondary: 'See an example meeting',
  heroPrice: 'Basic from $19 a month, with 10 hours of translated meetings.',
  poweredBy: 'Powered by',

  // 회의 예시(앱의 회의 화면과 같은 모양): 상대 타일, 내 타일, 대화 기록
  demoLabel: 'Example meeting',
  demoAlt: 'Example meeting screen: Sakura asks in Japanese whether Thursday works for next week’s demo, and Alex reads it in English. The newest caption is the largest.',
  historyTitle: 'Conversation',
  historyListening: 'Listening',
  history: [
    { who: 'me', time: '15:02', src: ['en', 'I sent the slides over ahead of time.'], tr: ['ja', '資料は事前にお送りしました。'] },
    { who: 'other', time: '15:03', src: ['ja', 'ありがとうございます。確認しました。'], tr: ['en', 'Thank you. I’ve checked them.'] },
    { who: 'other', time: '15:03', src: ['ja', '来週のデモ、木曜日でも大丈夫ですか？'], tr: ['en', 'Would Thursday work for next week’s demo?'] },
  ] as HistoryLine[],

  statement1: 'Too small to book an interpreter. Too fast for a translation app.',
  statement2: 'The overseas meetings you kept putting off can happen now, each in your own language.',

  featureTitle1: 'As you speak,',
  featureTitle2: 'words appear.',
  featureBody: 'It doesn’t wait for you to finish. The translation follows in lighter text and settles when the sentence ends. Because it reads the conversation so far, even a short reply like “B, please” comes across as you meant it.',
  featureAlt: 'Captions building up as people talk: the newest translation is the largest, and earlier lines get smaller and fainter.',
  featureLines: [
    { src: ['ja', '来週のデモ、木曜日でも大丈夫ですか？'], tr: ['en', 'Would Thursday work for next week’s demo?'] },
    { src: ['en', 'Sure. Should I show plan A or plan B first?'], tr: ['ja', 'はい、大丈夫です。A案とB案、どちらを先に？'] },
    { src: ['ja', 'Bからでお願いします。'], tr: ['en', 'Plan B first, please.'] },
  ] as Pair[],

  // 번역 엔진: 속도(스트리밍)와 품질(맥락). 숫자 대신 "기다리지 않는다"로 말한다.
  engineTitle1: 'Fast,',
  engineTitle2: 'and accurate.',
  engineLead: 'Google’s Gemini starts streaming the translation the moment it hears you. That is a different pace from interpreter apps that wait for the sentence to end.',
  speedEyebrow: 'Streaming translation',
  speedTitle: 'No waiting.',
  speedBody: 'It doesn’t wait for the sentence to end. A draft follows in lighter text while you speak, then settles the moment you stop.',
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
  contextQ: ['en', 'Should I show plan A or plan B first?'] as Said,
  contextA: ['en', 'B first.'] as Said,
  contextTr: ['ja', 'B案からお願いします。'] as Said,

  modesTitle: 'Three ways to meet.',
  modesLead: 'Online, across a desk, or in the middle of a meeting room.',
  modeVideo: { eyebrow: 'Video meeting', title1: 'One link,', title2: 'everyone’s language.', body: 'Up to four people. Whoever gets the link joins from a browser, with no sign-up and nothing to install.' } as Mode,
  modeInPerson: { eyebrow: 'In-person conversation', title1: 'One phone', title2: 'between you.', body: 'Each half of the screen faces one person and shows the other’s words in their language.' } as Mode,
  modeCaptions: { eyebrow: 'Live captions screen', title1: 'One tablet', title2: 'on the table.', body: 'With two languages written in different scripts, it tells which one each person is speaking and shows the translation in large type.' } as Mode,
  // 대면 대화 휴대폰: 위쪽 반은 상대를 향해 뒤집혀 있다
  phoneTop: { src: ['en', 'Yes, Thursday.'], tr: ['ja', 'はい、木曜日に。'] } as Pair,
  phoneBottom: { src: ['ja', 'ありがとうございます。'], tr: ['en', 'Thank you.'] } as Pair,
  phoneButton: 'Tap, then speak',
  // 자막 화면 태블릿: 지난 두 줄은 흐리게, 마지막 줄은 크게
  tabletPast: [
    ['en', 'Thank you for making time today.'],
    ['ja', 'まず弊社のサービスをご紹介します。'],
  ] as Said[],
  tabletNow: { src: ['ja', '予算について確認したいです。'], tr: ['en', 'I’d like to go over the budget.'] } as Pair,

  detailsTitle: 'What matters in a meeting.',
  details: [
    { title: '9 languages', body: 'Korean, Japanese, English, Chinese (Simplified), Spanish, French, German, Vietnamese and Thai. Each person picks their own.' },
    { title: 'No sign-up', body: 'Guests open the link and ask to join. That’s it. No app to install, either.' },
    { title: 'Any accent', body: 'Japanese spoken with a Korean accent is still written down as Japanese.' },
    { title: 'Save as .txt', body: 'Each person can keep the conversation, original and translation, as a file.' },
    { title: 'Screen share', body: 'While you present, a small caption window stays on top of your screen (Chrome, Edge).' },
    { title: 'Open source', body: 'The code is public under AGPL-3.0, so you can check it yourself.' },
  ],

  // 개인정보: 자막이 나타났다가 흩어져 사라진다(저장하지 않음). 두 사람 사이엔 암호화된 신호만 오간다.
  privacyTitle1: 'What’s said in the meeting',
  privacyTitle2: 'stays in the meeting.',
  privacyLead: 'Video and audio are encrypted between participants. What you say is sent to Google Gemini only to be translated. We store neither audio nor captions.',
  privacyStageLabel: 'Captions appear and scatter away, while only encrypted signals travel between the two participants.',
  privacyLines: [
    { who: 'other', src: ['ja', '来週のデモ、木曜日でも大丈夫ですか？'], tr: ['en', 'Would Thursday work for next week’s demo?'] },
    { who: 'me', src: ['en', 'Sure. I’ll send the slides ahead.'], tr: ['ja', 'はい、大丈夫です。資料は事前にお送りします。'] },
    { who: 'other', src: ['ja', '価格の条件も確認させてください。'], tr: ['en', 'I’d also like to go over the pricing terms.'] },
  ] as PrivacyLine[],
  privacyWire: 'Encrypted between participants',
  privacyChips: [
    ['', '0', ' seconds recorded'],
    ['', '0', ' caption lines left on our servers'],
    ['Saved only ', 'on your own device', ''],
  ] as [string, string, string][],
  privacyPause: 'Pause animation',
  privacyLink: 'Privacy Policy',

  pricingTitle: 'Pricing.',
  pricingLead: 'Choose by the hours of translated meetings you need each month. Guests are always free.',
  plans: [
    { name: 'Basic', price: '$19', unit: ' /month', note: '10 hours of translated meetings every month · Cancel anytime', cta: 'Start with Basic' },
    { name: 'Pro', price: '$39', unit: ' /month', note: '30 hours of translated meetings every month · Cancel anytime', cta: 'Start with Pro' },
  ],
  pricingFine: 'Every plan: all three ways to meet, nine languages and saved captions. Add hours at $3 each in a busy month. US dollars, plus tax where applicable.',

  faqTitle: 'Questions.',
  faq: [
    { q: 'Do guests need an account or an app?', a: 'No. They open the link and ask to join. Only the person who creates meetings needs an account.' },
    { q: 'How many people can join a meeting?', a: 'Up to four. Each person can speak a different language.' },
    { q: 'Can it replace a professional interpreter?', a: 'For everyday business conversation it works well. For contracts, medical or legal decisions, confirm the important points with a qualified interpreter.' },
    { q: 'What happens to my voice?', a: 'Your speech is sent to Google Gemini through its paid API to be turned into text and translated. Google does not use paid API data to train its models, and Tolkmeet does not store audio or captions.' },
    { q: 'What if I use up my hours?', a: 'Add hours for $3 each. Extra hours stay on your account until you use them. Or move to Pro for 30 hours a month.' },
  ],

  closingTitle1: 'Your next meeting,',
  closingTitle2: 'in everyone’s language.',

  footerDisclaimer: 'Translations are made by AI and can be wrong. Confirm anything important, such as contracts or medical and legal matters, with a professional. Meeting audio is sent to Google Gemini’s paid API to be translated; Tolkmeet doesn’t store it. Prices are in US dollars; tax may be added at checkout.',
  footerGithub: 'View the code on GitHub',
  footerProduct: 'Product',
  footerSupport: 'Support',
  footerContact: 'Contact us',
  footerCompany: 'Company',
  footerSecurity: 'Security',
  footerLegal: 'Legal',
  legalPrivacy: 'Privacy Policy',
  legalTerms: 'Terms of Service',
  legalRefund: 'Refund Policy',
  footerRights: 'All rights reserved.',
  /** 사업자 정보 줄(전자상거래법상 첫 화면 표시). 값은 site.ts의 COMPANY. */
  biz: { name: 'Operator', rep: 'Representative', regNo: 'Business registration no.', address: 'Address', email: 'Contact', country: ', Republic of Korea' },
}

export type Dict = typeof en
