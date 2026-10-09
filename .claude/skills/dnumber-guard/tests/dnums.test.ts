import type { On } from 'claude-code'
import { describe as group, expect, mock, test } from 'claude-code/testing'
import { cited, introduced, isExcluded, lastIndex, unreserved } from '../hooks/dnums.ts'

group('the rules', () => {
  test('a hex literal is not a citation', async () => {
    // CLAUDE.md's census trap: `IdSource::new(0xD0)` read as a dangling D0.
    expect([...cited('IdSource::new(0xD0); see §15 D832 and D9.')]).toEqual([832, 9])
  })

  test('an edit introduces only what its old text did not cite', async () => {
    expect(introduced('per D10 and D20', 'per D10, D20 and D30')).toEqual([30])
  })

  test('the last index row is the largest, whatever order the rows come in', async () => {
    expect(lastIndex('- **D9**\n- **D1024**\n- **D110**\n')).toBe(1024)
  })

  test('an existing entry may be cited; a number past it needs the block', async () => {
    const block = { from: 1025, to: 1027, head: 'abc', at: '' }
    expect(unreserved([5, 1024, 1025, 1027, 1028], 1024, block)).toEqual([1028])
    expect(unreserved([1025], 1024, undefined)).toEqual([1025])
  })

  test('CLAUDE.md and this mod are excluded, by any spelling of the path', async () => {
    expect(isExcluded('C:\\Repo\\CLAUDE.md', 'C:/repo')).toBe(true)
    expect(isExcluded('C:/repo/.claude/skills/dnumber-guard/tests/x.ts', 'C:/repo')).toBe(true)
    expect(isExcluded('C:/repo/docs/CLAUDE.md.bak', 'C:/repo')).toBe(false)
    expect(isExcluded('C:/repo/crates/a.rs', 'C:/repo')).toBe(false)
  })
})

/**
 * The engine beneath the plugin: a store in memory, git answered from `world`,
 * and an Edit that succeeds without touching anything.
 */
function engine(on: On, world: { last: number; cites: string; edits: string[] }) {
  mock.store(on)
  on('process.run', (_$, e) => {
    const args = e.argv.slice(1).join(' ')
    let stdout = ''
    let exitCode = 0
    if (args.includes('docs/decisions.md')) stdout = `- **D${world.last - 1}**\n- **D${world.last}**\n`
    else if (args.startsWith('grep --untracked')) {
      stdout = world.cites
      exitCode = world.cites ? 0 : 1
    } else if (args === 'rev-parse --short HEAD') stdout = 'abc1234\n'
    else if (args === 'rev-parse --show-toplevel') stdout = 'C:/repo\n'
    return { value: { exitCode, stdout, stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
  })
  on('tool.call', { tool: 'Edit' }, (_$, e) => {
    if (e.tool === 'Edit') world.edits.push(e.new_string)
    return { result: {} as never }
  })
  // The status line has no engine beneath a test; answer it so it is not noise.
  on('ui.status', () => ({ value: undefined }))
}

const RESERVE = 'mcp__dnumber-guard__reserve'

/**
 * What a call said, whichever way it came back: a refusal's reason, core's
 * `text`, or the mod's own tool's `result`, which carries no `text`.
 */
function said(r: { deny?: string; text?: string; result?: unknown }): string {
  return r.deny ?? r.text ?? String(r.result)
}

test('an unreserved number is refused, and the same edit goes through once reserved', async ($, on) => {
  const world = { last: 1024, cites: '', edits: [] as string[] }
  engine(on, world)
  const edit = { tool: 'Edit', file_path: 'C:/repo/crates/a.rs', old_string: '// x', new_string: '// x (§15 D1025)' } as const

  const refused = await $.tool.call(edit)
  expect(said(refused)).toContain('types D1025, past §15.0\'s last entry (D1024), and no block is reserved')
  expect(world.edits).toEqual([])

  const reserved = await $.tool.call({ tool: RESERVE, action: 'reserve', count: 3 } as never)
  expect(said(reserved)).toContain('Reserved D1025–D1027 at abc1234')

  const done = await $.tool.call(edit)
  expect(done.deny ?? done.isError).toBeUndefined()
  expect(world.edits).toEqual(['// x (§15 D1025)'])

  const beyond = await $.tool.call({ ...edit, new_string: '// x (§15 D1028)' })
  expect(said(beyond)).toContain('the reserved block is D1025–D1027')
  expect(world.edits.length).toBe(1)
})

test('a block whose numbers are already typed somewhere is not reserved', async ($, on) => {
  engine(on, { last: 1024, cites: 'crates/a.rs:D1026\n', edits: [] })
  const r = said(await $.tool.call({ tool: RESERVE, action: 'reserve', count: 3 } as never))
  expect(r).toContain('Not reserved')
  expect(r).toContain('- D1026: crates/a.rs')
})

test('release reports what was spent, what goes back, and an entry still owed', async ($, on) => {
  const world = { last: 1024, cites: '', edits: [] as string[] }
  engine(on, world)
  await $.tool.call({ tool: RESERVE, action: 'reserve', count: 4 } as never)

  // D1025 got its entry (last is now 1025); D1027 is cited with none, which
  // also leaves D1026 a gap below it.
  world.last = 1025
  world.cites = 'crates/a.rs:D1025\ndocs/decisions.md:D1025\ncrates/b.rs:D1027\n'
  const r = said(await $.tool.call({ tool: RESERVE, action: 'release' } as never))
  expect(r).toContain('- D1025: crates/a.rs, docs/decisions.md')
  expect(r).toContain('Released at zero sites: D1026, D1028.')
  expect(r).toContain('Cited with no §15.0 entry: D1027')
  expect(r).toContain('Unspent below a spent number: D1026')

  const after = said(await $.tool.call({ tool: RESERVE, action: 'status' } as never))
  expect(after).toContain('Nothing is reserved. §15.0\'s last entry is D1025')
})

test('CLAUDE.md and files outside the repository are not guarded', async ($, on) => {
  const world = { last: 1024, cites: '', edits: [] as string[] }
  engine(on, world)
  await $.tool.call({ tool: 'Edit', file_path: 'C:/repo/CLAUDE.md', old_string: 'a', new_string: 'D1030–D1040 released' })
  await $.tool.call({ tool: 'Edit', file_path: 'C:/elsewhere/notes.md', old_string: 'a', new_string: 'D1030' })
  expect(world.edits.length).toBe(2)
})
