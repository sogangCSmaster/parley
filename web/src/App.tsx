import { lazy, Suspense, useState } from 'react'
import { InAppBrowserPage, NotFoundPage } from './components/ErrorPages'
import { Home } from './components/Home'
import { InPerson } from './components/InPerson'
import { LiveCaptions } from './components/LiveCaptions'
import { Meeting } from './components/Meeting'
import { isInAppBrowser } from './lib/inapp'
import { parseRoomCode, usePath } from './lib/router'

// 회의와 상관없는 화면(로그인·계정·회의 목록·결제)은 필요할 때 받는다. 링크로 회의에 들어오는 손님은 받지 않는다.
const Login = lazy(() => import('./components/AuthPages').then((m) => ({ default: m.Login })))
const Signup = lazy(() => import('./components/AuthPages').then((m) => ({ default: m.Signup })))
const Verify = lazy(() => import('./components/AuthPages').then((m) => ({ default: m.Verify })))
const Forgot = lazy(() => import('./components/AuthPages').then((m) => ({ default: m.Forgot })))
const Reset = lazy(() => import('./components/AuthPages').then((m) => ({ default: m.Reset })))
const Account = lazy(() => import('./components/AccountPages').then((m) => ({ default: m.Account })))
const Security = lazy(() => import('./components/AccountPages').then((m) => ({ default: m.Security })))
const Meetings = lazy(() => import('./components/MeetingsPage').then((m) => ({ default: m.MeetingsPage })))
const Checkout = lazy(() => import('./components/BillingPages').then((m) => ({ default: m.Checkout })))
const PaidDone = lazy(() => import('./components/BillingPages').then((m) => ({ default: m.PaidDone })))

const PAGES: Record<string, React.ComponentType> = {
  '/login': Login,
  '/signup': Signup,
  '/verify': Verify,
  '/forgot': Forgot,
  '/reset': Reset,
  '/account': Account,
  '/account/security': Security,
  '/meetings': Meetings,
  '/checkout': Checkout,
  '/billing/done': PaidDone,
}

export default function App() {
  const path = usePath()
  const code = parseRoomCode(path)
  // "다시 참여" 시 Meeting을 새로 마운트해 미디어·연결을 처음부터 다시 잡는다.
  const [session, setSession] = useState(0)

  const Page = PAGES[path.replace(/\/$/, '') || '/']
  if (Page) {
    return (
      <Suspense fallback={<div className="shell" />}>
        <Page />
      </Suspense>
    )
  }
  if (path === '/') return <Home />
  if (!code) return <NotFoundPage />
  // 회의 화면은 카메라·마이크가 필요하다. 메신저 앱 안의 브라우저면 장치를 요청하기 전에 다른 브라우저로 안내한다.
  if (isInAppBrowser()) return <InAppBrowserPage />
  if (path.startsWith('/talk/') && code) return <InPerson key={code} code={code} />
  if (path.startsWith('/live/') && code) return <LiveCaptions key={code} code={code} />
  return <Meeting key={`${code}:${session}`} code={code} onRejoin={() => setSession((s) => s + 1)} />
}
