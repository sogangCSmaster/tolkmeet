import type { Dict } from './en'

// 소개 페이지의 한국어는 합니다체로 통일한다(유료 업무용 서비스의 소개). 앱 안의 버튼·안내는 해요체 그대로.
// 문구는 design/v2/source/MainFull.dc.html 그대로이고, 지원 언어 수(9개)와 사실에 맞게 고친 곳만 다르다.
export const ko: Dict = {
  lang: 'ko',
  htmlLang: 'ko',
  path: '/ko/',
  langName: '한국어',

  metaTitle: 'Tolkmeet: 실시간 번역 자막이 나오는 화상 회의',
  metaDescription:
    '한국어로 말하면 상대는 일본어로 읽습니다. 말하는 문장마다 듣는 사람의 언어로 자막이 바로 나오는 화상 회의입니다. 한국어·일본어·영어 등 9개 언어를 지원하며, 게스트는 가입 없이 링크로 참여합니다.',

  skipToContent: '본문으로 건너뛰기',
  navLabel: '주요 메뉴',
  navHow: '사용 장면',
  navModes: '세 가지 자리',
  navPrivacy: '개인정보 보호',
  navPricing: '요금',
  navFaq: '자주 묻는 질문',
  signIn: '로그인',
  getStarted: '시작하기',
  languageLabel: '화면 언어',

  meName: '민준',
  otherName: 'さくら',
  otherLang: 'ja',

  heroTitle1: '말은 각자의 언어로.',
  heroTitle2: '대화는 하나로.',
  heroLead: '말하는 순간, 상대의 화면에는 그 사람의 언어로 자막이 흐릅니다. 한국어, 일본어, 영어를 포함한 9개 언어.',
  heroSecondary: '회의 예시 보기',
  heroPrice: 'Basic 월 $19부터. 번역 회의 10시간 포함.',
  poweredBy: 'Powered by',

  demoLabel: '회의 예시',
  demoAlt: '회의 화면 예시: さくら가 일본어로 다음 주 데모를 목요일에 해도 되는지 묻고, 민준은 한국어 자막으로 읽습니다. 가장 최근 자막이 가장 큽니다.',
  historyTitle: '대화 기록',
  historyListening: '듣는 중',
  history: [
    { who: 'me', time: '15:02', src: ['ko', '자료는 미리 보내 드렸어요.'], tr: ['ja', '資料は事前にお送りしました。'] },
    { who: 'other', time: '15:03', src: ['ja', 'ありがとうございます。確認しました。'], tr: ['ko', '감사합니다. 확인했어요.'] },
    { who: 'other', time: '15:03', src: ['ja', '来週のデモ、木曜日でも大丈夫ですか？'], tr: ['ko', '다음 주 데모, 목요일도 괜찮으세요?'] },
  ],

  statement1: '통역사를 부르기엔 작은 회의, 번역 앱을 돌리기엔 빠른 대화.',
  statement2: '미뤄 온 해외 미팅, 이제 각자의 언어로 할 수 있습니다.',

  featureTitle1: '말하는 동안,',
  featureTitle2: '글자가 됩니다.',
  featureBody: '말이 끝나기를 기다리지 않습니다. 번역은 흐린 글씨로 먼저 따라오고, 문장이 끝나면 확정됩니다. 앞에 오간 대화를 함께 보기 때문에 “B부터요” 같은 짧은 대답도 맥락대로 옮깁니다.',
  featureAlt: '대화가 이어지며 자막이 쌓이는 모습. 가장 최근 번역이 가장 크고, 앞의 줄은 작아지고 흐려집니다.',
  featureLines: [
    { src: ['ja', '来週のデモ、木曜日でも大丈夫ですか？'], tr: ['ko', '다음 주 데모, 목요일도 괜찮으세요?'] },
    { src: ['ko', '네, 좋아요. A안과 B안 중에 뭘 먼저 보여 드릴까요?'], tr: ['ja', 'はい、大丈夫です。A案とB案、どちらを先に？'] },
    { src: ['ja', 'Bからでお願いします。'], tr: ['ko', 'B안부터 부탁드려요.'] },
  ],

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
  contextA: ['ko', 'B부터요.'],
  contextTr: ['ja', 'Bからお願いします。'],

  modesTitle: '세 가지 자리에서.',
  modesLead: '온라인 회의든, 마주 앉은 상담이든, 회의실 한가운데든.',
  modeVideo: { eyebrow: '화상 회의', title1: '링크 하나로,', title2: '각자의 언어로.', body: '최대 4명. 받은 사람은 가입도 설치도 없이 브라우저로 들어옵니다.' },
  modeInPerson: { eyebrow: '대면 대화', title1: '휴대폰 하나를', title2: '사이에 두고.', body: '화면 반쪽씩 각자를 향하고, 상대의 말을 내 언어로 보여 줍니다.' },
  // 자막 화면은 글자 체계로 언어를 가르므로(영어·스페인어처럼 같은 글자끼리는 못 가른다) "글자가 다른 두 언어"로 한정한다.
  modeCaptions: { eyebrow: '자막 화면', title1: '탁자 위에', title2: '태블릿 한 대.', body: '글자가 다른 두 언어라면 누가 어느 쪽으로 말하든 스스로 가려, 번역을 크게 보여 줍니다.' },
  phoneTop: { src: ['ko', '네, 목요일에요.'], tr: ['ja', 'はい、木曜日に。'] },
  phoneBottom: { src: ['ja', 'ありがとうございます。'], tr: ['ko', '감사합니다.'] },
  phoneButton: '누르고 말하기',
  tabletPast: [
    ['ko', '오늘 시간 내 주셔서 감사합니다.'],
    ['ja', 'まず弊社のサービスをご紹介します。'],
  ],
  tabletNow: { src: ['ja', '予算について確認したいです。'], tr: ['ko', '예산에 대해 확인하고 싶습니다.'] },

  detailsTitle: '회의에서 중요한 것들.',
  details: [
    { title: '9개 언어', body: '한국어, 일본어, 영어, 중국어(간체), 스페인어, 프랑스어, 독일어, 베트남어, 태국어. 사람마다 자기 언어를 고릅니다.' },
    { title: '가입 없이', body: '게스트는 링크를 열고 입장을 요청하면 끝. 앱 설치도 없습니다.' },
    { title: '발음 그대로', body: '한국식 발음으로 말한 일본어도 일본어로 받아 적습니다.' },
    { title: '.txt로 저장', body: '원문과 번역이 담긴 대화를 각자 파일로 남길 수 있습니다.' },
    { title: '화면 공유', body: '발표하는 동안에도 작은 자막 창이 화면 위에 떠 있습니다(Chrome, Edge).' },
    { title: '오픈 소스', body: '코드가 AGPL-3.0으로 공개되어 있어 직접 확인할 수 있습니다.' },
  ],

  privacyTitle1: '대화는 대화로',
  privacyTitle2: '끝납니다.',
  privacyLead: '영상과 음성은 참가자끼리 암호화되어 오갑니다. 말한 내용은 번역을 위해 Google Gemini로 보내질 뿐, 음성도 자막도 저장하지 않습니다.',
  privacyStageLabel: '자막이 화면에 나타났다가 흩어져 사라지고, 두 참가자 사이로 암호화된 신호만 오가는 그림',
  privacyLines: [
    { who: 'other', src: ['ja', '来週のデモ、木曜日でも大丈夫ですか？'], tr: ['ko', '다음 주 데모, 목요일도 괜찮으세요?'] },
    { who: 'me', src: ['ko', '네, 좋아요. 자료는 미리 보내 드릴게요.'], tr: ['ja', 'はい、大丈夫です。資料は事前にお送りします。'] },
    { who: 'other', src: ['ja', '価格の条件も確認させてください。'], tr: ['ko', '가격 조건도 확인하고 싶습니다.'] },
  ],
  privacyWire: '참가자끼리 암호화',
  privacyChips: [
    ['', '0', '초 녹음'],
    ['', '0', '줄 서버에 남은 자막'],
    ['저장은 ', '각자 내 기기', '에만'],
  ],
  privacyPause: '움직임 멈추기',
  privacyLink: '개인정보 처리방침',

  pricingTitle: '요금.',
  pricingLead: '한 달에 쓸 번역 회의 시간으로 고르세요. 게스트는 언제나 무료입니다.',
  plans: [
    { name: 'Basic', price: '$19', unit: ' /월', note: '매달 번역 회의 10시간 · 언제든 해지', cta: 'Basic으로 시작하기' },
    { name: 'Pro', price: '$39', unit: ' /월', note: '매달 번역 회의 30시간 · 언제든 해지', cta: 'Pro로 시작하기' },
  ],
  pricingFine: '모든 요금제: 세 가지 자리, 9개 언어, 자막 저장 포함. 바쁜 달에는 시간당 $3로 추가. 가격은 미국 달러, 세금 별도.',

  faqTitle: '자주 묻는 질문.',
  faq: [
    { q: '게스트도 가입하거나 앱을 설치해야 하나요?', a: '아닙니다. 링크를 열고 입장을 요청하면 됩니다. 계정은 회의를 만드는 사람에게만 필요합니다.' },
    { q: '한 회의에 몇 명까지 참여할 수 있나요?', a: '최대 4명입니다. 각자 다른 언어로 말해도 됩니다.' },
    { q: '전문 통역사를 대신할 수 있나요?', a: '일상적인 비즈니스 대화에는 충분합니다. 계약이나 의료·법률 판단처럼 중요한 내용은 전문 통역사와 한 번 더 확인하시기 바랍니다.' },
    { q: '제 목소리는 어떻게 처리되나요?', a: '말한 내용은 글자로 바꾸고 번역하기 위해 Google Gemini 유료 API로 보내집니다. Google은 유료 API 데이터를 모델 학습에 쓰지 않으며, Tolkmeet은 음성과 자막을 저장하지 않습니다.' },
    { q: '포함된 시간을 다 쓰면 어떻게 되나요?', a: '시간당 $3로 추가할 수 있으며, 추가한 시간은 다 쓸 때까지 남습니다. 매달 30시간인 Pro로 바꿀 수도 있습니다.' },
  ],

  closingTitle1: '다음 회의는,',
  closingTitle2: '모두의 언어로.',

  // 디자인은 "저장되지 않습니다". Google은 자사 API 약관에 따라 처리하므로, 사실대로 Tolkmeet이 저장하지 않는다고 쓴다.
  footerDisclaimer: '번역은 AI가 만들며 틀릴 수 있습니다. 계약·의료·법률처럼 중요한 내용은 전문가와 한 번 더 확인하십시오. 회의 음성은 번역을 위해 Google Gemini 유료 API로 전송되며, Tolkmeet은 저장하지 않습니다. 가격은 미국 달러 기준이며 세금은 결제 시 더해질 수 있습니다.',
  footerGithub: 'GitHub에서 코드 보기',
  footerProduct: '제품',
  footerSupport: '지원',
  footerContact: '문의하기',
  footerCompany: '회사',
  footerSecurity: '보안',
  footerLegal: '법적 고지',
  legalPrivacy: '개인정보 처리방침',
  legalTerms: '이용약관',
  legalRefund: '환불 정책',
  footerRights: 'All rights reserved.',
  biz: { name: '상호', rep: '대표', regNo: '사업자등록번호', address: '주소', email: '이메일', country: '' },
}
