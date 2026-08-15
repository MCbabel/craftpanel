# CraftPanel

[![CI](https://github.com/MCbabel/craftpanel/actions/workflows/ci.yml/badge.svg)](https://github.com/MCbabel/craftpanel/actions/workflows/ci.yml)

A panel for running Minecraft servers on **one** machine. No daemon, no message queues, no
containers: one program, one file, one `curl` command.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/MCbabel/craftpanel/HEAD/install.sh | sudo bash
```

The installer asks for the web port, the port range for game servers and an administrator name. It
creates the system accounts and the two systemd units, and prints the generated password **once**.

Run it again and it offers three things: update, uninstall, or nothing. An update leaves running
Minecraft servers running; see [The supervisor](#the-supervisor). An uninstall asks separately
whether the worlds and the database should go too.

**Requirements:** Linux with systemd and cgroup v2, and a Java runtime (21 for current game
versions). The installer checks both and says what is missing.

Before the first release is published there is nothing for the installer to download. Build a
bundle yourself and hand it to the same script:

```bash
scripts/release.sh
sudo CRAFTPANEL_BUNDLE=dist/craftpanel-x86_64-unknown-linux-gnu.tar.gz ./install.sh
```

## What it does

| | |
|---|---|
| **Servers** | create, start, stop, restart, kill; live console and live metrics |
| **Loaders** | Vanilla, Paper, Folia, Purpur, Leaf, Fabric, Velocity — one download each, checksum verified |
| **Content** | install and update mods, plugins and modpacks straight from Modrinth, one at a time or in bulk |
| **Files** | browse, edit, upload, unpack, move, download |
| **Backups** | create, restore, schedule; the world is told to save and hold still first |
| **Access** | two panel roles, three per-server roles over ten permission bits, invitations, audit log |
| **Sign-up** | off, open, or open with an administrator's approval — with e-mail verification either way |
| **Limits** | memory, CPU, processes and disk per user, set by an administrator |
| **Public address** | one playit.gg tunnel per server, through the user's own playit.gg account |
| **Mail** | invitations, e-mail verification and password resets through Resend |
| **Off-site backups** | upload backups into the user's own Google Drive |

## What it does not do

- NeoForge, Quilt and Forge are not wired up. They appear in the loader list and answer
  `502 upstream_unavailable`: "not wired up in this build yet". Only the seven loaders above are
  built. Spigot and CraftBukkit are deliberately out (they may not be redistributed; they would
  have to be compiled with BuildTools on the machine), and so is Bedrock.
- One machine. There is no agent, no node, nothing to join. The panel and the servers it runs live
  on the same host.
- Linux only. System users, cgroup v2 and systemd are load-bearing.
- No hosting business. No billing, no plans, no subdomains, no DNS, no node migration, no SFTP,
  although the interface it borrows has all of them. Those belong to a host. This is a tool for
  your own machine.

## What you have to set up yourself

Three areas need an account somewhere else. Each of them is off until you set it up, and being off
is a normal state: the panel says so and calls nobody.

- Google Drive backups need your own Google Cloud project. There is no shared client id and
  there cannot be one. Create a project, enable the Drive API, fill in the consent screen
  (external, scope `drive.file`), **publish it to production** — on *Testing* Google expires every
  refresh token after seven days — and create an OAuth client of type "TVs and Limited Input
  devices". The admin page lists the five steps with their addresses. Until then, backups stay on
  the machine. Details in `docs/DRIVE.md`.
- Mail needs a Resend key, and real users need a verified domain. Without a verified domain
  Resend only accepts the address the Resend account was opened with, which is enough to try it
  out and not enough for anybody else. The free tier is 100 mails a day and 3,000 a month.
  Everything that goes through mail waits on this: invitations, self sign-up (the verification
  link) and password resets. An administrator creating accounts by hand needs none of it.
  Details in `docs/MAIL.md`.
- A public address needs a playit.gg account per user. The panel provides none. On the free
  tier an account has four ports, so four servers can be reachable at once (sixteen with playit
  premium). Details in `docs/PLAYIT.md`.

## How it is built

Two processes, and one supervisor per running server.

```
craftpanel-helper (root, five commands)   craftpanel (unprivileged)
        │                                        │
        │ creates accounts, starts               │ web, database, API, files
        │ processes as craft-<id>                │
        ▼                                        ▼
   supervisor (craft-<id>) ◄── Unix socket ──► panel
        │
        └─► java -Xmx… -jar server.jar
```

The helper is the only part that needs root, and its whole vocabulary is five commands: create an
account, delete an account, set limits, hand back a subtree, start a process. It executes
**exactly one** binary: the supervisor. The Java command line travels as data and is run
unprivileged.

Every panel user gets a system account of their own. Their servers run under it, in a directory
other users can neither read nor list. The limits sit in a cgroup per user and
**throttle rather than kill**: `-Xmx` slows the garbage collector down, `cpu.max` hands out fewer
slices, and the hard ceiling is set high enough that it never fires in normal use.

### The supervisor

Between the panel and Java stands a supervisor, the same binary in a different mode. It owns the
pipes to the server and reports in to the panel. When the panel restarts, the supervisor connects
again and replays the console backlog. That is why running servers survive an update.

## The interface

It is **Modrinth's own UI library**, vendored under `vendor/modrinth/` and left alone there. Not
rebuilt — mounted. What was left out of the copy is their hosting surface (the purchase flow,
billing, plan selection) and their brand; everything that stayed, stayed as it was.

Modrinth's layouts talk to typed contracts you hand them, and the API call sits outside the layout.
We fill those contracts from our own interface. That this holds is Modrinth's own demonstration:
their Tauri desktop app fills the same contracts with local file system calls.

Modrinth's **trademarks** are removed and replaced with neutral shapes: the logo, the wordmark, the
Rinthbot mascot, the Modrinth Servers icon. The GPL covers their code and stops at the brand, which
trademark law governs. `scripts/check-no-branding.sh` fails the build if any of them comes back.
See [COPYING.md](COPYING.md).

## Build it yourself

```bash
pnpm install
pnpm --filter @craftpanel/web build   # must run before cargo: the interface is compiled in
cargo build --release

scripts/release.sh                 # both of the above, plus the bundle and its checksum
```

Checks, the same eight the CI runs:

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

Eight tests are `#[ignore]`d because they talk to the network (Modrinth, PaperMC, playit.gg,
Resend); run them with `cargo test -- --ignored` when you have changed something they cover.

More in [CONTRIBUTING.md](CONTRIBUTING.md).

## Documentation

`docs/` holds the design documents — what was decided, and why. They are the running record of
the project: `PLAN.md` (the whole shape), `api/CONTRACT.md` (the API contract), `DRIVE.md`,
`MAIL.md`, `PLAYIT.md`, `SIGN-UP.md`, `PASSWORD-RESET.md`, `INTERFACE.md` and `WIRING.md`
(interface and wiring), and `AUDIT.md` (an audit measured against a running build on 2026-08-13).

## Licence

**GPL-3.0-only.** The licence follows from the interface: it is built on Modrinth's UI library,
which is GPL-3.0-**only** — the "only" excludes the usual "or later" — and anything derived from
it inherits that. What was vendored, from where, and which parts of it are trademarks that may not
come back is in [COPYING.md](COPYING.md).

Not affiliated with or endorsed by Rinth, Inc. Nor with Mojang or Microsoft: the name points at
the game, and the Minecraft trademark is theirs.
