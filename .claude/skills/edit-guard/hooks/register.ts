import type { EngineInterface, Register } from 'claude-code'
import { type Kind, type Kinds, judge, pathspecs, refusal } from './judge.ts'

// Refuses the shell commands CLAUDE.md bans before they run: in-place stream
// edits and scripted writes into the project's source and record, and the git
// commands that sweep or discard the working tree. `judge.ts` holds the rules.
//
// It fails open: if the judge throws, or the disk cannot be asked, the command
// runs, because a guard that refused every command would stop the session for
// a defect of its own.

/**
 * What the disk says of each path a `git add` or `git checkout` names, so a
 * directory and a branch are told apart (`git checkout crates/ondin-app` from
 * `git checkout feature/x`). A path that is not there is `none`; a disk that
 * cannot be asked throws, and the guard fails open.
 */
async function kinds($: EngineInterface, command: string): Promise<Kinds> {
  const out = new Map<string, Kind>()
  for (const p of pathspecs(command)) {
    if (out.has(p)) continue
    if (!(await $.fs.exists(p))) {
      out.set(p, 'none')
      continue
    }
    const s = await $.fs.stat(p)
    out.set(p, s.kind === 'dir' ? 'dir' : 'file')
  }
  return out
}

export const register: Register = on => {
  for (const tool of ['Bash', 'PowerShell'] as const) {
    on('tool.call', { tool }, async ($, e, next) => {
      const command = 'command' in e && typeof e.command === 'string' ? e.command : ''
      const what = judge(command, await kinds($, command))
      return what ? { deny: refusal(what) } : next(e)
    }).catch(($, e, next) => next(e))
  }
}
