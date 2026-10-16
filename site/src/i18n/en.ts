// 소개 페이지 문구(영어가 기준). 키를 더하면 ko·ja에 빠진 키가 타입 오류로 드러난다.
export const en = {
  lang: 'en',
  htmlLang: 'en',
  path: '/',
  langName: 'English',

  metaTitle: 'Tolkmeet: video meetings with live translated captions',
  metaDescription:
    'Speak your own language in a video meeting. Tolkmeet shows what everyone says, translated into your language as they talk. Korean, Japanese, English and more. Guests join from a link, no account needed.',

  navHow: 'How it works',
  navPricing: 'Pricing',
  navFaq: 'FAQ',
  signIn: 'Sign in',
  startTrial: 'Start 7-day trial',
  languageLabel: 'Language',
  skipToContent: 'Skip to content',

  heroTitle: 'Video meetings where everyone speaks their own language',
  heroLead:
    'Captions show what each person says in their own words and in yours, a moment after they say it. Send a link and your guest joins from the browser, without an account.',
  heroSecondary: 'See a meeting',
  heroCaptionLabel: 'Example: a spoken sentence appears as a caption, with its translation below',
  navLabel: 'Main',

  // 히어로의 큰 자막: [이름, 말한 언어 코드, 말한 언어 이름, 원문, 번역 중간 결과들, 번역(이 페이지 언어)]
  heroLines: [
    ['さくら', 'ja', '日本語', '来週の打ち合わせ、木曜日でも大丈夫ですか？', ['Is next', 'Is next week’s meeting'], 'Would Thursday work for next week’s meeting?'],
    ['민준', 'ko', '한국어', '네, 목요일 오후 세 시로 할게요.', ['Yes,', 'Yes, Thursday'], 'Sure, let’s make it Thursday at 3 p.m.'],
    ['さくら', 'ja', '日本語', 'では資料は前日までにお送りします。', ['Then', 'Then I’ll send the'], 'Then I’ll send the materials the day before.'],
  ] as [string, string, string, string, string[], string][],

  demoTitle: 'What your meeting looks like',
  demoLead:
    'Sakura speaks Japanese and Minjun speaks Korean. Each reads the other in their own language, with the original right above the translation.',
  demoYou: 'Minjun (you)',
  demoCaptions: 'Captions',

  modesTitle: 'One app, three ways to meet',
  modes: [
    {
      title: 'Video meeting',
      body: 'Up to four people, each speaking their own language, from a browser on a computer or phone.',
    },
    {
      title: 'In-person conversation',
      body: 'Two people share one phone or tablet. Each half of the screen faces one person and shows the other’s words translated.',
    },
    {
      title: 'Live captions screen',
      body: 'One tablet listens to the room and tells Korean from Japanese on its own. Every sentence appears with the translation in large type.',
    },
  ],

  stepsTitle: 'How it works',
  steps: [
    { title: 'Create a meeting', body: 'Sign in, create a meeting and send the link by email or chat.' },
    { title: 'Pick your languages', body: 'Everyone chooses the language they speak and the one the others speak.' },
    { title: 'Talk', body: 'Your words appear as you speak, and the translation follows within a second or two.' },
  ],

  whyTitle: 'Why the captions are easy to follow',
  why: [
    {
      title: 'Translated with the conversation in view',
      body: 'Each sentence is translated along with the lines before it, so short replies like “yes, that one” come out right.',
    },
    {
      title: 'Fast enough to keep talking',
      body: 'Translation starts while the sentence is still being spoken and settles a moment after it ends.',
    },
    {
      title: 'Accents are fine',
      body: 'Japanese spoken with a Korean accent is still written as Japanese, not as Korean sounds.',
    },
    {
      title: 'Guests need nothing',
      body: 'Guests open the link in Chrome, Safari or Edge on a computer or phone, and ask to join. You let them in.',
    },
    {
      title: 'Nothing kept on our servers',
      body: 'Video and audio are encrypted and go straight between participants, passing through our relay only when a direct connection isn’t possible. Speech is sent to Google Gemini to be recognized and translated. Neither audio nor captions are stored, and anyone can save their own captions as a text file.',
    },
    {
      title: 'Open source',
      body: 'The code is public under the AGPL-3.0 license, so you can see exactly what it does.',
    },
  ],

  pricingTitle: 'Pricing',
  pricingLead: 'Plans are set by hours of translated meetings a month. Guests never pay.',
  plans: [
    {
      name: 'Trial',
      price: '$0',
      unit: 'for 7 days',
      hours: '60 minutes of translated meetings',
      note: 'A card is needed to start. After 7 days it becomes Pro at $19 a month unless you cancel before the trial ends.',
      cta: 'Start 7-day trial',
    },
    {
      name: 'Pro',
      price: '$19',
      unit: '/month',
      hours: '10 hours of translated meetings a month',
      note: '$15 a month when billed yearly.',
      cta: 'Start with Pro',
    },
    {
      name: 'Business',
      price: '$39',
      unit: '/user/month',
      hours: '30 hours a month for each user',
      note: '$32 a month when billed yearly.',
      cta: 'Start with Business',
    },
  ],
  pricingExtra: 'Need more in a busy month? Add hours for $3 each.',
  pricingFine: 'Prices in US dollars. Tax is added at checkout where it applies.',

  faqTitle: 'Questions',
  faq: [
    {
      q: 'Which languages are supported?',
      a: 'Korean, Japanese, English, Chinese (Simplified), Spanish, French, German and Vietnamese. Each person can pick a different one.',
    },
    {
      q: 'Do guests need an account or an app?',
      a: 'No. Guests open the meeting link in a browser and ask to join. Only the person who creates meetings needs an account.',
    },
    {
      q: 'How many people can join a meeting?',
      a: 'Up to four. Each person can speak a different language.',
    },
    {
      q: 'Can it replace a professional interpreter?',
      a: 'For everyday business conversation it works well. For contracts, medical or legal decisions, confirm the important points with a qualified interpreter.',
    },
    {
      q: 'What happens to my voice?',
      a: 'Your speech is sent to Google Gemini through its paid API to be turned into text and translated. Google does not use paid API data to train its models, and Tolkmeet does not store audio or captions.',
    },
    {
      q: 'Does the live captions screen work with any two languages?',
      a: 'It tells languages apart by their writing system, so it works for pairs like Korean and Japanese or Japanese and English. Two languages that share an alphabet, such as English and Spanish, need an in-person conversation instead.',
    },
  ],

  footerProduct: 'Product',
  footerOpenSource: 'Open source under AGPL-3.0',
  footerGithub: 'Source code on GitHub',
  footerCopyright: '© 2026 Tolkmeet',
}

export type Dict = typeof en
