import type { EngineInterface, Register, ToolCallInput, ToolCallResult } from 'claude-code'
import { type Block, EXCLUDED, introduced, isExcluded, lastIndex, members, span, unreserved } from './dnums.ts'

// CLAUDE.md's "Reserve the *block*, before writing a single citation", made a
// check instead of a habit. Its record has a number typed into a comment ahead
// of its reservation at least five times, each caught (when it was) by a grep
// run afterwards; nothing could refuse it at the keystroke.
//
// Two halves:
// - a tool, `mcp__dnumber-guard__reserve`, that takes the next numbers after
//   §15.0's last index row, runs the negative grep over them, and records the
//   block; and at the request's end releases it with the closing grep, which
//   says which numbers were spent where and which go back;
// - a guard on Edit and Write that refuses any edit typing a D-number past
//   §15.0's last entry and outside the reserved block — unless the number is
//   already cited elsewhere in the tree, so owed rather than new: the entry a
//   release reports as missing must be writable with no block, since reserve
//   refuses while that citation stands ([X7-L1-03] in the v0.5.0 release
//   review). A file git ignores is not guarded, as neither grep reads it.
//
// The guard fails open: if git cannot be asked, the edit goes through, since a
// broken guard refusing every edit is worse than the habit it backs up.
//
// The store is the user's, not the repository's, so a block is kept per
// worktree (`block:<toplevel>`): a session in another worktree, or another
// clone, neither sees nor releases it. Worktrees of one repository still meet
// in two places: reserve refuses a range that overlaps a block another
// worktree of the same repository holds, and refuses in a tree whose §15.0
// ends below the highest number the repository has spent (`reached:<common
// dir>`, written at release) — the worktree checked out at an older commit.

/** The one key every block was kept under before blocks were kept per worktree. */
const LEGACY = 'block'
const TOOL = 'reserve'

const normal = (p: string) => p.replace(/\\/g, '/').toLowerCase().replace(/\/+$/, '')
const blockKey = (root: string) => `block:${normal(root)}`
const reachedKey = (common: string) => `reached:${normal(common)}`

/** The worktree the session is in and the repository it belongs to. */
type Tree = { root: string; common: string }

async function git($: EngineInterface, args: string[]): Promise<{ code: number; out: string }> {
  const r = await $.process.run(['git', ...args], { timeoutMs: 60_000 })
  return { code: r.exitCode, out: r.stdout }
}

async function last($: EngineInterface): Promise<number> {
  const { out } = await git($, ['grep', '-h', '-o', '-E', '^- \\*\\*D[0-9]+\\*\\*', '--', 'docs/decisions.md'])
  const n = lastIndex(out)
  if (n === 0) throw new Error('§15.0 has no index rows in docs/decisions.md')
  return n
}

/** Where each of `numbers` is cited in the tree, tracked and untracked, CLAUDE.md aside. */
async function citations($: EngineInterface, numbers: readonly number[]): Promise<Map<number, Set<string>>> {
  const found = new Map<number, Set<string>>()
  if (numbers.length === 0) return found
  const pattern = `D(${numbers.join('|')})`
  const excludes = EXCLUDED.map(p => `:(exclude)${p}`)
  const { out } = await git($, ['grep', '--untracked', '-o', '-w', '-E', pattern, '--', '.', ...excludes])
  for (const line of out.split('\n')) {
    const m = /^(.*):D(\d+)\r?$/.exec(line)
    if (!m) continue
    const n = Number(m[2])
    if (!found.has(n)) found.set(n, new Set())
    found.get(n)!.add(m[1]!)
  }
  return found
}

async function tree($: EngineInterface): Promise<Tree | undefined> {
  const top = await git($, ['rev-parse', '--show-toplevel'])
  const root = top.out.trim()
  if (top.code !== 0 || root === '') return undefined
  const c = await git($, ['rev-parse', '--path-format=absolute', '--git-common-dir'])
  const common = c.code === 0 && c.out.trim() !== '' ? c.out.trim() : root
  return { root, common }
}

const asBlock = (v: unknown) => (v && typeof v === 'object' ? (v as Block) : undefined)

/**
 * This worktree's block and the key it is stored under. A block stored under
 * the old single key is still this session's until it is released, so a
 * session that reserved before the reload that brought per-worktree keys
 * keeps its block.
 */
async function readBlock($: EngineInterface, t: Tree): Promise<{ block: Block; key: string } | undefined> {
  const own = asBlock(await $.store.get(blockKey(t.root)))
  if (own) return { block: own, key: blockKey(t.root) }
  const legacy = asBlock(await $.store.get(LEGACY))
  return legacy ? { block: legacy, key: LEGACY } : undefined
}

