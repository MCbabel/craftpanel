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

These eight are what CI runs, and what a change is expected to keep green:

```bash
cargo test -p craftpanel-proto
cargo test -p craftpanel-helper
cargo test -p craftpanel
cd web && pnpm test
scripts/check-types.sh
scripts/check-no-branding.sh
python3 scripts/comments-test.py
scripts/check-no-comments.sh
```

The three `cargo` calls are separate on purpose. A workspace-wide build links three binaries at
once, which on a small machine that is also serving a live panel is how the OOM killer gets
involved.

Eight tests carry `#[ignore]` because they talk to the network: Modrinth, PaperMC, playit.gg,
Resend. They are not part of CI. Run them with `cargo test -- --ignored` when you touch what they
cover. Tests that need root, or a group of their own, print `skipped: …` and pass; on a machine
where they can run, they run.

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
writes `dist/craftpanel-<target>.tar.gz` plus its `.sha256`. `install.sh` downloads exactly those two
file names from the GitHub release tagged `v<version>`, so a release is: bump `version` in
`Cargo.toml`, run the script, and attach both files to the tag.

## Licence and security

Contributions are GPL-3.0-only, like the rest. If you find a security problem, open a private
advisory (Security → Report a vulnerability) rather than an issue; the helper runs as root, so
anything about it wants a quiet fix first. What belongs in such a report, and which parts of the
tree are the sensitive ones, is in [SECURITY.md](SECURITY.md).
