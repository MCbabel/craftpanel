# Contributing

## Getting a tree that builds

```bash
pnpm install
pnpm --filter @craftpanel/web build
cargo build
```

The interface has to be built before the panel is: `rust-embed` reads `web/dist` at compile time,
so `cargo` fails on a tree where the frontend has never been built.

Node 22 and pnpm 11; Rust 1.85 or newer.

## The checks

These ten are what CI runs, and what a change is expected to keep green — a release too, since
`.github/workflows/release.yml` runs the same file:

```bash
cargo test -p craftpanel-proto
cargo test -p craftpanel-helper
cargo test -p craftpanel
cd web && pnpm test
scripts/check-types.sh
scripts/check-no-branding.sh
python3 scripts/comments-test.py
scripts/check-no-comments.sh
scripts/check-shell.sh
python3 scripts/check-doc-lines.py
```

The three `cargo` calls are separate on purpose. A workspace-wide build links three binaries at
once, which on a small machine that is also serving a live panel is how the OOM killer gets
involved.

Eight tests carry `#[ignore]` because they talk to the network: Modrinth, PaperMC, playit.gg,
Resend, Adoptium. They are not part of CI. Run them with `cargo test -- --ignored` when you touch
what they cover. Tests that need root, or a group of their own, print `skipped: …` and pass; on a
machine where they can run, they run.

`scripts/check-types.sh` runs `vue-tsc` and counts only errors in our own files. Modrinth's
vendored sources do not typecheck under our configuration and are not ours to fix — see below.

`scripts/comments-test.py` puts the guard's own parser on the bench: 43 cases in which `//` is not
a comment — an address inside a string, a regular expression that looks like a division, three
languages in one `.vue`. It runs before the guard that stands on it, because a parser that has
gone blind reports a clean tree.

## Four rules

**`vendor/modrinth/**` is not ours.** It is somebody else's GPL work, vendored as it stands. No
edits, no reformatting, no removing comments. The same goes for `node_modules`. If something in
there is wrong for us, work around it on our side.

**Modrinth's brand may not come back.** The logo, the wordmark, the Rinthbot mascot and the
Modrinth Servers icon are deleted and stay deleted. `scripts/check-no-branding.sh` looks for them
by path, by file name and by identifier. See [COPYING.md](COPYING.md).

**No comments in our own code.** `crates/**` and `web/src/**` carry no `//`, `///`, `//!`, `/* */`
or `<!-- -->`. This is not a style preference and it is not "the code should speak for itself":
the reasons behind a decision are the most valuable thing this project has, and a comment is the
place where they go stale unread. They belong in `docs/`, next to the decision they justify, where
the next person looks. `scripts/check-no-comments.sh` enforces it;
`python3 scripts/comments.py --remove` does the mechanical part, and
`python3 scripts/comments.py --diff` shows what it would take out. The few things that
have to stay — `clap` help text, `@ts-expect-error`, and their kind — are listed under `TOOLING`
in `scripts/comments.py`.

**A migration that has been applied is frozen, comments and all.** `sqlx::migrate!`
(`crates/craftpanel/src/db.rs:27`) checksums the whole file, so one character changed anywhere in
`crates/craftpanel/migrations/` — in a comment just as much as in a statement — makes every panel
that has already run it refuse to start. That is why the head of `0002_schema.sql` points at
`docs/api/VERTRAG.md`, a name the document lost when it became `docs/api/CONTRACT.md`: the
reference is stale and stays stale, and tidying it up would take a running panel down. What has to
change gets a new migration; the old ones are a record of what happened, not a text you edit.

## Language

The README, this file and `docs/` are English. The documents under `docs/` are the working
documents the project is written from. A change that has a reason worth keeping adds that reason
to the document it belongs to.

The interface is English and German. English is what stands in the source: every message carries
an id and its `defaultMessage`, and `scripts/locale-extract.py` collects them into
`web/src/locales/en-US/index.json`. The German is `web/src/locales/de-DE/index.json`. Whoever adds
a message maintains both catalogues. `web/src/locales/catalogues.test.ts` fails on a message the
source has and a catalogue has not, on an id the source no longer has, and on a German text whose
placeholders differ from the English. It is an ordinary `vitest` file, so `pnpm test` runs it, and
CI with it — a half-translated interface does not get through.

## Style

Follow what is already there rather than what a formatter would do; the tree is hand-formatted in
places and there is deliberately no `rustfmt.toml`. Tests are named as sentences that state what
must hold — `a_withdrawn_connection_is_written_down_and_the_key_file_stays` — because the name is
what a failure prints.

## Releasing

`scripts/release.sh` builds the interface, builds the two binaries, re-checks the branding and
writes `dist/craftpanel-linux-x86_64.tar.gz` plus its `.sha256` — `linux-aarch64` for the other
architecture. `install.sh` downloads exactly those file names from the GitHub release tagged
`v<version>`.