/** The blocks other worktrees of the same repository hold. */
async function otherBlocks($: EngineInterface, t: Tree): Promise<Block[]> {
  const out: Block[] = []
  for (const key of await $.store.keys()) {
    if (!key.startsWith('block:') || key === blockKey(t.root)) continue
    const b = asBlock(await $.store.get(key))
    if (b?.common !== undefined && normal(b.common) === normal(t.common)) out.push(b)
  }
  return out
}

function showStatus($: EngineInterface, block: Block | undefined) {
  $.ui.status(block ? `§15 ${span(block.from, block.to)} reserved` : undefined)
}

const listSites = (found: Map<number, Set<string>>) =>
  [...found].sort(([a], [b]) => a - b).map(([n, paths]) => `- D${n}: ${[...paths].join(', ')}`)

const alreadyHeld = (held: Block) =>
  `A block is already reserved: ${span(held.from, held.to)} (at ${held.head}, ${held.at}). Release it first (action "release"), then reserve again.`

async function reserve($: EngineInterface, count: number): Promise<string> {
  const t = await tree($)
  if (!t) return 'Not reserved: git cannot say which worktree this session is in.'
  const held = await readBlock($, t)
  if (held) return alreadyHeld(held.block)
  const lastEntry = await last($)
  const from = lastEntry + 1
  const to = from + count - 1
  const others = (await otherBlocks($, t)).filter(b => b.to >= from)
  if (others.length > 0) {
    return `Not reserved: another worktree of this repository holds ${others.map(b => `${span(b.from, b.to)} (${b.root})`).join(', ')}, and this tree's next numbers (from D${from}) would collide with it. Release it there, or bring this tree's docs/decisions.md up to date first.`
  }
  const found = await citations($, members({ from, to }))
  if (found.size > 0) {
    return [
      `Not reserved: the negative grep over ${span(from, to)} found sites, so a number past §15.0's last entry (D${lastEntry}) is already typed somewhere with no entry behind it. Write the entry (the guard lets a number already cited in the tree through) or remove the citation first:`,
      ...listSites(found),
    ].join('\n')
  }
  const reached = Number((await $.store.get(reachedKey(t.common))) ?? 0)
  if (lastEntry < reached) {
    return `Not reserved: §15.0 here ends at D${lastEntry}, but this repository has spent up to D${reached} — a worktree behind the one that spent them. Bring this tree's docs/decisions.md up to date, then reserve.`
  }
  const { out } = await git($, ['rev-parse', '--short', 'HEAD'])
  const token = `${Date.now()}-${Math.random()}`
  const block: Block = { from, to, head: out.trim(), at: new Date().toISOString(), root: t.root, common: t.common, token }
  // The store has no compare-and-set: look again right before writing, so the
  // window in which a second session in this worktree can reserve the same
  // numbers is the write itself rather than the grep above, and read the
  // write back so a session that lost that race is told so.
  const raced = await readBlock($, t)
  if (raced) return alreadyHeld(raced.block)
  await $.store.set(blockKey(t.root), block)
  const back = asBlock(await $.store.get(blockKey(t.root)))
  if (back?.token !== token) {
    return `Not reserved: another session in this worktree reserved at the same moment${back ? ` (${span(back.from, back.to)})` : ''}. Ask "status", and spend only a block this session reserved.`
  }
  showStatus($, block)
  return `Reserved ${span(from, to)} at ${block.head}: §15.0's last entry is D${lastEntry}, and the negative grep over the block found zero sites (${EXCLUDED.join(', ')} excluded). Spend only these numbers; release the block at the end of the request with action "release".`
}

async function release($: EngineInterface): Promise<string> {
  const t = await tree($)
  if (!t) return 'Not released: git cannot say which worktree this session is in.'
  const held = await readBlock($, t)
  if (!held) return 'Nothing is reserved.'
  const { block, key } = held
  const lastEntry = await last($)
  const numbers = members(block)
  const found = await citations($, numbers)
  const spent = numbers.filter(n => found.has(n))
  const unspent = numbers.filter(n => !found.has(n))
  const unresolved = spent.filter(n => n > lastEntry)
  const gaps = unspent.filter(n => spent.some(s => s > n))

  await $.store.delete(key)
  const common = block.common ?? t.common
  const reached = Math.max(Number((await $.store.get(reachedKey(common))) ?? 0), lastEntry, ...spent)
  await $.store.set(reachedKey(common), reached)
  showStatus($, undefined)

  const lines = [`Released ${span(block.from, block.to)} (reserved at ${block.head}). §15.0's last entry is now D${lastEntry}.`]
  lines.push(spent.length ? 'Spent:' : 'Spent: none.')
  if (spent.length) lines.push(...listSites(found))
  lines.push(unspent.length ? `Released at zero sites: ${unspent.map(n => `D${n}`).join(', ')}.` : 'Every number was spent.')
  if (unresolved.length) {
    lines.push(`⚠️ Cited with no §15.0 entry: ${unresolved.map(n => `D${n}`).join(', ')} — write the entries (the guard lets a number already cited in the tree through, so no block is needed for them); the census's unresolved half will show them.`)
  }
  if (gaps.length) {
    lines.push(`⚠️ Unspent below a spent number: ${gaps.map(n => `D${n}`).join(', ')} — §15 runs from D1 with no gaps, so these must be spent or the later ones renumbered.`)
  }
  lines.push("CLAUDE.md's live D-number figures are the session's to update.")
  return lines.join('\n')
}

