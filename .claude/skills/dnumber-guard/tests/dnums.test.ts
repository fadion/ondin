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

type World = {
  last: number
  /** `git grep -o` lines, `path:D<n>`; the mock answers only those the pattern asks for. */
  cites: string
  edits: string[]
  /** `--show-toplevel`'s answer: the worktree the session is in. `C:/repo` when not given. */
  root?: string
  /** `--git-common-dir`'s answer: one per repository, shared by its worktrees. */
  common?: string
  /** Paths (as the Edit names them) that `git check-ignore` reports as ignored. */
  ignored?: string[]
  /** When set, every git call rejects: the guard's fail-open path. */
  broken?: boolean
}

/**
 * The engine beneath the plugin: a store in memory, git answered from `world`,
 * and an Edit that succeeds without touching anything.
 */
function engine(on: On, world: World, store?: Record<string, unknown>) {
  mock.store(on, store)
  on('process.run', (_$, e) => {
    if (world.broken) throw new Error('git is not there')
    const argv = e.argv.slice(1)
    const args = argv.join(' ')
    let stdout = ''
    let exitCode = 0
    if (args.includes('docs/decisions.md')) stdout = `- **D${world.last - 1}**\n- **D${world.last}**\n`
    else if (args.startsWith('grep --untracked')) {
      const asked = new Set((/D\(([\d|]+)\)/.exec(args)?.[1] ?? '').split('|'))
      stdout = world.cites
        .split('\n')
        .filter(l => asked.has(/:D(\d+)$/.exec(l)?.[1] ?? ''))
        .map(l => l + '\n')
        .join('')
      exitCode = stdout ? 0 : 1
    } else if (args === 'rev-parse --short HEAD') stdout = 'abc1234\n'
    else if (args === 'rev-parse --show-toplevel') stdout = `${world.root ?? 'C:/repo'}\n`
    else if (args === 'rev-parse --path-format=absolute --git-common-dir') stdout = `${world.common ?? 'C:/repo/.git'}\n`
    else if (argv[0] === 'check-ignore') exitCode = (world.ignored ?? []).includes(argv[argv.length - 1]!) ? 0 : 1
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

test('an entry a release reports as owed can be written, and the next block reserved once it is', async ($, on) => {
  // [X7-L1-03]: release deletes the block and reports D1025 cited with no
  // entry; the guard then refused the index row (no block) and reserve refused
  // the block (its grep finds D1025), each half's advice the other's refusal.
  const world: World = { last: 1024, cites: '', edits: [] }
  engine(on, world)
  await $.tool.call({ tool: RESERVE, action: 'reserve', count: 4 } as never)
  world.cites = 'crates/a.rs:D1025\n'
  const released = said(await $.tool.call({ tool: RESERVE, action: 'release' } as never))
  expect(released).toContain('Cited with no §15.0 entry: D1025')

  // The number is owed, not new: the entry goes through, and so does a second
  // citation of it; a number cited nowhere is still refused.
  const entry = { tool: 'Edit', file_path: 'C:/repo/docs/decisions.md', old_string: '- **D1024** — x', new_string: '- **D1024** — x\n- **D1025** — y' } as const
  const wrote = await $.tool.call(entry)
  expect(wrote.deny).toBeUndefined()
  expect(world.edits).toEqual([entry.new_string])
  const again = await $.tool.call({ tool: 'Edit', file_path: 'C:/repo/crates/b.rs', old_string: '// b', new_string: '// b (§15 D1025)' })
  expect(again.deny).toBeUndefined()
  const fresh = await $.tool.call({ tool: 'Edit', file_path: 'C:/repo/crates/b.rs', old_string: '// b', new_string: '// b (§15 D1026)' })
  expect(said(fresh)).toContain('types D1026')
  expect(world.edits.length).toBe(2)

  // Once the entry is in, the next block reserves.
  world.last = 1025
  const next = said(await $.tool.call({ tool: RESERVE, action: 'reserve', count: 3 } as never))
  expect(next).toContain('Reserved D1026–D1028')
})

test('a gitignored file is not guarded, since the greps never read it', async ($, on) => {
  // [X7-L1-07]: `git grep --untracked` honours .gitignore, so what the guard
  // refused there the census would never have counted.
  const world: World = { last: 1024, cites: '', edits: [], ignored: ['C:/repo/review/x.md', 'C:/repo/todo.md'] }
  engine(on, world)
  await $.tool.call({ tool: 'Edit', file_path: 'C:/repo/review/x.md', old_string: 'a', new_string: 'next free is D1025' })
  await $.tool.call({ tool: 'Edit', file_path: 'C:\\repo\\todo.md', old_string: 'a', new_string: 'D1025' })
  expect(world.edits.length).toBe(2)
  const tracked = await $.tool.call({ tool: 'Edit', file_path: 'C:/repo/docs/roadmap.md', old_string: 'a', new_string: 'D1025' })
  expect(said(tracked)).toContain('types D1025')
})

test('a block belongs to its worktree: another neither sees nor releases it, and one behind cannot collide with it', async ($, on) => {
  // [X7-L1-08]: the store is the user's, not the repository's.
  const world: World = { last: 1024, cites: '', edits: [] }
  engine(on, world)
  expect(said(await $.tool.call({ tool: RESERVE, action: 'reserve', count: 4 } as never))).toContain('Reserved D1025–D1028')

  // A second worktree of the same repository, at the same commit.
  world.root = 'C:/repo-wt'
  expect(said(await $.tool.call({ tool: RESERVE, action: 'status' } as never))).toContain('Nothing is reserved')
  expect(said(await $.tool.call({ tool: RESERVE, action: 'release' } as never))).toBe('Nothing is reserved.')
  const collide = said(await $.tool.call({ tool: RESERVE, action: 'reserve', count: 2 } as never))
  expect(collide).toContain('Not reserved')
  expect(collide).toContain('D1025–D1028')
  const edit = await $.tool.call({ tool: 'Edit', file_path: 'C:/repo-wt/crates/a.rs', old_string: 'a', new_string: 'D1025' })
  expect(said(edit)).toContain('no block is reserved')

  // The first worktree still holds its block, and releases it.
  world.root = 'C:/repo'
  expect(said(await $.tool.call({ tool: RESERVE, action: 'status' } as never))).toContain('D1025–D1028 is reserved')
  world.last = 1026
  world.cites = 'crates/a.rs:D1025\ncrates/a.rs:D1026\n'
  await $.tool.call({ tool: RESERVE, action: 'release' } as never)

  // A worktree whose decisions.md is behind what the repository has spent may
  // not reserve numbers already spent on the other side.
  world.root = 'C:/repo-old'
  world.last = 1024
  world.cites = ''
  const behind = said(await $.tool.call({ tool: RESERVE, action: 'reserve', count: 2 } as never))
  expect(behind).toContain('Not reserved')
  expect(behind).toContain('D1026')
})

test('a block stored before blocks were kept per worktree is still read, and released', async ($, on) => {
  // The store held one key, `block`, until [X7-L1-08]; a session that reserved
  // under it keeps its block across the reload that brings the fix.
  const world: World = { last: 1024, cites: '', edits: [] }
  engine(on, world, { block: { from: 1025, to: 1030, head: 'old1234', at: '' } })
  expect(said(await $.tool.call({ tool: RESERVE, action: 'status' } as never))).toContain('D1025–D1030 is reserved')
  const edit = await $.tool.call({ tool: 'Edit', file_path: 'C:/repo/crates/a.rs', old_string: 'a', new_string: 'D1025' })
  expect(edit.deny).toBeUndefined()
  expect(said(await $.tool.call({ tool: RESERVE, action: 'release' } as never))).toContain('Released D1025–D1030')
  expect(said(await $.tool.call({ tool: RESERVE, action: 'status' } as never))).toContain('Nothing is reserved')
})

test('the guard fails open: when git cannot be asked, the edit runs, once', async ($, on) => {
  // [X7-L6-01]: the `.catch` in register.ts is a decision, so it is pinned.
  const world: World = { last: 1024, cites: '', edits: [], broken: true }
  engine(on, world)
  const r = await $.tool.call({ tool: 'Edit', file_path: 'C:/repo/crates/a.rs', old_string: 'a', new_string: 'D1099' })
  expect(r.deny).toBeUndefined()
  expect(world.edits).toEqual(['D1099'])
})
