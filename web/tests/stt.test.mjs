import assert from 'node:assert/strict'
import test from 'node:test'
import { loadModule } from './load-module.mjs'

const { Vad, FRAME_SAMPLES } = loadModule('lib/vad.ts')
const { SttController } = loadModule('lib/stt.ts')

const frame = (amp) => Int16Array.from({ length: FRAME_SAMPLES }, (_, i) => (i % 2 ? amp : -amp))
const silence = () => frame(20)
const speech = () => frame(3000)

test('vad starts after two loud frames with preroll and ends after the hangover', () => {
  const vad = new Vad()
  for (let i = 0; i < 50; i++) assert.equal(vad.push(silence()), null)
  assert.equal(vad.push(speech()), null)
  const start = vad.push(speech())
  assert.equal(start.kind, 'start')
  // 300ms 프리롤 = 15프레임(마지막 두 프레임이 말소리)
  assert.equal(start.frames.length, 15)
  assert.equal(vad.push(speech()).kind, 'speech')
  const results = []
  for (let i = 0; i < 30; i++) results.push(vad.push(silence()).kind)
  assert.deepEqual(results.slice(0, 29), Array(29).fill('speech'))
  assert.equal(results[29], 'end')
  assert.equal(vad.push(silence()), null)
})

test('vad learns steady noise as the floor and ignores short clicks', () => {
  const vad = new Vad()
  for (let i = 0; i < 100; i++) vad.push(silence())
  // 조용하던 방에서 선풍기가 켜짐: 말로 시작되더라도 몇 초 안에 끝나고 다시 시작하지 않는다.
  const kinds = []
  for (let i = 0; i < 500; i++) kinds.push(vad.push(frame(600))?.kind ?? null)
  assert.ok(kinds.filter((k) => k === 'start').length <= 1)
  const end = kinds.indexOf('end')
  assert.ok(end > 0 && end * 20 < 6000, `ended after ${end * 20}ms`)
  assert.ok(kinds.slice(end + 1).every((k) => k === null))
  assert.equal(vad.push(frame(8000)), null, 'a single click is not speech')
  assert.equal(vad.push(frame(600)), null)
  assert.equal(vad.push(frame(8000)), null)
})

test('vad keeps real speech with syllable dips going over a noisy floor', () => {
  const vad = new Vad()
  for (let i = 0; i < 300; i++) vad.push(frame(300))
  const kinds = []
  // 10초 동안 200ms마다 60ms씩 잦아드는 말
  for (let i = 0; i < 500; i++) kinds.push(vad.push(frame(i % 10 < 7 ? 4000 : 350))?.kind ?? null)
  assert.equal(kinds.filter((k) => k === 'start').length, 1)
  assert.ok(!kinds.includes('end'), 'speech is not cut while talking')
})

test('vad does not treat the room noise at join as speech', () => {
  const vad = new Vad()
  const kinds = []
  for (let i = 0; i < 100; i++) kinds.push(vad.push(frame(700))?.kind ?? null)
  assert.ok(kinds.every((k) => k === null))
})

function setup({ lang = 'ko', mic = true, seg = { next: 0 } } = {}) {
  const clock = { t: 0 }
  const sessions = []
  const sent = []
  const statuses = []
  const tokens = []
  const deps = {
    now: () => clock.t,
    requestToken: () =>
      new Promise((resolve, reject) => tokens.push({ resolve, reject })),
    openSession: (grant, events) => {
      const s = { grant, events, log: [], closed: false }
      s.activityStart = () => s.log.push('start')
      s.audio = (pcm) => s.log.push(`audio:${pcm.length}`)
      s.activityEnd = () => s.log.push('end')
      s.close = () => {
        if (s.closed) return
        s.closed = true
        events.onClose()
      }
      sessions.push(s)
      return s
    },
    sendStt: (seg, text, final) => sent.push([seg, text, final]),
    onStatus: (s) => statuses.push(s),
  }
  const c = new SttController(deps, seg, lang)
  c.setMic(mic)
  const grant = async (l = lang) => {
    tokens.shift().resolve({ token: `t${sessions.length}`, model: 'models/m', lang: l })
    await Promise.resolve()
  }
  const ready = (i = sessions.length - 1) => sessions[i].events.onReady()
  const speak = (frames = 10) => {
    c.vad({ kind: 'start', frames: [speech(), speech()] })
    for (let i = 0; i < frames; i++) c.vad({ kind: 'speech', frames: [speech()] })
    c.vad({ kind: 'end', frames: [silence()] })
  }
  const advance = (ms) => {
    for (let left = ms; left > 0; left -= 1000) {
      clock.t += Math.min(1000, left)
      c.tick()
    }
  }
  return { c, clock, sessions, sent, statuses, tokens, grant, ready, speak, advance }
}

test('opens a session up front, streams one utterance in 100ms chunks and forwards transcripts', async () => {
  const h = setup()
  assert.equal(h.tokens.length, 1, 'session is requested before anyone speaks')
  await h.grant()
  h.ready()
  assert.equal(h.statuses.at(-1), 'live')
  h.speak(10)
  // 프레임 13개 = 100ms 조각 2개 + 남은 3프레임
  assert.deepEqual(h.sessions[0].log, ['start', `audio:${FRAME_SAMPLES * 5}`, `audio:${FRAME_SAMPLES * 5}`, `audio:${FRAME_SAMPLES * 3}`, 'end'])
  const ev = h.sessions[0].events
  ev.onInterim('안녕')
  ev.onInterim('안녕')
  ev.onInterim('안녕하세요')
  ev.onFinal('안녕하세요.')
  ev.onInterim('다음')
  assert.deepEqual(h.sent, [
    [0, '안녕', false],
    [0, '안녕하세요', false],
    [0, '안녕하세요.', true],
    [1, '다음', false],
  ])
})

