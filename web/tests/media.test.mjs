import assert from 'node:assert/strict'
import test from 'node:test'
import { loadModule } from './load-module.mjs'

function deferred() {
  let resolve
  const promise = new Promise((done) => { resolve = done })
  return { promise, resolve }
}

function track() {
  return { readyState: 'live', onended: null, stop() { this.readyState = 'ended' } }
}

function screenHarness(replace = async () => {}) {
  const request = deferred()
  const display = track()
  const camera = track()
  const states = []
  const busy = []
  const errors = []
  let captures = 0
  const { ScreenShare } = loadModule('lib/screen.ts', {}, {
    DOMException,
    navigator: { mediaDevices: { getDisplayMedia() { captures++; return request.promise } } },
  })
  const share = new ScreenShare(
    { setVideoTrack: replace }, camera,
    (value) => states.push(value), (value) => busy.push(value), () => errors.push(true),
  )
  return {
    share, display, camera, states, busy, errors,
    captures: () => captures,
    release: () => request.resolve({ getVideoTracks: () => [display], getTracks: () => [display] }),
  }
}

test('leaving while the screen picker is open stops its late capture', async () => {
  const h = screenHarness()
  const pending = h.share.toggle()
  h.share.dispose()
  h.release()
  await pending
  assert.equal(h.display.readyState, 'ended')
  assert.deepEqual(h.states, [])
})

test('duplicate share clicks open only one capture request', async () => {
  const h = screenHarness()
  const pending = h.share.toggle()
  await h.share.toggle()
  assert.equal(h.captures(), 1)
  h.release()
  await pending
  assert.equal(h.states[0], h.display)
  h.share.dispose()
  assert.equal(h.display.readyState, 'ended')
})

test('leaving during sender replacement stops the owned capture and publishes no share', async () => {
  const replace = deferred()
  const entered = deferred()
  const h = screenHarness(() => { entered.resolve(); return replace.promise })
  const pending = h.share.toggle()
  h.release()
  await entered.promise
  h.share.dispose()
  assert.equal(h.display.readyState, 'ended')
  replace.resolve()
  await pending
  assert.deepEqual(h.states, [])
})

test('a failed screen replacement stops capture and restores the camera', async () => {
  const replacements = []
  const h = screenHarness(async (value) => {
    replacements.push(value)
    if (replacements.length === 1) throw new Error('encoder rejected the screen')
  })
  const pending = h.share.toggle()
  h.release()
  await pending
  assert.deepEqual(replacements, [h.display, h.camera])
  assert.equal(h.display.readyState, 'ended')
  assert.deepEqual(h.states, [null])
  assert.equal(h.errors.length, 1)
  assert.equal(h.busy.at(-1), false)
})

test('a failed camera restore still stops screen capture and reports failure', async () => {
  let fail = false
  const h = screenHarness(async () => { if (fail) throw new Error('restore failed') })
  const pending = h.share.toggle()
  h.release()
  await pending
  fail = true
  await h.share.toggle()
  assert.equal(h.display.readyState, 'ended')
  assert.equal(h.states.at(-1), null)
  assert.equal(h.errors.length, 1)
})

test('screen capture ended during replacement is never announced as sharing', async () => {
  const replace = deferred()
  const entered = deferred()
  let calls = 0
  const h = screenHarness(() => { entered.resolve(); return ++calls === 1 ? replace.promise : Promise.resolve() })
  const pending = h.share.toggle()
  h.release()
  await entered.promise
  h.display.stop()
  replace.resolve()
  await pending
  assert.deepEqual(h.states, [null])
  assert.equal(calls, 2)
})

function meshHarness(replace) {
  const pcs = []
  class Stream {
    getAudioTracks() { return [] }
    getVideoTracks() { return [] }
  }
  class PeerConnection {
    signalingState = 'stable'
    transceivers = []
    constructor() { pcs.push(this) }
    addTransceiver(kind) {
      const sender = { replaceTrack: (value) => replace?.(this, value) }
      this.transceivers.push({ kind, sender })
      return { sender }
    }
  }
  const { Mesh } = loadModule('lib/mesh.ts', {}, { MediaStream: Stream, RTCPeerConnection: PeerConnection, console })
  return { mesh: new Mesh('self', new Stream(), [], () => {}, () => {}), pcs }
}

test('a participant with no local devices still negotiates audio and video receivers', () => {
  const h = meshHarness()
  h.mesh.connect('remote')
  assert.deepEqual(h.pcs[0].transceivers.map((t) => t.kind), ['audio', 'video'])
})

test('a failed peer replacement waits for remaining peers before callers can restore', async () => {
  const late = deferred()
  let h
  h = meshHarness((pc) => pc === h.pcs[0] ? Promise.reject(new Error('failed')) : late.promise)
  h.mesh.connect('one')
  h.mesh.connect('two')
  let settled = false
  const result = h.mesh.setVideoTrack(track()).catch(() => { settled = true })
  await new Promise((resolve) => setImmediate(resolve))
  assert.equal(settled, false)
  late.resolve()
  await result
  assert.equal(settled, true)
})

test('a peer leaving during track replacement does not fail sharing for other peers', async () => {
  const h = meshHarness((pc) => {
    pc.signalingState = 'closed'
    return Promise.reject(new Error('peer closed'))
  })
  h.mesh.connect('departed')
  await h.mesh.setVideoTrack(track())
})
