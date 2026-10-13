// The D-number rules as plain functions, so the tests can call them without
// an engine. The hooks module does the reading and the git calls.

/** A reserved block of §15 numbers, inclusive at both ends. */
export type Block = {
  from: number
  to: number
  /** The short commit the negative grep ran against. */
  head: string
  /** When it was reserved, ISO. */
  at: string
  /**
   * The worktree it was reserved in (`--show-toplevel`) and the repository
   * that worktree belongs to (`--git-common-dir`). Absent on a block stored
   * before blocks were kept per worktree.
   */
  root?: string
  common?: string
  /** Set at reserve and read back after the write, to see a concurrent reserve. */
  token?: string
}

/**
 * Every D-number cited in `text`, as numbers.
 *
 * Bounded on both sides by a non-word character, so a hex literal's `0xD0`
 * (CLAUDE.md's census trap, §15 D832's test) is not read as `D0`.
 */
export function cited(text: string): Set<number> {
  const out = new Set<number>()
  for (const m of text.matchAll(/(?<![A-Za-z0-9_])D(\d{1,5})(?![A-Za-z0-9_])/g)) out.add(Number(m[1]))
  return out
}

/** The numbers `after` cites and `before` does not: what an edit types in. */
export function introduced(before: string, after: string): number[] {
  const had = cited(before)
  return [...cited(after)].filter(n => !had.has(n)).sort((a, b) => a - b)
}

/** §15.0's last index row's number, from `git grep -o` of the index rows. */
export function lastIndex(rows: string): number {
  let last = 0
  for (const m of rows.matchAll(/^- \*\*D(\d+)\*\*/gm)) last = Math.max(last, Number(m[1]))
  return last
}

/**
 * The introduced numbers an edit may not type: past §15.0's last entry and
 * outside the reserved block. A number at or below the last entry cites an
 * entry that exists, and §15.0 has no gaps.
 */
export function unreserved(numbers: readonly number[], last: number, block: Block | undefined): number[] {
  return numbers.filter(n => n > last && !(block && n >= block.from && n <= block.to))
}

/** A block as CLAUDE.md writes one: `D<from>–D<to>`, or one number alone. */
export function span(from: number, to: number): string {
  return from === to ? `D${from}` : `D${from}–D${to}`
}

/** The numbers of a block, in order. */
export function members(block: Pick<Block, 'from' | 'to'>): number[] {
  const out: number[] = []
  for (let n = block.from; n <= block.to; n++) out.push(n)
  return out
}

/**
 * Paths, relative to the repository root, whose text names D-numbers as prose
 * about the numbering rather than as citations: CLAUDE.md records every
 * reservation and release by range, so it would read as spending every number
 * it has ever written down; and this mod's own tests type numbers past the
 * last entry on purpose.
 */
export const EXCLUDED = ['CLAUDE.md', '.claude/skills/dnumber-guard']

/** Whether an absolute or relative path (any slashes, any case) is one of EXCLUDED. */
export function isExcluded(path: string, root: string): boolean {
  const norm = (p: string) => p.replace(/\\/g, '/').toLowerCase().replace(/\/+$/, '')
  const p = norm(path)
  const r = norm(root)
  const rel = r && p.startsWith(r + '/') ? p.slice(r.length + 1) : p
  return EXCLUDED.some(x => {
    const ex = norm(x)
    return rel === ex || rel.startsWith(ex + '/')
  })
}
