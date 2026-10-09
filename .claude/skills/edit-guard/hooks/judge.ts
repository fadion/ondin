// The shell commands CLAUDE.md bans, as a plain function over the command
// text so the tests can call it without an engine.
//
// Two rules from CLAUDE.md, each paid for:
// - "Edit files with the `Edit` tool. Never splice source programmatically" —
//   a `perl -0pi` left a 3,900-line file at zero bytes, and a scripted rewrite
//   on Windows re-decides every line ending;
// - "Never `git add -A`" and "`git checkout <file>` is that net pointed the
//   wrong way" — a sweeping stage committed a subagent's half-finished work
//   four times, and a checkout threw away 200 lines of uncommitted work.
//
// Reading with `cat`, `sed -n`, `grep` and `find` stays allowed, and so does
// writing scratch files: only a write into a file the project tracks as source
// or record is refused.

/** Extensions of the files the editing rule protects: source and record. */
const PROTECTED = /\.(rs|md|toml|wgsl|ya?ml|sh|json|ts|tsx)$/i

/** Paths that are scratch, never the project's own files. */
const SCRATCH = /(^|[\\/])(tmp|temp|scratchpad)([\\/]|$)|^\/dev\/|^nul$|\$env:TEMP|%TEMP%|\$TMPDIR|\$\{?TMPDIR/i

/**
 * The command with its heredoc bodies cut and each quoted string replaced:
 * by its content when that has no whitespace (a path, a sed script), else by
 * `Q`. So a commit message that mentions `sed -i` is not read as running it,
 * while `sed -i 's/a/b/' x.rs` keeps every word that matters.
 */
export function skeleton(command: string): string {
  const lines = command.split('\n')
  const kept: string[] = []
  let until: string | undefined
  for (const line of lines) {
    if (until !== undefined) {
      if (line.trim() === until) until = undefined
      continue
    }
    kept.push(line)
    const m = /<<-?\s*(['"]?)([A-Za-z_][A-Za-z0-9_]*)\1/.exec(line)
    if (m) until = m[2]
  }
  return kept
    .join('\n')
    .replace(/@'[\s\S]*?\n'@|@"[\s\S]*?\n"@/g, 'Q')
    .replace(/'([^']*)'|"((?:[^"\\]|\\.)*)"/g, (_, a: string | undefined, b: string | undefined) => {
      const s = a ?? b ?? ''
      return /\s/.test(s) || s === '' ? 'Q' : s
    })
}

/** The simple commands of a skeleton: split on `;`, `&&`, `||`, `|` and newlines. */
function segments(skel: string): string[][] {
  return skel
    .split(/;|&&|\|\||\||\n/)
    .map(s => s.trim().split(/\s+/).filter(Boolean))
    .filter(s => s.length > 0)
}

/** Strips a leading `env X=y`, `sudo`, path and `.exe` so `/usr/bin/sed.exe` reads `sed`. */
function program(words: string[]): { name: string; args: string[] } {
  let i = 0
  while (i < words.length && (/^[A-Za-z_][A-Za-z0-9_]*=/.test(words[i]!) || words[i] === 'env' || words[i] === 'sudo' || words[i] === '&')) i++
  const name = (words[i] ?? '').replace(/^.*[\\/]/, '').replace(/\.exe$/i, '').toLowerCase()
  return { name, args: words.slice(i + 1) }
}

const isProtected = (p: string) => PROTECTED.test(p) && !SCRATCH.test(p)

