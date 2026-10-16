#!/usr/bin/env node
// 실제 경로 확인: 가짜 마이크(음성 WAV) → 브라우저 VAD → Gemini Live → /ws stt → 서버 번역 → 자막.
// Gemini를 실제로 호출하므로 CI용이 아니다. scripts/dev.sh로 앱을 띄우고 GEMINI_API_KEY가 서버에 있어야 한다.
//
//   node web/tests/browser-stt.mjs speech.wav [--lang en] [--partner ko] [--timeout 30000]
//
// WAV는 16-bit PCM이면 된다(Chromium이 반복 재생한다). 앞뒤에 1초 이상 무음을 두면 VAD 구간이 분명해진다.
// CHROMIUM_PATH로 Playwright가 받은 것과 다른 Chromium을 쓸 수 있다.
// HTTPS_PROXY가 있으면 브라우저도 그 프록시를 쓴다. PROXY_IGNORE_CERT=1은 TLS를 가로채는 개발용 프록시에서만 쓴다.
import { readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const args = process.argv.slice(2)
const option = (name, fallback) => {
  const i = args.indexOf(name)
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback
}
const wav = args.find((a, i) => !a.startsWith('--') && !args[i - 1]?.startsWith('--'))
if (!wav) {
  console.error('usage: browser-stt.mjs speech.wav [--lang en] [--partner ko] [--timeout 30000]')
  process.exit(2)
}
const base = option('--base', 'http://localhost:5173')
const lang = option('--lang', 'en')
const partner = option('--partner', 'ko')
const timeout = Number(option('--timeout', '30000'))

const text = readFileSync(join(ROOT, 'server/.dev-host-password'), 'utf8')
const field = (name) => text.match(new RegExp(`^${name}:\\s*(.+)$`, 'm'))?.[1].trim()

const proxy = process.env.HTTPS_PROXY || process.env.https_proxy
const browser = await chromium.launch({
  executablePath: process.env.CHROMIUM_PATH || undefined,
  args: [
    '--use-fake-ui-for-media-stream',
    '--use-fake-device-for-media-stream',
    `--use-file-for-fake-audio-capture=${resolve(wav)}`,
    '--autoplay-policy=no-user-gesture-required',
  ],
  ...(proxy ? { proxy: { server: proxy, bypass: 'localhost,127.0.0.1' } } : {}),
})
const context = await browser.newContext({
  baseURL: base,
  permissions: ['camera', 'microphone'],
  ignoreHTTPSErrors: process.env.PROXY_IGNORE_CERT === '1',
})
await context.addInitScript(
  ([l, p]) => localStorage.setItem('parley.prefs', JSON.stringify({ name: 'STT Tester', lang: l, partnerLang: p })),
  [lang, partner],
)
// 단계별 시각을 잰다: Gemini 연결·준비, 말 시작, 첫 중간/확정 결과, stt 송신, 첫 번역/확정 자막.
await context.addInitScript(() => {
  const t0 = performance.now()
  const marks = (window.__sttMarks = [])
  const mark = (name) => marks.push([name, Math.round(performance.now() - t0)])
  const Native = window.WebSocket
  window.WebSocket = class extends Native {
    constructor(url, protocols) {
      super(url, protocols)
      const gemini = String(url).includes('generativelanguage')
      if (gemini) mark('gemini connect')
      this.addEventListener('message', (e) => {
        const raw = typeof e.data === 'string' ? e.data : new TextDecoder().decode(e.data)
        if (gemini) {
          if (raw.includes('setupComplete')) mark('gemini ready')
          if (raw.includes('interimInputTranscription')) mark('first interim')
          if (raw.includes('"inputTranscription"')) mark('first final transcript')
        } else if (raw.includes('"t":"caption"')) {
          if (raw.includes('"text":"')) mark('first translated caption')
          if (raw.includes('"final":true')) mark('first final caption')
        } else if (raw.includes('"t":"stt_token"')) mark('token received')
      })
      this.addEventListener('close', (e) => gemini && mark(`gemini closed ${e.code} ${e.reason}`))
    }
    send(data) {
      if (typeof data === 'string') {
        if (data.includes('activityStart')) mark('activityStart')
        if (data.includes('activityEnd')) mark('activityEnd')
        if (data.includes('"t":"stt_token"')) mark('token requested')
      }
      super.send(data)
    }
  }
})
const page = await context.newPage()
const errors = []
page.on('pageerror', (e) => errors.push(e.message))
page.on('console', (m) => m.type() === 'error' && errors.push(m.text()))

const login = await page.request.post('/api/login', { data: { username: field('username'), password: field('password') } })
if (!login.ok()) throw new Error(`login failed: HTTP ${login.status()}`)
const { code } = await (await page.request.post('/api/rooms', { data: { translation: true } })).json()
await page.goto(`/${code}`)
await page.locator('.lobby-form .btn-primary:not([disabled])').click({ timeout: 15000 })
const joinedAt = Date.now()
await page.locator('.room').waitFor({ timeout: 15000 })

let result = 'timeout'
let elapsed = 0
try {
  // 번역까지 확정된 자막 한 줄
  await page.locator('.feed .entry:not(.pending) p:nth-of-type(2)').first().waitFor({ timeout })
  result = 'ok'
  elapsed = Date.now() - joinedAt
} catch {
  // 아래에서 화면 상태를 출력한다
}
const entries = await page.locator('.feed .entry').evaluateAll((els) =>
  els.map((el) => ({ final: !el.classList.contains('pending'), lines: [...el.querySelectorAll('p')].map((p) => p.textContent) })),
)
const marks = await page.evaluate(() => window.__sttMarks)
const status = await page.locator('.stt-status').textContent().catch(() => null)
const shot = join(ROOT, 'shots', `stt-${lang}-${partner}.png`)
await page.screenshot({ path: shot })
await browser.close()

console.log(`${result}: first final caption ${result === 'ok' ? `${elapsed}ms after clicking join` : 'not seen'}`)
const seen = new Set()
for (const [name, ms] of marks) {
  // 반복되는 표시는 처음 것만
  if (seen.has(name) && !name.startsWith('activity')) continue
  seen.add(name)
  console.log(`  ${String(ms).padStart(6)}ms  ${name}`)
}
for (const e of entries) console.log(`  ${e.final ? 'final  ' : 'pending'} ${e.lines.join(' → ')}`)
if (status) console.log(`  stt status: ${status}`)
console.log(shot)
if (errors.length) console.log(`browser errors:\n  ${[...new Set(errors)].slice(0, 10).join('\n  ')}`)
process.exit(result === 'ok' ? 0 : 1)