async function status($: EngineInterface): Promise<string> {
  const t = await tree($)
  if (!t) return 'git cannot say which worktree this session is in.'
  const block = (await readBlock($, t))?.block
  const lastEntry = await last($)
  return block
    ? `${span(block.from, block.to)} is reserved (at ${block.head}, ${block.at}). §15.0's last entry is D${lastEntry}.`
    : `Nothing is reserved. §15.0's last entry is D${lastEntry}, so the next free number is D${lastEntry + 1}.`
}

/**
 * Whether git ignores the file: `git grep --untracked`, which both greps run,
 * honours .gitignore, so a number typed into an ignored file (`review/`,
 * `todo.md`) is one the census never counts, and the guard leaves it alone.
 */
async function ignored($: EngineInterface, path: string): Promise<boolean> {
  const { code } = await git($, ['check-ignore', '-q', '--', path.replace(/\\/g, '/')])
  return code === 0
}

async function guard(
  $: EngineInterface,
  e: ToolCallInput,
  next: (e: ToolCallInput) => Promise<ToolCallResult>,
): Promise<ToolCallResult> {
  let before = ''
  let after = ''
  if (e.tool === 'Edit') {
    before = e.old_string
    after = e.new_string
  } else if (e.tool === 'Write') {
    after = e.content
    try {
      before = await $.fs.read(e.file_path)
    } catch {
      before = ''
    }
  } else {
    return next(e)
  }
  const t = await tree($)
  if (!t || !normal(e.file_path).startsWith(normal(t.root) + '/')) return next(e)
  if (isExcluded(e.file_path, t.root)) return next(e)

  const typed = introduced(before, after)
  if (typed.length === 0) return next(e)
  if (await ignored($, e.file_path)) return next(e)
  const lastEntry = await last($)
  const block = (await readBlock($, t))?.block
  const past = unreserved(typed, lastEntry, block)
  if (past.length === 0) return next(e)
  // A number already cited elsewhere in the tree is owed, not new: the entry a
  // release reported as missing has to be writable without a block, or reserve
  // (whose grep finds that citation) and this guard each refuse what the
  // other's advice asks for.
  const owed = await citations($, past)
  const bad = past.filter(n => !owed.has(n))
  if (bad.length === 0) return next(e)

  const where = block ? `the reserved block is ${span(block.from, block.to)}` : 'no block is reserved'
  return {
    deny: `dnumber-guard: this edit types ${bad.map(n => `D${n}`).join(', ')}, past §15.0's last entry (D${lastEntry}), and ${where}. CLAUDE.md: reserve the block before writing a single citation — call mcp__dnumber-guard__reserve (action "reserve"), then make the edit.`,
  }
}

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    await $.tool.register({
      name: TOOL,
      description:
        'Reserve, inspect or release a block of §15 D-numbers (docs/decisions.md). action "reserve" takes the next `count` numbers (default 10) after §15.0\'s last index row and runs the negative grep over them; "status" shows the block and the next free number; "release" runs the closing grep at the end of a request, reports which numbers were spent where, and frees the rest. Edits that type a D-number past the last entry and outside the block are refused until one is reserved.',
      inputSchema: {
        type: 'object',
        properties: {
          action: { type: 'string', enum: ['reserve', 'status', 'release'] },
          count: { type: 'integer', minimum: 1, maximum: 40 },
        },
        required: ['action'],
      },
      isDeferred: false,
    })
    const t = await tree($).catch(() => undefined)
    showStatus($, t ? (await readBlock($, t))?.block : undefined)
    return next(e)
  })

  on('tool.call', { tool: 'mcp__dnumber-guard__reserve' }, async ($, e) => {
    const input = e as unknown as { action?: string; count?: number }
    const action = input.action ?? 'status'
    const text =
      action === 'reserve'
        ? await reserve($, Math.max(1, Math.min(40, Math.trunc(input.count ?? 10))))
        : action === 'release'
          ? await release($)
          : await status($)
    return { result: text }
  })

  for (const tool of ['Edit', 'Write'] as const) {
    on('tool.call', { tool }, guard).catch(($, e, next) => next(e))
  }
}
