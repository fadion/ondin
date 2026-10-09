// The doc-comment-theft check, as plain functions so the tests can call them
// without an engine.
//
// It is CLAUDE.md's `docrun.awk` turned into a before/after comparison: that
// awk ranks every `///` run in the tree by length and finds *accumulated*
// thefts; this compares one file before and after one edit and finds the theft
// the edit just made, which is the half the length ranking cannot see (a merged
// run under the ranking's floor, a swap, a summary line carried off).

/** One `///` run and the item line under it. */
export type Run = {
  /** The item line, trimmed: the key a run is matched by across the edit. */
  item: string
  /** The run's `///` lines, trimmed, in order. */
  doc: string[]
}

/** What one edit did to a doc run that it should not have. */
export type Finding =
  | { kind: 'lost'; item: string; lines: number; nowAbove?: string }
  | { kind: 'summary-moved'; item: string; summary: string; nowAbove: string }
  | { kind: 'swapped'; item: string; docOf: string }
  | { kind: 'duplicated'; item: string; lines: number }

const DOC = /^\s*\/\/\//
const ATTR = /^\s*#\[/
const COMMENT = /^\s*\/\//

/** The file's lines, with any CR dropped so a CRLF file reads as LF. */
export function lines(text: string): string[] {
  return text.split('\n').map(l => l.replace(/\r$/, ''))
}

/**
 * Which lines belong to an attribute, the continuation lines of one that
 * spans several (`#[cfg(any(\n    target_os = "linux",\n ...))]`) included:
 * read line by line, those continuations looked like the item under the doc.
 */
function attributeLines(ls: readonly string[]): boolean[] {
  const mask = ls.map(() => false)
  let depth = 0
  ls.forEach((l, i) => {
    if (depth === 0 && !ATTR.test(l)) return
    mask[i] = true
    for (const ch of l) {
      if (ch === '[' || ch === '(') depth++
      else if (ch === ']' || ch === ')') depth = Math.max(0, depth - 1)
    }
  })
  return mask
}

/**
 * Every `///` run with the item line it documents, by `docrun.awk`'s rules:
 * an attribute or a plain `//` comment between a run and its item is skipped,
 * and a blank line ends the run with no item.
 */
export function docRuns(text: string): Run[] {
  const out: Run[] = []
  const ls = lines(text)
  const attr = attributeLines(ls)
  let doc: string[] = []
  for (const [i, line] of ls.entries()) {
    if (DOC.test(line)) {
      doc.push(line.trim())
      continue
    }
    if (doc.length === 0) continue
    if (attr[i] || COMMENT.test(line)) continue
    const item = line.trim()
    if (item !== '') out.push({ item, doc })
    doc = []
  }
  return out
}

/** How many times each trimmed line occurs, so a key is used only where unique. */
function counts(ls: string[]): Map<string, number> {
  const m = new Map<string, number>()
  for (const l of ls) {
    const t = l.trim()
    if (t !== '') m.set(t, (m.get(t) ?? 0) + 1)
  }
  return m
}

/** The doc run above line `at`, walking up past attributes and plain comments. */
function docAbove(ls: string[], attr: readonly boolean[], at: number): string[] {
  const doc: string[] = []
  for (let i = at - 1; i >= 0; i--) {
    const l = ls[i] ?? ''
    if (DOC.test(l)) {
      doc.unshift(l.trim())
      continue
    }
    if (attr[i] || COMMENT.test(l)) continue
    break
  }
  return doc
}

const join = (doc: readonly string[]) => doc.join('\n')

/**
 * Compares a Rust file before and after one edit and reports each item whose
 * doc run the edit took, moved, swapped or copied.
 *
 * Items are matched by their trimmed line and only where that line is unique
 * on both sides, so an edit that renames an item, or a file with two `fn new(`,
 * says nothing about those items rather than guessing. A run that merely
 * changed is a doc edit and is not reported: a theft is told apart by where
 * the old run's text went.
 */
export function compare(before: string, after: string): Finding[] {
  const bl = lines(before)
  const al = lines(after)
  const aAttr = attributeLines(al)
  const bCount = counts(bl)
  const aCount = counts(al)
  const bRuns = docRuns(before)
  const aRuns = docRuns(after)
  const bItems = new Set(bl.map(l => l.trim()))
  const aIndex = new Map<string, number>()
  al.forEach((l, i) => aIndex.set(l.trim(), i))

  const findings: Finding[] = []
  const unique = (item: string) => bCount.get(item) === 1 && aCount.get(item) === 1
  const bDocByItem = new Map(bRuns.filter(r => unique(r.item)).map(r => [r.item, r.doc]))

  for (const [item, was] of bDocByItem) {
    const at = aIndex.get(item)
    if (at === undefined) continue
    const now = docAbove(al, aAttr, at)
    if (join(now) === join(was)) continue
    const head = was[0] ?? ''

    if (now.length === 0) {
      const holder = aRuns.find(r => r.item !== item && r.doc.includes(head))
      findings.push({ kind: 'lost', item, lines: was.length, nowAbove: holder?.item })
      continue
    }
    const swappedWith = [...bDocByItem].find(
      ([other, doc]) => other !== item && join(doc) === join(now),
    )
    if (swappedWith) {
      findings.push({ kind: 'swapped', item, docOf: swappedWith[0] })
      continue
    }
    if (now[0] !== head) {
      const holder = aRuns.find(r => !bItems.has(r.item) && r.doc[0] === head)
      if (holder) findings.push({ kind: 'summary-moved', item, summary: head, nowAbove: holder.item })
    }
  }

  const tally = (runs: Run[]) => {
    const m = new Map<string, number>()
    for (const r of runs) if (r.doc.length >= 3) m.set(join(r.doc), (m.get(join(r.doc)) ?? 0) + 1)
    return m
  }
  const bTally = tally(bRuns)
  for (const [text, n] of tally(aRuns)) {
    if (n >= 2 && n > (bTally.get(text) ?? 0)) {
      const run = aRuns.find(r => join(r.doc) === text)!
      findings.push({ kind: 'duplicated', item: run.item, lines: run.doc.length })
    }
  }
  return findings
}

/** One finding as a line the model and the person can act on. */
export function describe(f: Finding): string {
  switch (f.kind) {
    case 'lost':
      return f.nowAbove
        ? `\`${f.item}\` lost its ${f.lines}-line \`///\` run, which now sits above \`${f.nowAbove}\``
        : `\`${f.item}\` lost its ${f.lines}-line \`///\` run`
    case 'summary-moved':
      return `\`${f.item}\`'s summary line ("${f.summary}") now heads the doc of the new \`${f.nowAbove}\`: the insertion landed inside its doc run`
    case 'swapped':
      return `\`${f.item}\` now carries the doc \`${f.docOf}\` had: two doc runs were swapped`
    case 'duplicated':
      return `a ${f.lines}-line \`///\` run above \`${f.item}\` now appears twice in the file`
  }
}
