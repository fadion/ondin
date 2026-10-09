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
//   §15.0's last entry and outside the reserved block.
//
// The guard fails open: if git cannot be asked, the edit goes through, since a
// broken guard refusing every edit is worse than the habit it backs up.

const KEY = 'block'
const TOOL = 'reserve'

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

async function readBlock($: EngineInterface): Promise<Block | undefined> {
  const v = await $.store.get(KEY)
  return v && typeof v === 'object' ? (v as Block) : undefined
}

function showStatus($: EngineInterface, block: Block | undefined) {
  $.ui.status(block ? `§15 ${span(block.from, block.to)} reserved` : undefined)
}

const listSites = (found: Map<number, Set<string>>) =>
  [...found].sort(([a], [b]) => a - b).map(([n, paths]) => `- D${n}: ${[...paths].join(', ')}`)

async function reserve($: EngineInterface, count: number): Promise<string> {
  const held = await readBlock($)
  if (held) {
    return `A block is already reserved: ${span(held.from, held.to)} (at ${held.head}, ${held.at}). Release it first (action "release"), then reserve again.`
  }
  const lastEntry = await last($)
  const from = lastEntry + 1
  const to = from + count - 1
  const found = await citations($, members({ from, to }))
  if (found.size > 0) {
    return [
      `Not reserved: the negative grep over ${span(from, to)} found sites, so a number past §15.0's last entry (D${lastEntry}) is already typed somewhere with no entry behind it. Write the entry or remove the citation first:`,
      ...listSites(found),
    ].join('\n')
  }
  const { out } = await git($, ['rev-parse', '--short', 'HEAD'])
  const block: Block = { from, to, head: out.trim(), at: new Date().toISOString() }
  await $.store.set(KEY, block)
  showStatus($, block)
  return `Reserved ${span(from, to)} at ${block.head}: §15.0's last entry is D${lastEntry}, and the negative grep over the block found zero sites (${EXCLUDED.join(', ')} excluded). Spend only these numbers; release the block at the end of the request with action "release".`
}

async function release($: EngineInterface): Promise<string> {
  const block = await readBlock($)
  if (!block) return 'Nothing is reserved.'
  const lastEntry = await last($)
  const numbers = members(block)
  const found = await citations($, numbers)
  const spent = numbers.filter(n => found.has(n))
  const unspent = numbers.filter(n => !found.has(n))
  const unresolved = spent.filter(n => n > lastEntry)
  const gaps = unspent.filter(n => spent.some(s => s > n))

  await $.store.delete(KEY)
  showStatus($, undefined)

  const lines = [`Released ${span(block.from, block.to)} (reserved at ${block.head}). §15.0's last entry is now D${lastEntry}.`]
  lines.push(spent.length ? 'Spent:' : 'Spent: none.')
  if (spent.length) lines.push(...listSites(found))
  lines.push(unspent.length ? `Released at zero sites: ${unspent.map(n => `D${n}`).join(', ')}.` : 'Every number was spent.')
  if (unresolved.length) {
    lines.push(`⚠️ Cited with no §15.0 entry: ${unresolved.map(n => `D${n}`).join(', ')} — write the entries; the census's unresolved half will show them.`)
  }
  if (gaps.length) {
    lines.push(`⚠️ Unspent below a spent number: ${gaps.map(n => `D${n}`).join(', ')} — §15 runs from D1 with no gaps, so these must be spent or the later ones renumbered.`)
  }
  lines.push("CLAUDE.md's live D-number figures are the session's to update.")
  return lines.join('\n')
}

async function status($: EngineInterface): Promise<string> {
  const block = await readBlock($)
  const lastEntry = await last($)
  return block
    ? `${span(block.from, block.to)} is reserved (at ${block.head}, ${block.at}). §15.0's last entry is D${lastEntry}.`
    : `Nothing is reserved. §15.0's last entry is D${lastEntry}, so the next free number is D${lastEntry + 1}.`
}

const normal = (p: string) => p.replace(/\\/g, '/').toLowerCase()

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
  const { out: top, code } = await git($, ['rev-parse', '--show-toplevel'])
  const root = top.trim()
  if (code !== 0 || !normal(e.file_path).startsWith(normal(root) + '/')) return next(e)
  if (isExcluded(e.file_path, root)) return next(e)

  const typed = introduced(before, after)
  if (typed.length === 0) return next(e)
  const lastEntry = await last($)
  const block = await readBlock($)
  const bad = unreserved(typed, lastEntry, block)
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
    showStatus($, await readBlock($))
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
