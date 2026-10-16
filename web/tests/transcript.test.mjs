import assert from 'node:assert/strict'
import test from 'node:test'
import { loadModule } from './load-module.mjs'

const { Transcript, MAX_TRANSCRIPT, formatTranscript, transcriptFileName } = loadModule('lib/transcript.ts', {}, { Map, Date })
const msg = (seg, rev, final, src, text = null) => ({
  t: 'caption', seg, speaker: 'p1', src_lang: 'ja', src, tgt_lang: 'ko', text, final, rev,
})

test('transcript keeps finalized lines in the order speech started, with the latest revision', () => {
  const tr = new Transcript()
  tr.add(msg('a', 1, false, 'はい'), 1000)
  tr.add(msg('b', 1, true, '予算', '예산'), 2000)
  tr.add(msg('a', 3, true, 'はい、ありがとう', '네, 감사합니다'), 3000)
  tr.add(msg('a', 2, true, 'はい', '네'), 4000) // 늦게 온 낮은 rev
  tr.add(msg('b', 2, true, '予算について', '예산에 대해'), 5000)
  const lines = tr.entries()
  assert.deepEqual(JSON.parse(JSON.stringify(lines.map((l) => [l.seg, l.src, l.at]))), [
    ['a', 'はい、ありがとう', 1000],
    ['b', '予算について', 2000],
  ])
})

test('pending lines are not saved and a retraction removes a line', () => {
  const tr = new Transcript()
  tr.add(msg('a', 1, false, 'えっと'), 0)
  assert.equal(tr.size, 0)
  tr.add(msg('a', 2, true, 'えっと'), 0)
  tr.add(msg('a', 3, true, ''), 0)
  assert.equal(tr.size, 0)
})

test('transcript drops the oldest lines beyond the cap', () => {
  const tr = new Transcript()
  for (let i = 0; i < MAX_TRANSCRIPT + 3; i++) tr.add(msg(`s${i}`, 1, true, `${i}`), i)
  assert.equal(tr.size, MAX_TRANSCRIPT)
  assert.equal(tr.entries()[0].src, '3')
})

test('transcript text shows speaker, time, original and translation', () => {
  const lines = [
    { seg: 'a', speaker: 'me', at: Date.UTC(2026, 9, 16, 13, 5), src_lang: 'ko', src: '안녕하세요', tgt_lang: 'ja', text: 'こんにちは' },
    { seg: 'b', speaker: 'p2', at: Date.UTC(2026, 9, 16, 13, 6), src_lang: 'ja', src: 'はい', tgt_lang: 'ja', text: null },
  ]
  const text = formatTranscript(lines, {
    title: 'Tolkmeet 회의 자막', code: 'abcd-efgh-ijkm', savedAt: new Date(Date.UTC(2026, 9, 16, 14)), locale: 'ko',
    speakerName: (id) => (id === 'me' ? '유승재 (나)' : 'Sakura'),
  })
  assert.match(text, /^Tolkmeet 회의 자막 · abcd-efgh-ijkm\n/)
  assert.match(text, /\[\d\d:05\] 유승재 \(나\)\n\(ko\) 안녕하세요\n\(ja\) こんにちは/)
  assert.match(text, /\[\d\d:06\] Sakura\n\(ja\) はい\n\n?$/, 'no translation row when there is none')
})

test('file name uses the meeting code and local time', () => {
  assert.equal(transcriptFileName('abcd-efgh-ijkm', new Date(2026, 9, 16, 9, 5)), 'tolkmeet-abcd-efgh-ijkm-20261016-0905.txt')
})

test('a meeting that crosses midnight shows dates where the day changes', () => {
  const base = new Date(2026, 9, 16, 23, 59).getTime()
  const lines = Array.from({ length: MAX_TRANSCRIPT }, (_, i) => ({
    seg: `s${i}`, speaker: 'p', at: base + i * 100, src_lang: 'ko', src: '안녕하세요', tgt_lang: 'ja', text: 'こんにちは',
  }))
  // 저장은 마지막 줄 뒤(다음 날)에 한다.
  const savedAt = new Date(base + MAX_TRANSCRIPT * 100 + 1000)
  const t0 = performance.now()
  const text = formatTranscript(lines, { title: 'T', code: 'c', savedAt, locale: 'en', speakerName: () => 'P' })
  const ms = performance.now() - t0
  assert.match(text, /\[Oct 16 23:59\] P/, 'the first line is from the day before the header date')
  assert.match(text, /\[Oct 17 00:00\] P/, 'the first line of the new day shows the date')
  assert.equal((text.match(/\[Oct 1[67] /g) ?? []).length, 2, 'dates only where the day changes')
  // 줄마다 포매터를 새로 만들면 100ms를 넘는다(느린 CI를 감안해 넉넉히).
  assert.ok(ms < 200, `formatted 5000 lines in ${ms.toFixed(1)}ms`)
})
