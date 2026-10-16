#!/usr/bin/env node
// 헤드리스 브라우저로 화면을 캡처한다. 브라우저 창이 없는 클라우드 세션에서 AI가 화면을 확인하는 용도.
//
//   pnpm --dir web shot /                     # 홈
//   pnpm --dir web shot --login / /login      # dev 계정으로 로그인한 뒤 캡처
//   pnpm --dir web shot --login @meeting      # 글로벌 미팅을 만들고 입장해 회의실까지 캡처
//   pnpm --dir web shot --mobile --locale ja /
//
// 옵션: --login, --mobile, --locale en|ko|ja, --out <dir>(기본 ../shots), --base <url>(기본 http://localhost:5173)
// 먼저 scripts/dev.sh 로 앱을 띄워야 한다. 카메라·마이크는 Chromium의 가짜 장치를 쓴다.
import { mkdirSync, readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const args = process.argv.slice(2)
const flag = (name) => args.includes(name)
const option = (name, fallback) => {
  const i = args.indexOf(name)
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback
}
const valued = new Set(['--locale', '--out', '--base'])
const targets = args.filter((a, i) => !a.startsWith('--') && !valued.has(args[i - 1]))
const base = option('--base', 'http://localhost:5173')
const outDir = resolve(ROOT, option('--out', 'shots'))
const locale = option('--locale', 'ko')
const mobile = flag('--mobile')

if (targets.length === 0) {
  console.error('usage: shot [--login] [--mobile] [--locale ko] [--out dir] <path|@meeting>...')
  process.exit(2)
}

/** server/.dev-host-password (dev 모드가 처음 실행될 때 만든다)에서 테스트 계정을 읽는다. */
function devCredentials() {
  const file = join(ROOT, 'server/.dev-host-password')
  let text
  try {
    text = readFileSync(file, 'utf8')
  } catch {
    throw new Error(`${file} 가 없습니다. scripts/dev.sh 로 dev 모드 서버를 먼저 띄우세요.`)
  }
  const field = (name) => text.match(new RegExp(`^${name}:\\s*(.+)$`, 'm'))?.[1].trim()
  return { username: field('username'), password: field('password') }
}

mkdirSync(outDir, { recursive: true })
const browser = await chromium.launch({
  args: ['--use-fake-ui-for-media-stream', '--use-fake-device-for-media-stream'],
})
const context = await browser.newContext({
  baseURL: base,
  viewport: mobile ? { width: 390, height: 844 } : { width: 1440, height: 900 },
  deviceScaleFactor: mobile ? 2 : 1,
  isMobile: mobile,
  permissions: ['camera', 'microphone'],
})
// 화면 언어·이름을 미리 정해 두어 결과가 매번 같게 한다.
await context.addInitScript(
  ([loc]) => {
    try {
      localStorage.setItem('tolkmeet.locale', loc)
      if (!localStorage.getItem('tolkmeet.prefs')) {
        localStorage.setItem('tolkmeet.prefs', JSON.stringify({ name: 'AI Tester', lang: loc, partnerLang: loc === 'ko' ? 'ja' : 'ko' }))
      }
    } catch {
      // 저장소가 막혀도 캡처는 계속한다
    }
  },
  [locale],
)

const page = await context.newPage()
const errors = []
page.on('pageerror', (e) => errors.push(e.message))
page.on('console', (m) => m.type() === 'error' && errors.push(m.text()))

if (flag('--login')) {
  const res = await page.request.post('/api/login', { data: devCredentials() })
  if (!res.ok()) throw new Error(`login failed: HTTP ${res.status()}`)
}

const suffix = `${mobile ? 'mobile' : 'desktop'}-${locale}`
const saved = []
async function capture(name) {
  await page.waitForLoadState('networkidle').catch(() => {})
  // 웹폰트가 늦게 오면 글자 모양이 달라지므로 폰트 로딩을 기다린다.
  await page.evaluate(() => document.fonts.ready)
  const file = join(outDir, `${name}-${suffix}.png`)
  await page.screenshot({ path: file, fullPage: true })
  saved.push(file)
}

for (const target of targets) {
  if (target === '@meeting') {
    // 글로벌 미팅을 만들고 로비 → 회의실까지 순서대로 캡처한다(--login 필요).
    const res = await page.request.post('/api/rooms', { data: { translation: true } })
    if (!res.ok()) throw new Error(`create meeting failed: HTTP ${res.status()} (--login 을 붙였나요?)`)
    const { code } = await res.json()
    await page.goto(`/${code}`)
    await page.locator('.lobby-form .btn-primary:not([disabled])').waitFor({ timeout: 15000 })
    await capture('lobby')
    await page.locator('.lobby-form .btn-primary').click()
    await page.locator('.room').waitFor({ timeout: 15000 })
    await page.waitForTimeout(1500)
    await capture('room')
    continue
  }
  await page.goto(target)
  const name = target === '/' ? 'home' : target.replace(/^\//, '').replace(/[^a-zA-Z0-9-]+/g, '_')
  await capture(name)
}

await browser.close()
for (const file of saved) console.log(file)
if (errors.length) {
  console.log(`\nbrowser errors (${errors.length}):`)
  for (const e of [...new Set(errors)].slice(0, 10)) console.log(`  ${e}`)
}
