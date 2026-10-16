// 소개 페이지 문구(영어가 기준). 키를 더하면 ko·ja에 빠진 키가 타입 오류로 드러난다.

/** 스크롤 장면의 한 발화: 누가(상대/나), 말한 언어, 원문, 번역 중간 결과들, 확정 번역과 그 언어 */
export type Utterance = { who: 'other' | 'me'; lang: string; text: string; drafts: string[]; tr: string; trLang: string }
export type FaceName = 'sakura' | 'minjun'

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
  getStarted: 'Start with Pro',
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
  heroPrice: 'Pro $19 a month, 10 hours of translated meetings included',
  heroSecondary: 'See an example meeting',
  // 기능 — 결과: 로고처럼 둘째 줄이 비켜 선다
  heroPoints: [
    ['Send a link', 'Guests join without an account'],
    ['Speak your own language', 'Captions show up in theirs'],
    ['One phone on the table', 'Works face to face, too'],
  ] as [string, string][],

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
    { title: 'Encrypted, and never stored', body: 'Video and audio are encrypted between participants and pass through our relay only when a direct connection isn’t possible. Speech is sent to Google Gemini to be recognized and translated. We keep neither audio nor captions.' },
    { title: 'Keep the conversation', body: 'Anyone can save their own captions, original and translation, as a text file.' },
    { title: 'Eight languages', body: 'Korean, Japanese, English, Chinese (Simplified), Spanish, French, German and Vietnamese. Each person picks their own.' },
    { title: 'Open source', body: 'The code is public under AGPL-3.0, so you can check exactly what it does.' },
  ],

  pricingTitle: 'Pricing',
  pricingLead: 'Plans are set by hours of translated meetings each month. Guests are always free.',
  plans: [
    { name: 'Pro', price: '$19', unit: '/month', hours: '10 hours of translated meetings a month', note: '$15 a month when billed yearly.', cta: 'Start with Pro' },
    { name: 'Business', price: '$39', unit: '/user/month', hours: '30 hours a month for each user', note: '$32 a month when billed yearly.', cta: 'Start with Business' },
  ],
  pricingExtra: 'Need more in a busy month? Add hours for $3 each.',
  pricingFine: 'Prices in US dollars. Tax is added at checkout where it applies.',

  faqTitle: 'Questions',
  faq: [
    { q: 'Do guests need an account or an app?', a: 'No. Guests open the meeting link in a browser and ask to join. Only the person who creates meetings needs an account.' },
    { q: 'How many people can join a meeting?', a: 'Up to four. Each person can speak a different language.' },
    { q: 'Can it replace a professional interpreter?', a: 'For everyday business conversation it works well. For contracts, medical or legal decisions, confirm the important points with a qualified interpreter.' },
    { q: 'What happens to my voice?', a: 'Your speech is sent to Google Gemini through its paid API to be turned into text and translated. Google does not use paid API data to train its models, and Tolkmeet does not store audio or captions.' },
    { q: 'Does the live captions screen work with any two languages?', a: 'It tells languages apart by their writing system, so it works for pairs like Korean and Japanese or Japanese and English. For two languages that share an alphabet, such as English and Spanish, use an in-person conversation instead.' },
    { q: 'Why is there no free trial?', a: 'Every translated minute has a real cost, so Tolkmeet is paid only. The example meeting on this page shows how the screen and captions work.' },
    { q: 'What if I go over 10 hours?', a: 'Add hours for $3 each in that month, or move to Business for 30 hours per user.' },
  ],

  closingLine1: 'Your next meeting,',
  closingLine2: 'in everyone’s language.',

  footerProduct: 'Product',
  footerOpenSource: 'Open source under AGPL-3.0',
  footerGithub: 'Source code on GitHub',
  footerCopyright: '© 2026 Tolkmeet',
}

export type Dict = typeof en
