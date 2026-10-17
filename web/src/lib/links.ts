/** 소개 사이트(GitHub Pages)의 법적 문서. 앱과 다른 도메인이라 새 탭으로 연다. */
export const SITE = 'https://tolkmeet.com'
/** 법적 문서는 영어와 한국어가 있다. 한국어 화면이면 한국어판, 나머지는 영어판으로. */
export function legalUrl(doc: 'terms' | 'privacy' | 'refund', locale: string): string {
  return `${SITE}${locale === 'ko' ? '/ko' : ''}/${doc}/`
}
export const GITHUB_URL = 'https://github.com/sogangCSmaster/tolkmeet'
