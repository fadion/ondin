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
