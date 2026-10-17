import type { Dict } from './en'

// 소개 페이지의 한국어는 합니다체로 통일한다(유료 업무용 서비스의 소개). 앱 안의 버튼·안내는 해요체 그대로.
export const ko: Dict = {
  lang: 'ko',
  htmlLang: 'ko',
  path: '/ko/',
  langName: '한국어',

  metaTitle: 'Tolkmeet: 실시간 번역 자막이 나오는 화상 회의',
  metaDescription:
    '한국어로 말하면 상대는 일본어로 읽습니다. 말하는 문장마다 듣는 사람의 언어로 자막이 바로 나오는 화상 회의입니다. 한국어·일본어·영어 등 8개 언어를 지원하며, 게스트는 가입 없이 링크로 참여합니다.',

  navStory: '사용 장면',
  navModes: '세 가지 자리',
  navPricing: '요금',
  navFaq: '자주 묻는 질문',
  navLabel: '주요 메뉴',
  signIn: '로그인',
  getStarted: '시작하기',
  languageLabel: '언어',
  themeLabel: '테마',
  themeSystem: '시스템',
  themeLight: '라이트',
  themeDark: '다크',
  skipToContent: '본문으로 건너뛰기',

  heroLine1: '한국어로 말하면,',
  heroLine2: '상대는 일본어로 읽습니다.',
  heroLead:
    '말하는 문장마다 듣는 사람의 언어로 자막이 바로 나오는 화상 회의입니다. 한국어·일본어·영어를 포함한 8개 언어를 지원합니다.',
  heroPrice: 'Basic 월 $19 · 번역 회의 10시간 포함',
  heroSecondary: '회의 예시 보기',
  heroPoints: [
    ['링크만 보내면', '상대는 가입 없이 참여합니다'],
    ['각자 자기 말로', '자막은 내 언어로 나옵니다'],
    ['휴대폰 하나로', '마주 앉은 대화에도 쓸 수 있습니다'],
  ],

  statement: '통역사를 부르기엔 작은 회의, 번역 앱을 돌리기엔 빠른 대화. 미뤄 온 해외 미팅, 이제 각자의 언어로 할 수 있습니다.',

  meName: '민준',
  meFace: 'minjun',
  otherName: 'さくら',
  otherNameLang: 'ja',
  otherFace: 'sakura',
  you: '{name} (나)',
  panelTitle: '자막',
  panelLangs: [
    ['ko', '한국어'],
    ['ja', '日本語'],
  ],
  translating: '번역 중…',
  storyLabel: '회의 예시: 민준은 한국어로, 사쿠라는 일본어로 말하고, 서로의 말을 자기 언어로 읽습니다.',
  story: [
    { who: 'other', lang: 'ja', text: '来週のデモ、木曜日でも大丈夫ですか？', drafts: ['다음 주', '다음 주 데모'], tr: '다음 주 데모, 목요일도 괜찮으세요?', trLang: 'ko' },
    { who: 'me', lang: 'ko', text: '네, 좋아요. A안과 B안 중에 뭘 먼저 보여 드릴까요?', drafts: ['はい', 'はい、大丈夫です。'], tr: 'はい、大丈夫です。A案とB案、どちらを先にお見せしましょうか？', trLang: 'ja' },
    { who: 'other', lang: 'ja', text: 'Bからでお願いします。', drafts: ['B부터'], tr: 'B안부터 부탁드려요.', trLang: 'ko' },
  ],
  storySteps: [
    { title: '말하는 동안 글자가 됩니다', body: '말이 끝나기를 기다리지 않습니다. 말하는 대로 바로 받아 적습니다.' },
    { title: '번역은 바로 뒤따라옵니다', body: '문장이 이어지는 동안 흐린 글씨로 먼저 보이고, 말이 끝나면 확정됩니다.' },
    { title: '짧은 대답도 맥락대로', body: '앞에 오간 대화를 함께 보고 번역하므로, “Bからで”라는 짧은 대답이 B안을 가리킨다는 것까지 옮깁니다.' },
  ],

  modesTitle: '세 가지 자리에서',
  modes: [
    { title: '화상 회의', body: '최대 4명이 각자 자기 언어로 통화합니다. 컴퓨터나 휴대폰의 브라우저만 있으면 됩니다.' },
    { title: '대면 대화', body: '마주 앉은 두 사람이 휴대폰 하나로 대화합니다. 화면 반쪽씩 각자를 향하고, 상대의 말을 내 언어로 보여 줍니다.' },
    { title: '자막 화면', body: '탁자 위에 태블릿 하나를 두면 됩니다. 방 안의 말을 듣고 문장마다 어떤 언어인지 가려, 번역을 큰 글씨로 보여 줍니다.' },
  ],

  detailsTitle: '회의에서 중요한 것들',
  details: [
    { title: '발음이 서툴러도 괜찮습니다', body: '한국식 발음으로 말한 일본어도 한글이 아니라 일본어로 받아 적습니다.' },
    { title: '게스트는 준비할 것이 없습니다', body: 'Chrome, Safari, Edge에서 링크를 열고 입장을 요청하면 주최자가 들여보냅니다.' },
    { title: '화면 공유', body: '발표하는 동안에도 작은 자막 창이 화면 위에 떠 있습니다(Chrome, Edge).' },
    { title: '대화는 내 손에', body: '원문과 번역이 담긴 자막을 각자 텍스트 파일로 저장할 수 있습니다.' },
    { title: '8개 언어', body: '한국어, 일본어, 영어, 중국어(간체), 스페인어, 프랑스어, 독일어, 베트남어를 지원합니다. 사람마다 자기 언어를 고릅니다.' },
    { title: '오픈 소스', body: '코드가 AGPL-3.0으로 공개되어 있어, 무엇을 하는지 직접 확인할 수 있습니다.' },
  ],

  pricingTitle: '요금',
  pricingLead: '요금제는 한 달에 쓸 수 있는 번역 회의 시간으로 나뉩니다. 게스트는 언제나 무료입니다.',
  plans: [
    { name: 'Basic', price: '$19', unit: '/월', hours: '매달 번역 회의 10시간', note: '월 구독 · 언제든 해지', cta: 'Basic으로 시작하기' },
    { name: 'Pro', price: '$39', unit: '/월', hours: '매달 번역 회의 30시간', note: '월 구독 · 언제든 해지', cta: 'Pro로 시작하기' },
  ],
  includesTitle: '모든 요금제에 포함',
  includes: ['화상 회의, 대면 대화, 자막 화면', '8개 언어 전부', '게스트는 가입 없이 무료로 참여', '텍스트 파일로 저장하는 자막'],
  pricingExtra: '바쁜 달에 더 필요하면 시간당 $3로 추가할 수 있습니다.',
  pricingFine: '가격은 미국 달러 기준입니다. 세금은 해당되는 경우 결제할 때 더해집니다.',

  faqTitle: '자주 묻는 질문',
  faq: [
    { q: '게스트도 가입하거나 앱을 설치해야 하나요?', a: '아닙니다. 게스트는 브라우저로 회의 링크를 열고 입장을 요청하면 됩니다. 계정은 회의를 만드는 사람에게만 필요합니다.' },
    { q: '한 회의에 몇 명까지 참여할 수 있나요?', a: '최대 4명입니다. 각자 다른 언어로 말해도 됩니다.' },
    { q: '전문 통역사를 대신할 수 있나요?', a: '일상적인 비즈니스 대화에는 충분합니다. 계약이나 의료·법률 판단처럼 중요한 내용은 전문 통역사와 한 번 더 확인하시기 바랍니다.' },
    { q: '제 목소리는 어떻게 처리되나요?', a: '말한 내용은 글자로 바꾸고 번역하기 위해 Google Gemini 유료 API로 보내집니다. Google은 유료 API 데이터를 모델 학습에 쓰지 않으며, Tolkmeet은 음성과 자막을 저장하지 않습니다.' },
    { q: '자막 화면은 아무 두 언어나 되나요?', a: '글자 체계로 언어를 가리기 때문에 한국어·일본어, 일본어·영어처럼 글자가 다른 언어끼리 쓸 수 있습니다. 영어·스페인어처럼 같은 알파벳을 쓰는 두 언어는 대면 대화를 이용하시기 바랍니다.' },
    { q: '포함된 시간을 다 쓰면 어떻게 되나요?', a: '시간당 $3로 추가할 수 있으며, 추가한 시간은 다 쓸 때까지 남습니다. 매달 30시간인 Pro로 바꿀 수도 있습니다.' },
  ],

  closingLine1: '다음 회의는,',
  closingLine2: '모두의 언어로.',

  navPrivacy: '개인정보 보호',
  poweredBy: 'Powered by',

  engineTitle1: '빠르고,',
  engineTitle2: '정확하게.',
  engineLead: 'Google의 Gemini가 말을 듣는 순간부터 번역을 흘려보냅니다. 문장이 끝나기를 기다리는 통역 앱과는 속도가 다릅니다.',
  speedEyebrow: '스트리밍 번역',
  speedTitle: '기다림 없이.',
  speedBody: '문장이 끝나기를 기다리지 않습니다. 말하는 동안 번역이 흐린 글씨로 먼저 따라오고, 말이 끝나면 곧바로 확정됩니다.',
  speedSrcLang: 'ja',
  speedRows: [
    ['말하기 시작', '来週のデモ、木曜日でも…'],
    ['말하는 중', '다음 주 데모, 목요일…'],
    ['말이 끝나면', '다음 주 데모, 목요일도 괜찮으세요?'],
  ],
  contextEyebrow: '맥락 번역',
  contextTitle1: '문장이 아니라',
  contextTitle2: '대화를 옮깁니다.',
  contextBody: '앞에 오간 말을 함께 읽고 번역해서, 짧은 대답도 존댓말과 호칭도 회의 내내 흔들리지 않습니다.',
  contextQ: ['ko', 'A안과 B안 중에 뭘 먼저 보여 드릴까요?'],
  contextA: ['ja', 'Bからで。'],
  contextTr: ['ko', 'B안부터 부탁드립니다.'],

  privacyTitle1: '대화는 대화로',
  privacyTitle2: '끝납니다.',
  privacyLead: '영상과 음성은 참가자끼리 암호화되어 오갑니다. 말한 내용은 번역을 위해 Google Gemini로 보내질 뿐, 음성도 자막도 저장하지 않습니다.',
  privacyStageLabel: '자막이 화면에 나타났다가 흩어져 사라지고, 두 참가자 사이로 암호화된 신호만 오가는 그림',
  privacyLines: [
    { who: 'other', lang: 'ja', text: '来週のデモ、木曜日でも大丈夫ですか？', tr: '다음 주 데모, 목요일도 괜찮으세요?', trLang: 'ko' },
    { who: 'me', lang: 'ko', text: '네, 좋아요. 자료는 미리 보내 드릴게요.', tr: 'はい、大丈夫です。資料は事前にお送りします。', trLang: 'ja' },
    { who: 'other', lang: 'ja', text: '価格の条件も確認させてください。', tr: '가격 조건도 확인하고 싶습니다.', trLang: 'ko' },
  ],
  privacyWire: '참가자끼리 암호화',
  privacyChips: [
    ['', '0', '초 녹음'],
    ['서버에 남는 자막 ', '0', '줄'],
    ['저장은 ', '각자 내 기기', '에만'],
  ],
  privacyPause: '움직임 멈추기',
  privacyPlay: '움직임 다시 켜기',
  privacyLink: '개인정보 처리방침',

  footerLegal: '법적 고지',
  legalPrivacy: '개인정보 처리방침',
  legalTerms: '이용약관',
  legalRefund: '환불 정책',
  footerDisclaimer: '번역은 AI가 만들며 틀릴 수 있습니다. 계약·의료·법률처럼 중요한 내용은 전문가와 한 번 더 확인하십시오. 회의 음성은 번역을 위해 Google Gemini 유료 API로 전송되며, Tolkmeet은 저장하지 않습니다. 가격은 미국 달러 기준이며 세금은 결제 시 더해질 수 있습니다.',

  footerProduct: '제품',
  footerOpenSource: 'AGPL-3.0 오픈 소스',
  footerGithub: 'GitHub에서 소스 코드 보기',
  footerCopyright: '© 2026 Tolkmeet',
  biz: { name: '상호', rep: '대표', regNo: '사업자등록번호', address: '주소', email: '이메일', country: '' },
}