/** Why the command is refused, or undefined when it may run. */
export function judge(command: string): string | undefined {
  const skel = skeleton(command)

  for (const words of segments(skel)) {
    const { name, args } = program(words)

    if (name === 'sed' && args.some(a => /^-[A-Za-z]*i/.test(a) || a.startsWith('--in-place'))) {
      return 'an in-place `sed`'
    }
    if (name === 'perl' && args.some(a => /^-[A-Za-z0-9]*i/.test(a))) return 'an in-place `perl`'
    if ((name === 'awk' || name === 'gawk') && args.join(' ').includes('-i inplace')) return 'an in-place `awk`'
    if (name === 'tee' && args.some(a => !a.startsWith('-') && isProtected(a))) return '`tee` into a project file'
    if (['set-content', 'add-content', 'out-file', 'sc', 'ac'].includes(name) && args.some(a => isProtected(a))) {
      return `\`${name}\` into a project file`
    }
    if (name === 'git') {
      const verdict = judgeGit(args)
      if (verdict) return verdict
    }
  }

  for (const m of skel.matchAll(/(?<![0-9&<>])>>?\s*([^\s;&|<>]+)/g)) {
    const target = m[1]!
    if (target.startsWith('&')) continue
    if (isProtected(target)) return `a redirect into ${target}`
  }

  if (/\b(python3?|py|node|deno|bun)\b/i.test(skel)) {
    const writes = /open\s*\([^)]*,\s*['"](w|a|wb|ab|r\+|w\+)['"]|\.write_text\s*\(|\.write_bytes\s*\(|writeFileSync|appendFileSync|\bwriteFile\s*\(/
    const paths = [...command.matchAll(/[A-Za-z0-9_.\\/:-]+\.(rs|md|toml|wgsl|ya?ml|sh|json|ts|tsx)\b/gi)].map(m => m[0])
    if (writes.test(command) && paths.some(isProtected)) return 'a script writing a project file'
  }

  if (/\[(System\.)?IO\.File\]::(Write|Append)All/i.test(command)) {
    const paths = [...command.matchAll(/[A-Za-z0-9_.\\/:-]+\.(rs|md|toml|wgsl|ya?ml|sh|json|ts|tsx)\b/gi)].map(m => m[0])
    if (paths.some(isProtected)) return 'a .NET write into a project file'
  }
  return undefined
}

function judgeGit(args: string[]): string | undefined {
  // `git -C dir add -A`: skip the global options before the subcommand.
  let i = 0
  while (i < args.length && args[i]!.startsWith('-')) i += args[i] === '-C' || args[i] === '-c' ? 2 : 1
  const sub = args[i]
  const rest = args.slice(i + 1)
  if (sub === 'add' && rest.some(a => a === '-A' || a === '--all' || a === '.' || a === '-u' || a === '--update' || a === ':/' || a === '*')) {
    return '`git add` of everything (`-A`, `--all`, `-u`, `.`)'
  }
  if (sub === 'commit' && rest.some(a => /^-[A-Za-z]*a[A-Za-z]*$/.test(a) || a === '--all')) return '`git commit -a`'
  if (sub === 'checkout') {
    if (rest.some(a => a === '-b' || a === '-B' || a === '--orphan')) return undefined
    if (rest.includes('--') || rest.includes('.') || rest.some(a => /[\\/]|\.[A-Za-z][A-Za-z0-9]*$/.test(a) && !a.startsWith('-'))) {
      return '`git checkout` of a file'
    }
  }
  if (sub === 'restore' && !rest.includes('--staged') && !rest.includes('-S')) return '`git restore` of a working-tree file'
  if (sub === 'reset' && rest.includes('--hard')) return '`git reset --hard`'
  if (sub === 'clean' && rest.some(a => /^-[A-Za-z]*f/.test(a))) return '`git clean -f`'
  if (sub === 'stash' && (rest.length === 0 || rest[0] === 'push' || rest[0] === 'save' || rest[0] === '-u')) {
    return '`git stash` of the working tree'
  }
  return undefined
}

/** The refusal the model reads, naming the rule and what to do instead. */
export function refusal(what: string): string {
  return [
    `edit-guard: refused ${what}.`,
    'CLAUDE.md: edit files with the Edit tool and never splice source programmatically (a stream editor writes the whole file every time it runs, and on Windows re-decides every line ending);',
    'stage the specific files you changed, never everything (a subagent may be writing in the tree);',
    "and undo an edit with an edit — `git checkout`, `restore`, `reset --hard`, `clean` and `stash` cannot tell which half of a file's uncommitted work you meant.",
    'If the person asked for this exact command, ask them to run it themselves.',
  ].join(' ')
}
