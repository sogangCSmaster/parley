/**
 * 메신저·SNS 앱 안의 브라우저(WebView). 카메라·마이크 권한을 못 받거나 WebRTC가 막혀 있어 회의를 열 수 없다.
 * 카카오톡·라인·네이버·인스타그램·페이스북·위챗·X·틱톡, 그리고 안드로이드 WebView 표시("; wv)")를 본다.
 */
const IN_APP = /KAKAOTALK|\bLine\/|NAVER\(inapp|Instagram|FBAN|FBAV|FB_IAB|MicroMessenger|Twitter|BytedanceWebview|musical_ly|; wv\)/i

export function isInAppBrowser(ua: string = navigator.userAgent): boolean {
  return IN_APP.test(ua)
}

/** 안드로이드에서는 Chrome으로 바로 넘기는 intent 링크를 줄 수 있다. iOS는 링크 복사만. */
export function chromeIntentUrl(href: string = location.href, ua: string = navigator.userAgent): string | null {
  if (!/Android/i.test(ua)) return null
  const url = new URL(href)
  return `intent://${url.host}${url.pathname}${url.search}#Intent;scheme=${url.protocol.replace(':', '')};package=com.android.chrome;end`
}
