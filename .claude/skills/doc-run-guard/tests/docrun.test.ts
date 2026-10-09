import { describe as group, expect, test } from 'claude-code/testing'
import { compare } from '../hooks/docrun.ts'

const VICTIM = [
  '/// Adds two numbers.',
  '/// Overflow wraps.',
  '#[inline]',
  'fn add(a: i32, b: i32) -> i32 { a + b }',
  '',
].join('\n')

group('compare', () => {
  test('an insertion anchored on the item line takes its doc', async () => {
    // The commonest of CLAUDE.md's instances: the Edit anchored on `fn add(`,
    // so the new function lands between `add` and its doc and takes it.
    const after = VICTIM.replace('#[inline]\nfn add(', '#[inline]\nfn helper() {}\n\nfn add(')
    expect(compare(VICTIM, after)).toEqual([
      { kind: 'lost', item: 'fn add(a: i32, b: i32) -> i32 { a + b }', lines: 2, nowAbove: 'fn helper() {}' },
    ])
  })

  test('the same insertion anchored above the doc says nothing', async () => {
    const after = 'fn helper() {}\n\n' + VICTIM
    expect(compare(VICTIM, after)).toEqual([])
  })

  test('an insertion inside the run carries the summary line off', async () => {
    // §15 D805's shape: the victim is still documented, but its doc now opens
    // mid-argument and its summary heads the new item's doc.
    const before = ['/// Summary of victim.', '/// More about it.', '/// Even more.', 'fn victim() {}', ''].join('\n')
    const after = [
      '/// Summary of victim.',
      '/// New doc.',
      'fn new_one() {}',
      '/// More about it.',
      '/// Even more.',
      'fn victim() {}',
      '',
    ].join('\n')
    expect(compare(before, after)).toEqual([
      { kind: 'summary-moved', item: 'fn victim() {}', summary: '/// Summary of victim.', nowAbove: 'fn new_one() {}' },
    ])
  })

  test('two runs exchanged are a swap, though neither item lost a doc', async () => {
    const before = ['/// About a.', '#[test]', 'fn a() {}', '', '/// About b.', '#[test]', 'fn b() {}', ''].join('\n')
    const after = ['/// About b.', '#[test]', 'fn a() {}', '', '/// About a.', '#[test]', 'fn b() {}', ''].join('\n')
    expect(compare(before, after).map(f => f.kind)).toEqual(['swapped', 'swapped'])
  })

  test('a run copied beside the original is a duplication', async () => {
    const doc = ['/// One.', '/// Two.', '/// Three.']
    const before = [...doc, 'fn a() {}', ''].join('\n')
    const after = [...doc, 'fn a() {}', '', ...doc, 'fn b() {}', ''].join('\n')
    expect(compare(before, after)).toEqual([{ kind: 'duplicated', item: 'fn a() {}', lines: 3 }])
  })

  test('rewriting a doc is not a theft', async () => {
    const after = VICTIM.replace('/// Overflow wraps.', '/// Overflow panics in debug.')
    expect(compare(VICTIM, after)).toEqual([])
  })

  test('a CRLF file reads as an LF one', async () => {
    const crlf = VICTIM.replace(/\n/g, '\r\n')
    const after = crlf.replace('#[inline]\r\nfn add(', '#[inline]\r\nfn helper() {}\r\n\r\nfn add(')
    expect(compare(crlf, after).map(f => f.kind)).toEqual(['lost'])
  })

  test('an attribute spanning several lines sits between a doc and its item', async () => {
    // Found replaying the history: `#[cfg(any(` over three lines read its
    // continuations as the item, and reported four tests as robbed.
    const attr = ['/// Doc.', '#[cfg(any(', '    target_os = "linux",', '    target_os = "macos",', '))]', '#[test]', 'fn t() {}', '']
    const before = attr.join('\n')
    const after = ['fn other() {}', '', ...attr].join('\n')
    expect(compare(before, after)).toEqual([])
    const stolen = before.replace('#[test]\nfn t() {}', '#[test]\nfn helper() {}\n\nfn t() {}')
    expect(compare(before, stolen).map(f => f.kind)).toEqual(['lost'])
  })

  test('an item whose line is not unique is left alone rather than guessed', async () => {
    const before = ['/// First.', 'fn new() {}', '', '/// Second.', 'fn new() {}', ''].join('\n')
    const after = ['fn new() {}', '', '/// Second.', 'fn new() {}', ''].join('\n')
    expect(compare(before, after)).toEqual([])
  })
})

test('an Edit that takes a doc comes back with a reminder for the model', async ($, on) => {
  // The test's hooks stand for the engine: fs.read answers from `file`, and
  // the Edit applies its replacement to it, so the plugin reads the file on
  // both sides of the edit as it would in a session.
  let file = VICTIM
  on('fs.read', () => ({ value: file }))
  on('tool.call', { tool: 'Edit' }, (_$, e) => {
    if (e.tool !== 'Edit') throw new Error('not an Edit')
    file = file.replace(e.old_string, e.new_string)
    return { result: { filePath: e.file_path } as never }
  })

  const ran = await $.tool.call({
    tool: 'Edit',
    file_path: 'C:/repo/src/lib.rs',
    old_string: '#[inline]\nfn add(',
    new_string: '#[inline]\nfn helper() {}\n\nfn add(',
  })
  expect(ran.context?.length).toBe(1)
  expect(ran.context?.[0]).toContain('lost its 2-line `///` run, which now sits above `fn helper() {}`')
})

test('a clean Edit and a non-Rust file come back untouched', async ($, on) => {
  let file = VICTIM
  on('fs.read', () => ({ value: file }))
  on('tool.call', { tool: 'Edit' }, (_$, e) => {
    if (e.tool !== 'Edit') throw new Error('not an Edit')
    file = file.replace(e.old_string, e.new_string)
    return { result: { filePath: e.file_path } as never }
  })
  const clean = await $.tool.call({ tool: 'Edit', file_path: 'C:/repo/src/lib.rs', old_string: '/// Adds', new_string: 'fn helper() {}\n\n/// Adds' })
  expect(clean.context).toBeUndefined()

  file = VICTIM
  const md = await $.tool.call({ tool: 'Edit', file_path: 'C:/repo/notes.md', old_string: '#[inline]\nfn add(', new_string: '#[inline]\nfn helper() {}\n\nfn add(' })
  expect(md.context).toBeUndefined()
})
