import type { EngineInterface, Register, ToolCallInput, ToolCallResult } from 'claude-code'
import { compare, describe } from './docrun.ts'

// After every Edit or Write to a `.rs` file, compares the file's `///` runs
// before and after and tells the model, in the edit's own result, when the
// edit took an item's doc comment. CLAUDE.md's "Anchor an insertion above,
// never on the item below it" records thirty-four of these, every one with
// all the gates green: the check here is that section's habits 2 and 3, run on
// every insertion instead of the ones that felt risky.
//
// It never refuses an edit (the edit has already run when the theft is
// visible); it adds a reminder the model reads after the tool's result, and a
// toast so the person sees it too.

async function readOrUndefined($: EngineInterface, path: string): Promise<string | undefined> {
  try {
    return await $.fs.read(path)
  } catch {
    return undefined
  }
}

async function check(
  $: EngineInterface,
  e: ToolCallInput,
  next: (e: ToolCallInput) => Promise<ToolCallResult>,
): Promise<ToolCallResult> {
  const path = 'file_path' in e && typeof e.file_path === 'string' ? e.file_path : ''
  if (!/\.rs$/i.test(path)) return next(e)

  const before = await readOrUndefined($, path)
  const ran = await next(e)
  if (before === undefined || ran.deny !== undefined || ran.isError) return ran
  const after = await readOrUndefined($, path)
  if (after === undefined) return ran

  const findings = compare(before, after)
  if (findings.length === 0) return ran

  const name = path.replace(/\\/g, '/').split('/').pop()
  $.ui.toast(`doc-run-guard: ${findings.length} doc run(s) displaced in ${name}`, { timeoutMs: 8000 })
  const message = [
    `doc-run-guard: this edit to ${path} looks like a doc-comment theft (CLAUDE.md, "Anchor an insertion above, never on the item below it"):`,
    ...findings.map(f => `- ${describe(f)}`),
    'Read the spot now and put each doc back on its own item; re-anchor the insertion on the line above the neighbour\'s doc run, not on its `fn`, `#[test]` or attribute line.',
  ].join('\n')
  return { ...ran, context: [...(ran.context ?? []), message] } as ToolCallResult
}

export const register: Register = on => {
  for (const tool of ['Edit', 'Write'] as const) {
    on('tool.call', { tool }, check).catch(($, e, next) => next(e))
  }
}
