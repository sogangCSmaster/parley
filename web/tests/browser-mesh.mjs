// Optional real-WebRTC regression check against a running local Vite server.
import assert from 'node:assert/strict'
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ?? 'playwright')
const browser = await chromium.launch({ headless: true })
try {
  const page = await browser.newPage()
  await page.goto(process.env.TEST_ORIGIN ?? 'http://127.0.0.1:5173')
  const results = await page.evaluate(async () => {
    const { Mesh } = await import('/src/lib/mesh.ts')
    const audio = new AudioContext()
    const oscillator = audio.createOscillator()
    const destination = audio.createMediaStreamDestination()
    oscillator.connect(destination)
    oscillator.start()
    const canvas = document.createElement('canvas')
    canvas.width = canvas.height = 64
    const local = new MediaStream([...destination.stream.getTracks(), ...canvas.captureStream(5).getTracks()])
    async function connect(empty) {
      const received = { a: [], b: [] }
      let a, b
      a = new Mesh('a', local, [], (_, data) => queueMicrotask(() => b.handleSignal('a', data)), (_, stream) => { received.a = stream.getTracks().map((t) => t.kind) })
      b = new Mesh('b', empty ? new MediaStream() : local, [], (_, data) => queueMicrotask(() => a.handleSignal('b', data)), (_, stream) => { received.b = stream.getTracks().map((t) => t.kind) })
      b.connect('a')
      const deadline = performance.now() + 10_000
      const connected = () => [...a.links.values(), ...b.links.values()].length === 2 && [...a.links.values(), ...b.links.values()].every((link) => link.pc.connectionState === 'connected')
      while (!connected() && performance.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 25))
      const result = { empty, received, connected: connected() }
      a.close()
      b.close()
      return result
    }
    const results = [await connect(true), await connect(false)]
    local.getTracks().forEach((t) => t.stop())
    await audio.close()
    return results
  })
  for (const result of results) {
    assert.equal(result.connected, true, JSON.stringify(result))
    assert.ok(result.received.b.includes('audio'), JSON.stringify(result))
    assert.ok(result.received.b.includes('video'), JSON.stringify(result))
    if (!result.empty) {
      assert.ok(result.received.a.includes('audio'), JSON.stringify(result))
      assert.ok(result.received.a.includes('video'), JSON.stringify(result))
    }
  }
  console.log(JSON.stringify(results))
} finally {
  await browser.close()
}
