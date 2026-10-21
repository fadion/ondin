import { describe as group, expect, test } from 'claude-code/testing'
import { MAX_LINES, refusal } from '../hooks/archive.ts'

group('refusal', () => {
  test('a whole read of the ledger is refused, in either path spelling', async () => {
    expect(refusal('docs/archive/ledger.md', undefined)).toBeDefined()
    expect(refusal('C:\\Users\\x\\ondin\\docs\\archive\\ledger.md', undefined)).toBeDefined()
  })

  test('a ranged read of the ledger runs, up to the cap', async () => {
    expect(refusal('docs/archive/ledger.md', 120)).toBeUndefined()
    expect(refusal('docs/archive/ledger.md', MAX_LINES)).toBeUndefined()
    expect(refusal('docs/archive/ledger.md', MAX_LINES + 1)).toBeDefined()
  })

  test('every other path runs whole', async () => {
    expect(refusal('docs/decisions.md', undefined)).toBeUndefined()
    expect(refusal('docs/archived-notes.md', undefined)).toBeUndefined()
  })
})
