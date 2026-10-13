import { describe as group, expect, test } from 'claude-code/testing'
import { judge } from '../hooks/judge.ts'

const refused = [
  // The splices CLAUDE.md records.
  "sed -i 's/old/new/' crates/ondin-app/src/app.rs",
  "sed -i.bak -e 's/a/b/' docs/roadmap.md",
  'perl -0pi -e "s/a/b/g" crates/ondin-core/src/lib.rs',
  "gawk -i inplace '{print}' Cargo.toml",
  "cat >> crates/ondin-app/src/app.rs <<'EOF'\n#[test]\nfn x() {}\nEOF",
  'echo hi > docs/roadmap.md',
  'cargo fmt && echo x | tee CLAUDE.md',
  "python - <<'EOF'\np='crates/ondin-app/src/app.rs'\ns=open(p).read()\nopen(p,'w').write(s.replace('a','b'))\nEOF",
  'Set-Content -Path crates\\ondin-app\\src\\app.rs -Value $x',
  "Get-Content x | Out-File docs\\decisions.md",
  "[IO.File]::WriteAllText('C:\\Users\\jonid\\Projects\\ondin\\docs\\vm.md', $t)",
  // The git commands that sweep or discard.
  'git add -A',
  'git add . && git commit -m "x"',
  'git -C C:/repo add --all',
  'git commit -am "wip"',
  'git checkout -- crates/ondin-app/src/app.rs',
  'git checkout crates/ondin-app/src/app.rs',
  'git checkout .',
  'git restore docs/roadmap.md',
  'git reset --hard HEAD',
  'git clean -fd',
  'git stash',
  // [X7-L1-01]: the same harms in their other spellings.
  'git clean --force -d',
  'git clean -xf',
  'git restore --staged --worktree crates/ondin-app/src/app.rs',
  'git restore -SW crates/ondin-app/src/app.rs',
  'git checkout -f main',
  'git checkout --force main',
  'git switch --discard-changes main',
  'git switch -f main',
  'git stash --include-untracked',
  'git stash -k',
  'git stash push -m wip',
  'git add -Av',
  'git add -fA',
  'git add crates/',
  'git add docs crates',
  'git add ..',
  // [X7-L1-02]: an in-place edit inside a loop, a conditional or a wrapper.
  "for f in crates/ondin-app/src/*.rs; do sed -i 's/a/b/' $f; done",
  "find crates -name '*.rs' -exec sed -i 's/a/b/' {} +",
  "find crates -name '*.rs' -execdir perl -pi -e 's/a/b/' {} \\;",
  "find crates -name '*.rs' | xargs sed -i 's/a/b/'",
  "find crates -name '*.rs' -print0 | xargs -0 -n 1 sed -i 's/a/b/'",
  "if true; then sed -i 's/a/b/' crates/x.rs; fi",
  "(sed -i 's/a/b/' crates/x.rs)",
  "time sed -i 's/a/b/' crates/x.rs",
  "echo $(sed -i 's/a/b/' crates/x.rs)",
  "bash -c \"sed -i 's/a/b/' crates/x.rs\"",
  "bash -lc 'git add -A'",
  "pwsh -Command \"Set-Content crates/x.rs -Value 1\"",
  // [X7-L1-05]: whole-file writes with a non-literal target, or by another program.
  "$p = 'crates/ondin-app/src/app.rs'; (Get-Content $p) -replace 'a','b' | Set-Content $p",
  'cp C:/scratch/app.rs crates/ondin-app/src/app.rs',
  'Copy-Item C:/scratch/app.rs -Destination crates/ondin-app/src/app.rs',
  'Copy-Item C:/scratch/app.rs crates/ondin-app/src/app.rs',
  'mv /tmp/x.rs crates/ondin-app/src/app.rs',
  'Move-Item $env:TEMP/x.rs crates/ondin-app/src/app.rs',
  'New-Item -Force crates/x.rs',
  "ruby -pi -e 'gsub(/a/,\"b\")' crates/x.rs",
  "python -c \"p='crates/x.rs'; s=open(p).read(); open(p,mode='w').write(s)\"",
  "p=docs/roadmap.md; echo x > $p",
  "awk 'NR<10' crates/x.rs > /tmp/x.rs && mv /tmp/x.rs crates/x.rs",
  'head -n 40 crates/x.rs > C:/tmp/t.rs && cat C:/tmp/ins.rs >> C:/tmp/t.rs && cp C:/tmp/t.rs crates/x.rs',
  "awk '{print}' crates/x.rs > /tmp/x.rs.bak; cp /tmp/x.rs.bak crates/x.rs",
]

