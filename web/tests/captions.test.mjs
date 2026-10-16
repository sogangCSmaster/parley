import assert from 'node:assert/strict'
import test from 'node:test'
import { loadModule } from './load-module.mjs'

const { applyCaption } = loadModule('lib/captions.ts')
const pending = {
  t: 'caption', seg: 'speaker-0', speaker: 'speaker', src_lang: 'en',
  src: 'hello', tgt_lang: 'ko', text: null, final: false, rev: 1,
}

test('cancelled captions retain their revision so late translations cannot restore them', () => {
  const initial = applyCaption([], pending, 100)
  const cancelled = applyCaption(initial, { ...pending, src: '', final: true, rev: 3 }, 200)
  assert.equal(cancelled[0].src, '')
  assert.equal(cancelled[0].final, true)
  assert.equal(cancelled[0].at, 100)
  assert.equal(applyCaption(cancelled, { ...pending, text: '안녕', rev: 2 }, 300), cancelled)
})

test('cancellation records share the bounded caption history', () => {
  let captions = []
  for (let i = 0; i < 250; i++) {
    captions = applyCaption(captions, { ...pending, seg: `speaker-${i}`, src: '', final: true }, i)
  }
  assert.equal(captions.length, 200)
  assert.equal(captions[0].seg, 'speaker-50')
})
