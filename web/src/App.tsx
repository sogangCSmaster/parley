import { useState } from 'react'
import { Home } from './components/Home'
import { Login } from './components/Login'
import { Meeting } from './components/Meeting'
import { parseRoomCode, usePath } from './lib/router'

export default function App() {
  const path = usePath()
  const code = parseRoomCode(path)
  // "다시 참여" 시 Meeting을 새로 마운트해 미디어·연결을 처음부터 다시 잡는다.
  const [session, setSession] = useState(0)

  if (path === '/login') return <Login />
  if (path === '/' || !code) return <Home invalidPath={path !== '/'} />
  return <Meeting key={`${code}:${session}`} code={code} onRejoin={() => setSession((s) => s + 1)} />
}