const allowed = [
  // Reading is fine, and so is writing scratch.
  "sed -n '10,40p' crates/ondin-app/src/app.rs",
  'grep -n "fn foo" crates/x.rs | head',
  'cargo test --workspace --release 2>&1 | tee /tmp/release-run.log',
  'cargo build -p ondin-app > /dev/null 2>&1',
  "cat > C:/Users/jonid/AppData/Local/Temp/claude/x/scratchpad/probe.md <<'EOF'\nhi\nEOF",
  'echo $x > $TMPDIR/out.json',
  'git diff --stat > notes.txt',
  // A commit message may mention what it fixed.
  'git commit -m "fix: stop a sed -i in the release script; never git add -A"',
  "git commit -F - <<'EOF'\ndocs: say why sed -i and git checkout -- are banned\nEOF",
  // The staging the commit skill uses, and the git that changes nothing.
  'git add crates/ondin-app/src/app.rs docs/decisions.md',
  'git restore --staged crates/ondin-app/src/app.rs',
  'git checkout -b feature',
  'git checkout main',
  'git checkout v0.4.1',
  'git checkout-index -a --prefix=C:/tmp/staged/',
  'git apply --cached --recount patch.diff',
  'git stash list',
  "python -c \"print(open('crates/x.rs').read().count('fn'))\"",
  // [X7-L1-04]: Rust syntax in a quoted search pattern is not a redirect.
  "grep -n '->' crates/ondin-core/src/lib.rs",
  "rg -c '=>' crates/ondin-core/src/lib.rs",
  "grep -n 'Option<Node>' crates/ondin-core/src/node.rs",
  'rg -n "fn x() -> Vec<u8>" crates/ondin-core/src/lib.rs',
  // [X7-L1-06]: the review method's own append, a script writing scratch, a
  // slashed branch, a dry run.
  'cat C:/Users/jonid/AppData/Local/Temp/claude/x/scratchpad/X7.md >> review/release-v0.5.0/passes/X7.md',
  "python -c \"s=open('crates/x.rs').read(); open('C:/Users/jonid/AppData/Local/Temp/o.txt','w').write(s)\"",
  'git checkout origin/main',
  'git checkout feature/x',
  'git add -A --dry-run',
  'git add -n .',
  'git clean -n',
  'git clean --dry-run -d',
  // Counter-cases for the new refusals: the same programs used harmlessly.
  'git stash pop',
  'git stash apply stash@{0}',
  'git stash show -p',
  'git switch main',
  'git switch -c feature',
  'git checkout -bfix main',
  'git add LICENSE .gitignore Cargo.lock',
  'git add crates/ondin-app/src/*.rs',
  'perl -Ilib -e "print 1"',
  'cp crates/ondin-app/src/app.rs C:/Users/jonid/AppData/Local/Temp/app.rs',
  'Copy-Item crates/x.rs $env:TEMP/x.rs',
  'mv crates/a.rs crates/b.rs',
  'New-Item -ItemType Directory -Force crates/new',
  "$p = 'C:/tmp/o.txt'; 'x' | Set-Content $p",
  'for f in crates/*.rs; do grep -c fn $f; done',
  "find crates -name '*.rs' -exec grep -l foo {} +",
  "find crates -name '*.rs' | xargs grep -l foo",
  'time cargo build -p ondin-app',
  'bash -c "cargo test -p ondin-app"',
  'git commit -m "docs: never bash -c \'sed -i x\' over crates/x.rs"',
  // Found replaying every command of this project's session transcripts
  // through the new judge: the backup a flip-check takes, a release review's
  // passes copied in, a snapshot of the ledger, and redirections that are not
  // pathspecs.
  'S="$TEMP/claude/x/scratchpad"; cp crates/ondin-app/src/app.rs "$S/app.rs.bak"',
  'cp crates/ondin-app/src/app.rs "$SP/app.rs.bak"',
  'S=/c/Users/jonid/AppData/Local/Temp/claude/x/scratchpad; D=/c/Users/jonid/Projects/ondin/review/release-v0.4.1; cp $S/X1.md $D/passes/X1.md',
  'cp review/findings.md review/.snapshots/findings-S13.1.md',
  // Copying a backup back is that backup's use (CLAUDE.md: copy a file to the
  // scratchpad before an experiment you expect to revert).
  'S=/c/Users/jonid/AppData/Local/Temp/claude/x/scratchpad; cp $S/app.rs crates/ondin-app/src/app.rs',
  'cp /tmp/boolean.bak crates/ondin-core/src/boolean.rs',
  'cp /tmp/theme.orig2.rs crates/ondin-app/src/theme.rs',
  'git add crates/ondin-app/src/app.rs 2>&1 >/dev/null',
  'git checkout -q $(git rev-parse HEAD)',
]

