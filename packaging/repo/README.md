# The Linux package repositories

Ondin publishes a signed **APT** repository and a signed **DNF/zypper**
repository at <https://fadion.github.io/ondin>, so a `.deb` or `.rpm` install
is carried forward by `apt-get upgrade` / `dnf upgrade` instead of by re-running
`install.sh`. This directory builds them; `.github/workflows/pages.yml` runs it
after every tagged release.

```
build-site.sh        the whole site: collects packages, calls the two below,
                     writes the client config, the key and the landing page
build-apt-repo.sh    pool/ + dists/ + a signed InRelease
build-rpm-repo.sh    signed packages + repodata/ + a signed repomd.xml.asc
verify-site.sh       reads a built site back the way apt and dnf would
verify-site.py       the index half of that check
index.html.in        the landing page, with __BASE_URL__ / __VERSION__ /
                     __FINGERPRINT__ filled in at build time
lib.sh               the guard every builder's `rm -rf` goes through
```

The packages themselves are built by `packaging/linux/` (`build-deb.sh`,
`build-rpm.sh`, both around one `stage-payload.sh`) and attached to each GitHub
Release; this directory only ever reads them back from there.

The whole arrangement is ported from Schemaic, the same author's SQL editor,
which has run it through several releases. Where a comment here cites a
measurement, it says whose.

## The shape of it

**The GitHub Release assets are the source of truth; the site is derived.** Each
run downloads the `.deb` and `.rpm` from the five most recent releases that have
them and rebuilds everything from nothing.

That is what keeps this cheap to own. There is no `gh-pages` branch — a branch
accumulating every release's packages would be cloned by everyone who ever
clones this repository, forever, and deleting the files later would not shrink a
single clone that already exists. An Actions deployment has no git history, so
the published size is the only size: five releases' worth of packages against
GitHub Pages' 1 GB limit, with a 100 GB/month bandwidth allowance above it.
(Schemaic's run about 40 MB a release; Ondin's have not been measured yet —
`du -sh` is the last line `build-site.sh` prints.)

It also means **a repository that has gone wrong is repaired by running the
workflow again**, from the Actions tab. There is no accumulated state to unpick,
because the previous run's output is never read.

## One-time setup

**Done on 2026-10-03** — the key exists, both secrets are set, Pages is on, and
the fingerprint (`456BA115B1DA8E1DBF93299454AC6F3C915023EA`) is in both files.
The steps stay here because they are also the procedure for **rotating** the
key. Without them nothing here works, and the failure is loud on purpose:
`pages.yml` refuses to publish without the key, and `install.sh` refuses its
repository routes while it carries anything but a fingerprint.

### 1. The signing key

Not optional, and not the same thing as code signing. A GPG key here says *this
package came from this repository and arrived unaltered* — it vouches for no
identity, costs nothing, and is what `apt` and `dnf` actually check. The
alternative is not "an unsigned repository": it is telling every user to write
`[trusted=yes]`, which is a worse posture than the direct download it would be
replacing. So the workflow refuses to publish without one.

On any machine with `gpg` (WSL will do), in an isolated keyring so this never
lands in your personal one:

```bash
export GNUPGHOME="$(mktemp -d)" && chmod 700 "$GNUPGHOME"
echo allow-loopback-pinentry > "$GNUPGHOME/gpg-agent.conf"

# Pick a passphrase and keep it; you will need it again below.
printf '%s' 'YOUR-PASSPHRASE' > "$GNUPGHOME/pass" && chmod 600 "$GNUPGHOME/pass"

gpg --batch --pinentry-mode loopback --passphrase-file "$GNUPGHOME/pass" \
    --quick-generate-key "Ondin package signing <fadion@users.noreply.github.com>" \
    rsa4096 sign never
```

`rsa4096` rather than an elliptic-curve key because RHEL-era `rpm` cannot verify
EdDSA signatures, and `never` rather than an expiry because an expired key breaks
`apt update` for every user on a date rather than on a release — a silent,
scheduled outage with nothing to do about it but rotate.

Then export it and hand both halves to Actions:

```bash
fpr=$(gpg --list-secret-keys --with-colons | awk -F: '/^fpr:/ {print $10; exit}')
echo "$fpr"   # the fingerprint — step 2 needs it

gpg --batch --pinentry-mode loopback --passphrase-file "$GNUPGHOME/pass" \
    --armor --export-secret-keys "$fpr" > ondin-signing-key.asc

gh secret set GPG_PRIVATE_KEY --repo fadion/ondin < ondin-signing-key.asc
gh secret set GPG_PASSPHRASE --repo fadion/ondin   # paste the passphrase
```

**Back `ondin-signing-key.asc` up somewhere you will still have it in two
years, then delete the working copy.** Losing it means generating a new key, and
every machine that has ever installed from this repository then fails
verification on the next `apt update` until its user imports the replacement by
hand. There is no way to reach those people to tell them.

The public key is not committed here on purpose. The site exports it from the
private key on every run, so there is exactly one copy and nothing that can
quietly disagree with what is actually signing the packages.

### 2. The fingerprint, in exactly two files

**The fingerprint is the deliberate exception to "one copy"**, and it is written
by hand in two places and nowhere else:

1. **`install.sh`** — replace the placeholder so the line reads, in full,

   ```bash
   KEY_FINGERPRINT="<the 40 upper-case hex digits>"
   ```

   with nothing after the closing quote. Until then the script refuses the apt
   and dnf routes with a message saying why, and installs nothing.

