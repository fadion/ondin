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
//
// The command is read as the shell would split it, well enough to find the
// program each simple command runs: a loop's body, a conditional's branch, a
// subshell, a `$(…)`, the words after `find -exec`, `xargs`, `time` and their
// kin, and the script a `bash -c` or `pwsh -Command` runs, which is judged as
// a command of its own ([X7-L1-02] in the v0.5.0 release review).

/** Extensions of the files the editing rule protects: source and record. */
const PROTECTED = /\.(rs|md|toml|wgsl|ya?ml|sh|json|ts|tsx)$/i

/** Paths that are scratch or build output, never the project's own files. */
const SCRATCH = /(^|[\\/])(tmp|temp|scratchpad|target)([\\/]|$)|^\/dev\/|^nul$|\$env:TEMP|%TEMP%|\$TMPDIR|\$\{?TMPDIR/i

/**
 * The review ledger, gitignored, where an append (`>>`, `tee -a`) and a copy
 * are the prescribed writes: its method appends a block written to the
 * scratchpad with `cat scratch >> review/findings.md`, and a release review
 * copies each pass file in from the scratchpad ([X7-L1-06]). A `>` there still
 * truncates a ledger with no `git checkout` behind it, and stays refused, as
 * does every in-place edit.
 */
const LEDGER = /(^|[\\/])review[\\/]/i

/** What the hook found on disk for a path a git command names. */
export type Kind = 'file' | 'dir' | 'none'
/** Paths as the command spells them, to what the disk says of each. */
export type Kinds = ReadonlyMap<string, Kind>

/** A quoted string's stand-in in the skeleton: `«n»`, `n` its index in `strings`. */
const MARK = /«(\d+)»/g

type Parsed = {
  /**
   * The command with its heredoc bodies cut and each quoted string replaced:
   * by its content when that is one plain word (a path, a sed script), else
   * by its mark. So a commit message that mentions `sed -i` is not read as
   * running it, while `sed -i 's/a/b/' x.rs` keeps every word that matters.
   */
  skel: string
  /**
   * The same with every quoted string a mark, for the redirect scan: a `>`
   * inside a quoted pattern (`grep -n '->' lib.rs`) is not a redirect
   * ([X7-L1-04]).
   */
  redir: string
  /** The quoted strings' contents, by mark. */
  strings: string[]
}

function parse(command: string): Parsed {
  const kept: string[] = []
  let until: string | undefined
  for (const line of command.split('\n')) {
    if (until !== undefined) {
      if (line.trim() === until) until = undefined
      continue
    }
    kept.push(line)
    const m = /<<-?\s*(['"]?)([A-Za-z_][A-Za-z0-9_]*)\1/.exec(line)
    if (m) until = m[2]
  }
  const text = kept.join('\n').replace(/@'[\s\S]*?\n'@|@"[\s\S]*?\n"@/g, 'Q')
  const strings: string[] = []
  let skel = ''
  let redir = ''
  let at = 0
  for (const m of text.matchAll(/'([^']*)'|"((?:[^"\\]|\\.)*)"/g)) {
    const pre = text.slice(at, m.index)
    const raw = m[1] ?? (m[2] ?? '').replace(/\\(["\\$`])/g, '$1')
    const mark = `«${strings.push(raw) - 1}»`
    skel += pre + (raw === '' || /[\s;|&()<>`]/.test(raw) ? mark : raw)
    redir += pre + mark
    at = m.index + m[0].length
  }
  return { skel: skel + text.slice(at), redir: redir + text.slice(at), strings }
}

/** The skeleton of a command, as `parse` makes it. */
export function skeleton(command: string): string {
  return parse(command).skel
}

/**
 * The simple commands of a skeleton: split on `;`, `&&`, `||`, `|`, newlines,
 * parentheses (a subshell, a `$(…)`) and backticks, each without its
 * redirections (`2>&1`, `> out.txt`, `<<EOF`), which the redirect scan reads
 * and which are not the program's arguments.
 */
function segments(skel: string): string[][] {
  return skel
    .split(/;|&&|\|\||\||\n|\(|\)|`/)
    .map(s => withoutRedirects(s.trim().split(/\s+/).filter(Boolean)))
    .filter(s => s.length > 0)
}

function withoutRedirects(words: readonly string[]): string[] {
  const out: string[] = []
  for (let i = 0; i < words.length; i++) {
    const w = words[i]!
    if (/^(\d*|&)>>?$|^\d*<$|^<<-?$/.test(w)) {
      i++
      continue
    }
    if (/^(\d*|&)>>?\S|^<<-?\S|^<[^<]/.test(w)) continue
    out.push(w)
  }
  return out
}

/** Words that run the command after them, and the shell keywords that lead one. */
const LEADERS = new Set([
  'do', 'then', 'else', 'elif', 'if', 'while', 'until', '!', '{', '&',
  'time', 'nohup', 'command', 'builtin', 'exec', 'nice', 'xargs', 'timeout', 'stdbuf', 'env', 'sudo', 'doas', 'wsl',
])
/** A leader's options that take the next word as their value. */
const VALUED = new Set(['-I', '-n', '-P', '-L', '-d', '-a', '-s', '-E', '-u', '-g', '-k', '--cd', '--distribution', '--user'])

/** Strips leaders (`env X=y`, `sudo`, `xargs -0`, `do`), a path and `.exe`, so `/usr/bin/sed.exe` reads `sed`. */
function program(words: string[]): { name: string; args: string[] } {
  let i = 0
  while (i < words.length) {
    const w = words[i]!
    if (/^[A-Za-z_][A-Za-z0-9_]*=/.test(w)) {
      i++
      continue
    }
    const lead = w.toLowerCase().replace(/\.exe$/, '')
    if (!LEADERS.has(lead)) break
    i++
    while (i < words.length && words[i]!.startsWith('-')) i += VALUED.has(words[i]!) ? 2 : 1
    if (lead === 'timeout' && /^\d/.test(words[i] ?? '')) i++
  }
  const name = (words[i] ?? '').replace(/^.*[\\/]/, '').replace(/\.exe$/i, '').toLowerCase()
  return { name, args: words.slice(i + 1) }
}

const isProtected = (p: string) => PROTECTED.test(p) && !SCRATCH.test(p)

/** A path outside the tree, or scratch: where a copy or move onto a source file brings whole new content from. */
const isOutside = (p: string) => /^([A-Za-z]:|[\\/~$%])/.test(p) || SCRATCH.test(p)

/**
 * A backup a session took: in its scratchpad, or named `.bak` or `.orig`
 * (`app.rs.bak`, `menu7.bak`, `theme.orig.rs`). CLAUDE.md asks for one
 * "before an experiment you expect to revert", and copying it back is that
 * backup's use, not a scripted rewrite.
 */
const isBackup = (p: string) => /(^|[\\/])scratchpad[\\/]/i.test(p) || /\.(\w*bak\d*|orig\d*)(\.\w+)?$/i.test(p)

const slashes = (p: string) => p.replace(/\\/g, '/')

/**
 * The string values the command gives variables — `p='x.rs'`, `$p = 'x.rs'`,
 * `p=x.rs`, `const p = "x.rs"` — so a write whose target is a variable is
 * judged by what the variable holds ([X7-L1-05]).
 */
function assignments(command: string): Map<string, string> {
  const vars = new Map<string, string>()
  for (const m of command.matchAll(/(?:^|[\s;(,])(?:const |let |var )?\$?([A-Za-z_]\w*)\s*=\s*[rbfRBF]{0,2}(['"])([^'"\n]*)\2/g)) {
    vars.set(m[1]!, m[3]!)
  }
  for (const m of command.matchAll(/(?:^|[\s;])([A-Za-z_]\w*)=([^\s;'"|&()]+)/g)) {
    if (!vars.has(m[1]!)) vars.set(m[1]!, m[2]!)
  }
  return vars
}

type Context = {
  strings: string[]
  vars: Map<string, string>
  kinds?: Kinds
  depth: number
  /** The files this command writes by a redirect or `tee`, slashes forward. */
  written: Set<string>
}

/**
 * A word as the program receives it: a quoted string's content, and each
 * variable the command gives a value replaced by it (`$S/x.rs` with `S` set
 * earlier in the same command). A variable it does not set stays as written.
 */
function resolve(word: string, ctx: Context): string {
  return word
    .replace(MARK, (_, i: string) => ctx.strings[Number(i)] ?? '')
    .replace(/\$\{?([A-Za-z_]\w*)\}?/g, (whole, name: string) => ctx.vars.get(name) ?? whole)
}

/** Whether a path still holds a variable the command never set, so where it lands is unknown. */
const unknownVar = (p: string) => /\$\{?[A-Za-z_]/.test(p.replace(/\$env:TEMP|\$\{?TMPDIR\}?/gi, ''))

/** The words that are not options, everything after `--` included. */
function positionals(args: readonly string[]): string[] {
  const out: string[] = []
  let rest = false
  for (const a of args) {
    if (rest) out.push(a)
    else if (a === '--') rest = true
    else if (!a.startsWith('-')) out.push(a)
  }
  return out
}

const SHELLS = new Set(['bash', 'sh', 'zsh', 'dash', 'ksh', 'pwsh', 'powershell', 'cmd'])

/** Why one simple command is refused, or undefined. */
function judgeWords(words: string[], ctx: Context): string | undefined {
  const { name, args } = program(words)
  const res = (a: string) => resolve(a, ctx)

  if (name === 'sed' && args.some(a => /^-[A-Za-z]*i/.test(a) || a.startsWith('--in-place'))) {
    return 'an in-place `sed`'
  }
  // `-pi`, `-0pi`, `-i.bak`; never `-Ilib` (an include path, not in place).
  if ((name === 'perl' || name === 'ruby') && args.some(a => /^-[a-hj-zA-HJ-Z0-9]*i/.test(a))) {
    return `an in-place \`${name}\``
  }
  if ((name === 'awk' || name === 'gawk') && args.join(' ').includes('-i inplace')) return 'an in-place `awk`'
  if (name === 'tee') {
    const append = args.some(a => a === '-a' || a === '--append')
    const target = args.map(res).find(a => !a.startsWith('-') && isProtected(a) && !(append && LEDGER.test(a)))
    if (target) return '`tee` into a project file'
  }
  if (['set-content', 'add-content', 'out-file', 'sc', 'ac'].includes(name) && args.some(a => isProtected(res(a)))) {
    return `\`${name}\` into a project file`
  }
  if (['cp', 'copy', 'copy-item', 'cpi', 'mv', 'move', 'move-item', 'mi'].includes(name)) {
    const named = (flag: RegExp) => {
      const i = args.findIndex(a => flag.test(a))
      return i >= 0 ? args[i + 1] : undefined
    }
    const pos = positionals(args.filter((a, i) => !/^-(destination|path|literalpath)$/i.test(args[i - 1] ?? '')))
    const dest = res(named(/^-destination$/i) ?? (pos.length >= 2 ? pos[pos.length - 1]! : ''))
    const src = res(named(/^-(path|literalpath)$/i) ?? pos[0] ?? '')
    // A copy or move over a source file is a scripted rewrite when this same
    // command wrote what it copies (`awk … > /tmp/x.rs && mv /tmp/x.rs x.rs`),
    // or when it brings a whole file in from outside the tree that is not a
    // backup the session took. A copy or move inside the tree is a rename or a
    // new file; a copy into `review/` is how a release review's passes land
    // there; a destination behind a variable this command never set is not
    // known to be the project's.
    const fenced = dest && isProtected(dest) && !LEDGER.test(dest) && !unknownVar(dest)
    const spliced = ctx.written.has(slashes(src))
    if (fenced && (spliced || (isOutside(src) && !isBackup(src)))) return `\`${name}\` over a project file`
  }
  if ((name === 'new-item' || name === 'ni') && args.some(a => /^-force$/i.test(a)) && args.some(a => isProtected(res(a)))) {
    return '`New-Item -Force` over a project file (it truncates)'
  }
  if (name === 'git') {
    const verdict = judgeGit(args.map(res), ctx.kinds)
    if (verdict) return verdict
  }
  if (name === 'find') {
    for (const [i, a] of args.entries()) {
      if (!/^-(exec|execdir|ok|okdir)$/.test(a)) continue
      const end = args.findIndex((b, j) => j > i && (b === '+' || b === '\\' || b === ';'))
      const verdict = judgeWords(args.slice(i + 1, end < 0 ? undefined : end), ctx)
      if (verdict) return verdict
    }
  }
  if (SHELLS.has(name) && ctx.depth < 3) {
    const i = args.findIndex(a => (name === 'cmd' ? /^\/c$/i : /^(-[a-z]*c|-command)$/i).test(a))
    if (i >= 0 && i + 1 < args.length) {
      const script = name === 'cmd' ? args.slice(i + 1).map(res).join(' ') : res(args[i + 1]!)
      const verdict = judge(script, ctx.kinds, ctx.depth + 1)
      if (verdict) return verdict
    }
  }
  return undefined
}

/**
 * The targets of the writes a script makes, or undefined when a write's
 * target cannot be read off the text: `open(p, 'w')`, `open(p, mode='w')`,
 * `writeFileSync(p, …)`, `Path(p).write_text(…)`, with `p` a literal or a
 * variable given a literal ([X7-L1-06], [X7-L1-05]).
 */
function writeTargets(command: string, vars: Map<string, string>): string[] | undefined {
  const value = (expr: string): string | undefined => {
    const lit = /^[rbfRBF]{0,2}(['"])(.*)\1$/.exec(expr.trim())
    if (lit) return lit[2]
    return /^[A-Za-z_]\w*$/.test(expr.trim()) ? vars.get(expr.trim()) : undefined
  }
  const exprs: string[] = []
  let calls = 0
  for (const m of command.matchAll(/open\s*\(([^)]*)\)/g)) {
    const parts = m[1]!.split(',')
    if (parts.length < 2 || !/['"](?:[wax]|r\+)[bt+]*['"]/.test(parts.slice(1).join(','))) continue
    calls++
    exprs.push(parts[0]!)
  }
  for (const m of command.matchAll(/\b(?:writeFileSync|appendFileSync|writeFile)\s*\(\s*([^,()]+?)\s*,/g)) {
    calls++
    exprs.push(m[1]!)
  }
  for (const m of command.matchAll(/\.write_(?:text|bytes)\s*\(/g)) {
    calls++
    const p = /Path\s*\(\s*([^()]+?)\s*\)\s*$/.exec(command.slice(0, m.index))
    if (p) exprs.push(p[1]!)
  }
  if (calls === 0 || exprs.length < calls) return undefined
  const out: string[] = []
  for (const e of exprs) {
    const v = value(e)
    if (v === undefined) return undefined
    out.push(v)
  }
  return out
}

/** Why the command is refused, or undefined when it may run. */
export function judge(command: string, kinds?: Kinds, depth = 0): string | undefined {
  const { skel, redir, strings } = parse(command)
  const ctx: Context = { strings, vars: assignments(command), kinds, depth, written: new Set() }

  const redirects = [...redir.matchAll(/(?<![0-9&<>])(>>?)\s*([^\s;&|<>]+)/g)]
    .filter(m => !m[2]!.startsWith('&'))
    .map(m => ({ op: m[1]!, target: resolve(m[2]!, ctx) }))
  for (const r of redirects) ctx.written.add(slashes(r.target))
  for (const words of segments(skel)) {
    const { name, args } = program(words)
    if (name === 'tee') for (const a of args) if (!a.startsWith('-')) ctx.written.add(slashes(resolve(a, ctx)))
  }

  for (const words of segments(skel)) {
    const verdict = judgeWords(words, ctx)
    if (verdict) return verdict
  }

  for (const { op, target } of redirects) {
    if (op === '>>' && LEDGER.test(target)) continue
    if (isProtected(target)) return `a redirect into ${target}`
  }

  if (/\b(python3?|py|node|deno|bun)\b/i.test(skel)) {
    const writes = /open\s*\([^)]*['"](?:[wax]|r\+)[bt+]*['"]|\.write_text\s*\(|\.write_bytes\s*\(|writeFileSync|appendFileSync|\bwriteFile\s*\(/
    if (writes.test(command)) {
      const targets = writeTargets(command, ctx.vars)
      // A target read off the text decides; where one cannot be, any project
      // path the script names is taken as the target.
      const paths = targets ?? [...command.matchAll(/[A-Za-z0-9_.\\/:-]+\.(rs|md|toml|wgsl|ya?ml|sh|json|ts|tsx)\b/gi)].map(m => m[0])
      if (paths.some(isProtected)) return 'a script writing a project file'
    }
  }

  if (/\[(System\.)?IO\.File\]::(Write|Append)All/i.test(command)) {
    const paths = [...command.matchAll(/[A-Za-z0-9_.\\/:-]+\.(rs|md|toml|wgsl|ya?ml|sh|json|ts|tsx)\b/gi)].map(m => m[0])
    if (paths.some(isProtected)) return 'a .NET write into a project file'
  }
  return undefined
}

/**
 * The paths the command's `git add` and `git checkout` name, for the hook to
 * ask the disk about: only the disk tells `git checkout feature/x` (a branch)
 * from `git checkout crates/ondin-app` (a directory's work thrown away).
 */
export function pathspecs(command: string): string[] {
  const { skel, strings } = parse(command)
  const ctx: Context = { strings, vars: assignments(command), depth: 0, written: new Set() }
  const out: string[] = []
  for (const words of segments(skel)) {
    const { name, args } = program(words)
    if (name !== 'git') continue
    const { sub, rest } = subcommand(args.map(a => resolve(a, ctx)))
    if (sub === 'add' || sub === 'checkout') out.push(...positionals(rest))
  }
  return out
}

/** Splits combined short flags (`-Av` → `-A -v`) so each is compared alone. */
function expand(args: readonly string[]): string[] {
  return args.flatMap(a => (/^-[A-Za-z]{2,}$/.test(a) ? [...a.slice(1)].map(c => `-${c}`) : [a]))
}

function subcommand(args: string[]): { sub: string | undefined; rest: string[] } {
  // `git -C dir add -A`: skip the global options before the subcommand.
  let i = 0
  while (i < args.length && args[i]!.startsWith('-')) i += args[i] === '-C' || args[i] === '-c' ? 2 : 1
  return { sub: args[i], rest: args.slice(i + 1) }
}

/** Files that are commonly named with no extension, so not read as a directory. */
const BARE_FILE = /^(LICEN[CS]E|COPYING|NOTICE|AUTHORS|README|CHANGELOG|Makefile|Dockerfile|Justfile|Procfile|Gemfile|Rakefile)([-_.].*)?$/i

/** Whether a pathspec names a directory: by the disk when it was asked, else by its spelling. */
function isDirectory(p: string, kinds: Kinds | undefined): boolean {
  const k = kinds?.get(p)
  if (k !== undefined) return k === 'dir'
  if (/[\\/]$/.test(p)) return true
  const last = p.split(/[\\/]/).pop() ?? p
  return !last.includes('.') && !BARE_FILE.test(last)
}

function judgeGit(args: string[], kinds: Kinds | undefined): string | undefined {
  const { sub, rest: raw } = subcommand(args)
  const rest = sub === 'commit' ? raw : expand(raw)
  const has = (...flags: string[]) => rest.some(a => flags.includes(a))
  const pos = positionals(rest)

  if (sub === 'add') {
    if (has('-n', '--dry-run')) return undefined
    const sweep = pos.some(p => ['.', '..', ':/', ':', '*'].includes(p.replace(/[\\/]+$/, '') || '.'))
    if (has('-A', '--all', '-u', '--update') || sweep) return '`git add` of everything (`-A`, `--all`, `-u`, `.`)'
    const dir = pos.find(p => isDirectory(p, kinds))
    if (dir) return `\`git add\` of a directory (\`${dir}\`): stage the files you changed`
  }
  if (sub === 'commit' && rest.some(a => /^-[A-Za-z]*a[A-Za-z]*$/.test(a) || a === '--all')) return '`git commit -a`'
  if (sub === 'checkout') {
    if (has('-b', '-B', '--orphan')) return undefined
    if (has('-f', '--force')) return '`git checkout --force`, which discards local changes'
    if (raw.includes('--') || pos.includes('.')) return '`git checkout` of a file'
    for (const p of pos) {
      const k = kinds?.get(p)
      if (k === 'file' || k === 'dir') return '`git checkout` of a file'
      // Unasked, a name with an extension is a file; a slashed name with none
      // is as likely a branch (`origin/main`, `feature/x`) and goes through.
      if (k === undefined && /\.[A-Za-z][A-Za-z0-9]*$/.test(p)) return '`git checkout` of a file'
    }
  }
  if (sub === 'switch' && has('-f', '--force', '--discard-changes')) return '`git switch --discard-changes`'
  if (sub === 'restore') {
    // Only a restore of the index alone leaves the working file be.
    if (!has('-S', '--staged') || has('-W', '--worktree')) return '`git restore` of a working-tree file'
  }
  if (sub === 'reset' && rest.includes('--hard')) return '`git reset --hard`'
  if (sub === 'clean' && has('-f', '--force') && !has('-n', '--dry-run')) return '`git clean -f`'
  if (sub === 'stash') {
    // Only these leave the working tree alone; `push`, `save`, a bare stash
    // and every flag-led spelling (`-u`, `--include-untracked`) stash it.
    const reads = ['list', 'show', 'pop', 'apply', 'drop', 'branch', 'clear', 'create', 'store']
    if (!reads.includes(rest[0] ?? '')) return '`git stash` of the working tree'
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
