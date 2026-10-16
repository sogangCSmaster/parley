import type { RoomSettings } from '../protocol/RoomSettings'

const JSON_HEADERS = { 'Content-Type': 'application/json' }

export type LoginResult = 'ok' | 'invalid' | 'locked' | 'failed'

export async function fetchMe(): Promise<string | null> {
  const res = await fetch('/api/me')
  if (!res.ok) throw new Error(`status ${res.status}`)
  return ((await res.json()) as { username: string | null }).username
}

export async function login(username: string, password: string): Promise<LoginResult> {
  try {
    const res = await fetch('/api/login', { method: 'POST', headers: JSON_HEADERS, body: JSON.stringify({ username, password }) })
    if (res.ok) return 'ok'
    if (res.status === 401) return 'invalid'
    if (res.status === 429) return 'locked'
    return 'failed'
  } catch {
    return 'failed'
  }
}

export async function logout() {
  const res = await fetch('/api/logout', { method: 'POST' })
  if (!res.ok) throw new Error(`status ${res.status}`)
}

export async function createRoom(settings: RoomSettings): Promise<string> {
  const res = await fetch('/api/rooms', { method: 'POST', headers: JSON_HEADERS, body: JSON.stringify(settings) })
  if (!res.ok) throw new Error(`status ${res.status}`)
  return ((await res.json()) as { code: string }).code
}

/** 없는 회의면 null. */
export async function fetchRoom(code: string): Promise<RoomSettings | null> {
  const res = await fetch(`/api/rooms/${code}`)
  if (res.status === 404) return null
  if (!res.ok) throw new Error(`status ${res.status}`)
  return (await res.json()) as RoomSettings
}