2. **The top-level `README.md`** — a line holding the fingerprint **and nothing
   else** (no prose, no backticks, no leading spaces), which in practice means a
   line of its own inside a fenced code block.

That is the whole list. The landing page's copy is not a third one: `build-site.sh`
derives it from the key that signed the build, every run.

`pages.yml` reads the fingerprint back out of the built `index.html` and checks
it with `grep -qx` — a **whole-line** match — against both files: `README.md`
must contain a line that *is* the fingerprint, and `install.sh` must contain the
line `KEY_FINGERPRINT="<fingerprint>"`. Either one missing fails the publish.
So the first publish fails until both are filled in, which is correct: a site
whose key nothing independent vouches for is the thing this check exists to stop.

Why publish a fingerprint at all: it is printed in the README because the
by-hand `dnf` route asks the user to approve a fingerprint, and one they can only
compare against the same server the key arrived from is not a comparison — this
repository's history is a channel that server does not control. `install.sh`
carries the same constant as `KEY_FINGERPRINT` and **refuses** a key that does
not match it, which is that same independent channel applied to the route the
README leads with (`curl … | bash`): the script is served from
`raw.githubusercontent.com` and the key from `fadion.github.io`, so the constant
is a check the key's own origin cannot forge.

Those two make a rotation sharper than it looks. The README merely becoming wrong
is a misinformed reader; `install.sh` becoming wrong is **every new install
failing**, and every machine already installed continuing to trust the old key
until someone re-runs it. So a rotation is: new key → `install.sh` and README
updated in the same commit → published → said in the release notes. `pages.yml`
compares both against the key that actually signed the build and fails the
publish if either has drifted, which is the backstop, not the procedure.

### 3. Pages

Once, so that deployments from Actions are accepted:

```bash
gh api -X POST repos/fadion/ondin/pages -f build_type=workflow
```

Or Settings → Pages → Build and deployment → Source → **GitHub Actions**.

Then let a tag deploy. Switching the source creates a `github-pages` environment
that accepts **`main` only**, and the publish a release triggers runs on the
*tag*, so it is refused before a step runs — with the release itself complete
beside it. That is what v0.4.0's first publish hit; it was published by running
**Pages** from `main` and the rule added afterwards:

```bash
gh api -X POST repos/fadion/ondin/environments/github-pages/deployment-branch-policies -f name='v*' -f type=tag
```

Or Settings → Environments → `github-pages` → Deployment branches and tags → add
the tag rule `v*`.

### 4. First publish

The workflow runs itself after the next tagged release, called from
`release.yml` with that tag as `expect_version`. To stand the site up before
then, run **Pages** from the Actions tab — it builds from releases that already
exist, so it needs no new tag. It needs at least one release that carries a
`.deb` and an `.rpm`; v0.3.0 and the tags before it carry none, and the build
refuses to publish an empty repository.

## Testing it

`verify-site.sh` runs in the workflow before anything is deployed, and it is the
only check there is. What it catches is not a wrong answer from a function but
metadata that disagrees with the packages beside it — a `Packages` file naming a
`.deb` that was pruned, a `Release` still describing the previous run's
`Packages.gz`. Every tool involved reports success for those, and they surface on
a user's machine at install time. So the check is the one a client performs:
verify the signatures against the *published* public key, then confirm every file
the indexes name exists and hashes to what they claim.

To build the site by hand — needs `gh`, `apt-utils`, `createrepo-c`, `rpm`,
`gpg` and `python3`:

```bash
export GPG_KEY_ID=<fingerprint> GPG_PASSPHRASE_FILE=/path/to/pass
export ONDIN_REPO_URL=http://localhost:8000 ONDIN_RETAIN=2
bash packaging/repo/build-site.sh /tmp/site
bash packaging/repo/verify-site.sh /tmp/site
```

The output directory is emptied first, and only if it is new, empty, or was
written by these builders before (`lib.sh`'s `reset_dir` refuses anything else).

To then install from it the way a user would, serve it and point a container at
it — this is the end-to-end check:

```bash
(cd /tmp/site && python3 -m http.server 8000) &
docker run --rm -it --network host debian:12 bash -c '
  apt-get update && apt-get install -y curl ca-certificates
  curl -fsSL http://localhost:8000/ondin-archive-keyring.gpg \
    -o /usr/share/keyrings/ondin-archive-keyring.gpg
  curl -fsSL http://localhost:8000/ondin.sources \
    -o /etc/apt/sources.list.d/ondin.sources
  apt-get update && apt-get install -y ondin && dpkg -L ondin'
```

⚠️ `ondin.sources` names the URL the site was built for, so build it with
`ONDIN_REPO_URL=http://localhost:8000` as above for this to resolve. `install.sh`
itself always points at the real site, which makes this the check of the
packages and the repository, not of the script.

## What users do with it

Documented on the generated landing page. The parts that are interface, and
cannot be reworded without breaking somebody:

- `https://fadion.github.io/ondin/deb` and `/rpm` — written into every user's
  source list. Moving either one stops updates for every existing install,
  silently. This is the same class of permanence as the Velopack channel names.
- `Origin: Ondin`, `Suite: stable` — what a Debian user writes in
  `Unattended-Upgrade::Allowed-Origins` as `"Ondin:stable"`.
- `/usr/share/keyrings/ondin-archive-keyring.gpg` — the path `Signed-By`
  names in the published `.sources` file.

`pages.yml` greps every one of these out of the files that carry them before it
builds, and fails if any has moved.

Nothing upgrades on its own without the user asking, on either family. What the
repositories buy is that asking is the command they already run for everything
else.