Publishing is a tag and nothing else. Bump `version` in `Cargo.toml`, commit, push `v<version>`, and
`.github/workflows/release.yml` runs the script on an x86_64 runner and on an arm64 one and makes
the release out of both bundles and both checksums. It needs no API token: the workflow publishes
with the `GITHUB_TOKEN` Actions hands it, which is what `permissions: contents: write` on the
publishing job is for. Before either runner starts it refuses a tag that disagrees with `Cargo.toml`
— a v0.2.0 holding a binary that answers 0.1.0 is found months later, if at all — and it refuses a
tag that already has a release, so nothing published is ever quietly replaced; a botched one has to
be deleted by hand before that tag can serve again. Running the script yourself is still how you get
a bundle to try before tagging (`CRAFTPANEL_BUNDLE=`), and the only way to serve an architecture no
release covers.

Nothing is published that has not been tested. `release.yml` calls `.github/workflows/ci.yml`
rather than repeating its steps, so a tag goes through the same guards, the same web tests and the
same three `cargo test` runs a pull request goes through, on the tagged commit and on no other one
— a run started by hand from a branch is refused, because the tests would then be testing something
else. It puts the better part of an hour in front of the build. A tarball a stranger installs as
root has not earned less.

Every action in `.github/workflows/` hangs on a full commit SHA with its version in the comment
beside it. `@v4` is whatever its owner points it at today, and one change under such a tag in
`tj-actions/changed-files` printed the secrets of thousands of repositories into their build logs.
To move a pin, read the SHA off the tag instead of guessing it —
`git ls-remote --tags https://github.com/actions/checkout` prints one line per tag and the `^{}`
line of an annotated tag is the commit it names — and change the comment in the same edit as the
SHA. `dtolnay/rust-toolchain` hangs on its `stable` branch, which has no version to quote: its
comment carries the date that branch head was read, and the compiler it installs is whatever rustup
calls stable on the day the job runs. A pin nobody ever moves is its own kind of stale, so this is
worth a pass whenever the workflows are opened for something else.

The build target is `<arch>-unknown-linux-musl` unless `CRAFTPANEL_TARGET` says otherwise, and the
script refuses a bundle whose binaries came out dynamically linked. That is what lets one file serve
every distribution: built against a current glibc, the panel answers Debian 12 with
`GLIBC_2.39 not found`, and there is no way out of that from inside a `curl | sudo bash`. Two things
keep it possible — the TLS is rustls, so no OpenSSL is wanted, and nothing in the tree calls
`getpwnam` and its relatives, which static musl cannot serve; the helper makes system accounts with
`useradd` and reads `/etc/passwd` and `/etc/group` itself. Whoever changes either of those has to
weigh the release against it. On a fresh machine the target needs
`rustup target add x86_64-unknown-linux-musl` and `apt install musl-tools`.

The file name is not the Rust target triple on purpose: how the binaries are linked is a decision
that may be revisited, and the name people download should not change with it. `install.sh`
`detect_arch` writes the same two names; both sides have to keep saying the same thing, or the
installer downloads nothing and the user only reads `download failed`.

The `.sha256` beside each bundle is load-bearing in the same way. `install.sh` refuses to install a
bundle it cannot check, so a release carrying `craftpanel-linux-x86_64.tar.gz` without
`craftpanel-linux-x86_64.tar.gz.sha256` stops every installation of that version, with an error
that names the release page. That is deliberate — a missing sum used to be a warning nobody read
while the bytes went in as root — and it makes a half-finished upload something to notice rather
than something to shrug at. Look at the release page before telling anybody the version is out.

Each bundle is signed before the release is created. `actions/attest` in the publishing job asks
GitHub's OIDC provider for a token that names this repository, this workflow file, the tagged
commit and that run; Sigstore issues a certificate against exactly that and against nothing else,
and the signed statement is stored under the repository rather than laid down beside the asset,
which is the whole difference between it and the `.sha256`. That is what `id-token: write` and
`attestations: write` on the publishing job are for, and why the build job has neither: it runs a
toolchain installer and a package manager, and a token that can sign in this project's name has no
business in that job.

Make the check once yourself after a release. A failing step is loud, but whether the identity a
reader is told to type actually matches the certificate is not something the run tells you:

```bash
gh attestation verify craftpanel-linux-x86_64.tar.gz \
  --repo MCbabel/craftpanel \
  --signer-workflow MCbabel/craftpanel/.github/workflows/release.yml
```

What that proves, what the `.sha256` proves, and what neither of them proves is written out in
[SECURITY.md](SECURITY.md). The release notes carry the short version, for the people downloading.

## Licence and security

Contributions are GPL-3.0-only, like the rest. If you find a security problem, open a private
advisory (Security → Report a vulnerability) rather than an issue; the helper runs as root, so
anything about it wants a quiet fix first. What belongs in such a report, and which parts of the
tree are the sensitive ones, is in [SECURITY.md](SECURITY.md).
