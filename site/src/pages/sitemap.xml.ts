import { LOCALES, SITE_URL } from '../site'

// 언어별 주소와 서로의 hreflang을 담은 사이트맵(페이지가 셋뿐이라 플러그인 없이 만든다).
export function GET() {
  const links = LOCALES.map((l) => `    <xhtml:link rel="alternate" hreflang="${l.lang}" href="${SITE_URL}${l.path}"/>`).join('\n')
  const urls = LOCALES.map((l) => `  <url>\n    <loc>${SITE_URL}${l.path}</loc>\n${links}\n  </url>`).join('\n')
  const body = `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">\n${urls}\n</urlset>\n`
  return new Response(body, { headers: { 'Content-Type': 'application/xml' } })
}
