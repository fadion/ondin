import type { Register } from 'claude-code'
import { judge, refusal } from './judge.ts'

// Refuses the shell commands CLAUDE.md bans before they run: in-place stream
// edits and scripted writes into the project's source and record, and the git
// commands that sweep or discard the working tree. `judge.ts` holds the rules.
//
// It fails open: if the judge throws, the command runs, because a guard that
// refused every command would stop the session for a defect of its own.

export const register: Register = on => {
  for (const tool of ['Bash', 'PowerShell'] as const) {
    on('tool.call', { tool }, ($, e, next) => {
      const command = 'command' in e && typeof e.command === 'string' ? e.command : ''
      const what = judge(command)
      return what ? { deny: refusal(what) } : next(e)
    }).catch(($, e, next) => next(e))
  }
}