test('speech before the session exists is buffered and delivered in order', async () => {
  const h = setup()
  h.speak(4)
  await h.grant()
  assert.deepEqual(h.sessions[0].log, ['start', `audio:${FRAME_SAMPLES * 5}`, 'audio:' + FRAME_SAMPLES * 2, 'end'])
})

test('missing final: the last interim is finalized after a short wait', async () => {
  const seg = { next: 7 }
  const h = setup({ seg })
  await h.grant()
  h.ready()
  h.speak()
  h.sessions[0].events.onInterim('hello there')
  h.advance(1000)
  assert.equal(h.sent.length, 1)
  h.advance(2000)
  assert.deepEqual(h.sent, [[7, 'hello there', false], [7, 'hello there', true]])
  assert.equal(seg.next, 8, 'the counter is shared with later controllers on the same connection')
})

test('keepalive silence every 30s while quiet', async () => {
  const h = setup()
  await h.grant()
  h.ready()
  h.advance(29_000)
  assert.deepEqual(h.sessions[0].log, [])
  h.advance(1000)
  assert.deepEqual(h.sessions[0].log, ['audio:1600'])
  h.advance(30_000)
  assert.deepEqual(h.sessions[0].log, ['audio:1600', 'audio:1600'])
})

test('after 3 minutes of silence a fresh session is opened in the background and swapped in', async () => {
  const h = setup()
  await h.grant()
  h.ready()
  h.advance(179_000)
  assert.equal(h.tokens.length, 0)
  h.advance(1000)
  assert.equal(h.tokens.length, 1)
  await h.grant()
  assert.equal(h.sessions[0].closed, false, 'old session stays until the new one is ready')
  h.ready(1)
  assert.equal(h.sessions[0].closed, true)
  assert.equal(h.statuses.at(-1), 'live')
  assert.ok(!h.statuses.includes('paused'))
  h.speak(4)
  assert.equal(h.sessions[1].log[0], 'start')
  // 교체 직후에는 다시 교체하지 않는다.
  h.advance(60_000)
  assert.equal(h.tokens.length, 0)
})

test('language change rotates the session, but never in the middle of an utterance', async () => {
  const h = setup()
  await h.grant()
  h.ready()
  h.c.vad({ kind: 'start', frames: [speech()] })
  h.c.setLang('ja')
  await h.grant('ja')
  h.ready(1)
  assert.equal(h.sessions[0].closed, false, 'still speaking')
  h.c.vad({ kind: 'end', frames: [] })
  h.c.tick()
  assert.equal(h.sessions[0].closed, false, 'waiting for the final')
  h.sessions[0].events.onFinal('끝.')
  h.c.tick()
  assert.equal(h.sessions[0].closed, true)
  h.speak(1)
  assert.equal(h.sessions[1].log[0], 'start')
})

test('a dropped session is reopened with backoff and shows paused meanwhile', async () => {
  const h = setup()
  await h.grant()
  h.ready()
  h.advance(40_000)
  h.sessions[0].close() // 오래 잘 동작하던 세션: 바로 다시 연다
  assert.equal(h.tokens.length, 1)
  await h.grant()
  h.sessions[1].close() // 준비되기 전에 끊김 → 실패
  assert.equal(h.statuses.at(-1), 'paused')
  assert.equal(h.tokens.length, 0)
  h.advance(1000)
  assert.equal(h.tokens.length, 1)
  h.tokens.shift().reject('failed')
  await Promise.resolve()
  h.advance(2000)
  assert.equal(h.tokens.length, 0, 'second failure waits longer')
  h.advance(1000)
  assert.equal(h.tokens.length, 1)
  await h.grant()
  h.ready()
  assert.equal(h.statuses.at(-1), 'live')
})

test('unavailable stops asking for tokens', async () => {
  const h = setup()
  h.tokens.shift().reject('unavailable')
  await Promise.resolve()
  h.speak()
  h.advance(120_000)
  assert.equal(h.tokens.length, 0)
  assert.equal(h.statuses.at(-1), 'unavailable')
})

test('muting ends the utterance at once; a long mute closes the session and unmuting reopens', async () => {
  const h = setup()
  await h.grant()
  h.ready()
  h.c.vad({ kind: 'start', frames: [speech()] })
  h.c.setMic(false)
  assert.deepEqual(h.sessions[0].log, ['start', `audio:${FRAME_SAMPLES}`, 'end'])
  h.sessions[0].events.onFinal('')
  h.advance(179_000)
  assert.equal(h.sessions[0].closed, false)
  h.advance(1000)
  assert.equal(h.sessions[0].closed, true)
  assert.equal(h.tokens.length, 0, 'no session while muted')
  h.c.setMic(true)
  assert.equal(h.tokens.length, 1)
})

test('dispose ends speech and closes every session', async () => {
  const h = setup()
  await h.grant()
  h.ready()
  h.c.vad({ kind: 'start', frames: [speech()] })
  h.c.dispose()
  assert.equal(h.sessions[0].log.at(-1), 'end')
  assert.equal(h.sessions[0].closed, true)
  h.advance(60_000)
  assert.equal(h.tokens.length, 0)
})
