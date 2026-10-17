import { useState } from 'react'
import { InAppBrowserPage, NotFoundPage } from './components/ErrorPages'
import { Home } from './components/Home'
import { InPerson } from './components/InPerson'
import { LiveCaptions } from './components/LiveCaptions'
import { Login } from './components/Login'
import { Meeting } from './components/Meeting'
import { isInAppBrowser } from './lib/inapp'
import { parseRoomCode, usePath } from './lib/router'

export default function App() {
  const path = usePath()
  const code = parseRoomCode(path)
  // "다시 참여" 시 Meeting을 새로 마운트해 미디어·연결을 처음부터 다시 잡는다.
  const [session, setSession] = useState(0)

  if (path === '/login') return <Login />
  if (path === '/') return <Home />
  if (!code) return <NotFoundPage />
  // 회의 화면은 카메라·마이크가 필요하다. 메신저 앱 안의 브라우저면 장치를 요청하기 전에 다른 브라우저로 안내한다.
  if (isInAppBrowser()) return <InAppBrowserPage />
  if (path.startsWith('/talk/') && code) return <InPerson key={code} code={code} />
  if (path.startsWith('/live/') && code) return <LiveCaptions key={code} code={code} />
  return <Meeting key={`${code}:${session}`} code={code} onRejoin={() => setSession((s) => s + 1)} />
}