group('judge', () => {
  for (const command of refused) {
    test(`refuses: ${command.split('\n')[0]}`, async () => {
      expect(judge(command), command).toBeDefined()
    })
  }
  for (const command of allowed) {
    test(`allows: ${command.split('\n')[0]}`, async () => {
      expect(judge(command), command).toBeUndefined()
    })
  }
})

test('a refused command never reaches the shell', async ($, on) => {
  const ran: string[] = []
  on('tool.call', { tool: 'Bash' }, (_$, e) => {
    if (e.tool === 'Bash') ran.push(e.command)
    return { result: {} as never }
  })
  const r = await $.tool.call({ tool: 'Bash', command: 'git add -A' })
  expect(r.deny ?? r.text).toContain('edit-guard: refused `git add` of everything')
  await $.tool.call({ tool: 'Bash', command: 'git status' })
  expect(ran).toEqual(['git status'])
})

test('a path git would read as a file or a directory is told apart from a branch by the file system', async ($, on) => {
  // [X7-L1-01], [X7-L1-06]: `git checkout <dir>` discards a directory's work
  // and `git checkout feature/x` switches branch; only the disk knows which.
  const ran: string[] = []
  const disk: Record<string, 'file' | 'dir'> = { 'crates/ondin-app': 'dir', crates: 'dir', NOTES: 'file' }
  // The engine hands the hook the path resolved against the working directory.
  const at = (path: string) => Object.entries(disk).find(([p]) => path.replace(/\\/g, '/').endsWith(`/${p}`))?.[1]
  on('fs.exists', (_$, e) => ({ value: at(e.path) !== undefined }))
  on('fs.stat', (_$, e) => ({ value: { kind: at(e.path) ?? 'other', size: 0, mtimeMs: 0, isLink: false } }))
  on('tool.call', { tool: 'Bash' }, (_$, e) => {
    if (e.tool === 'Bash') ran.push(e.command)
    return { result: {} as never }
  })
  for (const command of ['git checkout crates/ondin-app', 'git add crates', 'git checkout feature/x', 'git add NOTES']) {
    await $.tool.call({ tool: 'Bash', command })
  }
  expect(ran).toEqual(['git checkout feature/x', 'git add NOTES'])
})

test('the guard fails open: when the disk cannot be asked, the command runs, once', async ($, on) => {
  // [X7-L6-01]: the `.catch` in register.ts is a decision, so it is pinned.
  // Nothing here answers `fs.exists`, so the kit's bottom hook throws and the
  // guard's own hook throws with it; the command still runs.
  const ran: string[] = []
  on('tool.call', { tool: 'Bash' }, (_$, e) => {
    if (e.tool === 'Bash') ran.push(e.command)
    return { result: {} as never }
  })
  const r = await $.tool.call({ tool: 'Bash', command: 'git checkout feature/x' })
  expect(r.deny).toBeUndefined()
  expect(ran).toEqual(['git checkout feature/x'])
})
