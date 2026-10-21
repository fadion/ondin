import type { Register } from 'claude-code'
import { refusal } from './archive.ts'

// Refuses a whole read of the frozen ledger under `docs/archive/`; `archive.ts`
// holds the rule. Grep, Glob and shell reads are not this hook's, and stay
// allowed. It fails open: a guard that threw would otherwise stop every Read.

export const register: Register = on => {
  on('tool.call', { tool: 'Read' }, async ($, e, next) => {
    const path = 'file_path' in e && typeof e.file_path === 'string' ? e.file_path : ''
    const limit = 'limit' in e && typeof e.limit === 'number' ? e.limit : undefined
    const why = refusal(path, limit)
    return why ? { deny: why } : next(e)
  }).catch(($, e, next) => next(e))
}
