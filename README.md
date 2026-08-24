# CraftPanel

[![CI](https://github.com/MCbabel/craftpanel/actions/workflows/ci.yml/badge.svg)](https://github.com/MCbabel/craftpanel/actions/workflows/ci.yml)

A panel for running Minecraft servers on **one** machine. No daemon, no message queues, no
containers: one program, one file, one `curl` command.

## Install

```bash
curl -fsSL https://github.com/MCbabel/craftpanel/releases/latest/download/install.sh | sudo bash
```

The installer asks for the web port, the port range for game servers and an administrator name. It
creates the system accounts and the two systemd units, and prints the generated password **once**.

Run it again and it offers three things: update, uninstall, or nothing. An update leaves running
Minecraft servers running; see [The supervisor](#the-supervisor). An uninstall asks the running
servers to save and stop first, then counts what lies in the data directory — worlds, backups,
playit and Drive keys, fetched Java runtimes, system accounts — and asks separately whether all of
that goes with it.

Kept data is picked up again. Where an earlier uninstall left `/var/lib/craftpanel` and
`/etc/craftpanel/config.toml` standing, a later install takes them as they are: it asks nothing the
configuration already answers, makes no second administrator over a database that has one, and
names the accounts you sign in with. A password nobody remembers is set from the terminal with
`craftpanel admin passwd --username NAME --print-password`.

**Requirements:** Linux with systemd and cgroup v2. The installer checks both and says what is
missing. A Java runtime is no longer one of them: the panel fetches the one a game version needs —
Eclipse Temurin from [Adoptium](https://adoptium.net), a JRE, checksum verified — into
`/var/lib/craftpanel/runtimes/`. That covers Java 8 for 1.16 and older as much as Java 21 for the
current versions. A JRE that is already on the machine is used as it stands, and an administrator
can switch the fetching off in the panel settings; then the panel names the runtime a server is
missing instead of going and getting it. Details in `docs/JAVA.md`.

What the installer fetches is one bundle for this machine — `craftpanel-linux-x86_64.tar.gz` or
`craftpanel-linux-aarch64.tar.gz` from the newest release, checked against the `sha256` published
beside it. Both binaries in it are linked statically against musl, so one file runs on an old
Debian as on a current Ubuntu: the panel asks the machine it lands on for no glibc of its own age.

A bundle you built yourself is taken just as readily, which is the way in on a machine that cannot
reach GitHub, on an architecture no release covers, and when trying a change before it is tagged.
It is also the answer if the one-liner above prints
`curl: (22) The requested URL returned error: 404` and does nothing else: that address is a release
asset, and a `404` from it means this repository has published no release yet, so there is neither
an installer to fetch nor a bundle for it to fetch.

```bash
scripts/release.sh
sudo CRAFTPANEL_BUNDLE=dist/craftpanel-linux-x86_64.tar.gz ./install.sh
```

### Doing it deliberately

The one-liner pipes a script into a root shell, so here is everything needed to not have to take
that on trust.

**Read it first.** Nothing is lost by taking the two steps apart, and the file is worth keeping:
the same script updates and uninstalls later. Its sha256 is printed in the release notes, so the
script can be held against them the way the bundle is.

```bash
curl -fsSL -o install.sh https://github.com/MCbabel/craftpanel/releases/latest/download/install.sh
sha256sum install.sh
less install.sh
sudo bash install.sh
```

**A download that breaks off installs nothing.** The last line of `install.sh` is `main "$@"`, and
everything above it only defines functions and sets variables. A connection that dies in the middle
therefore leaves `bash` with definitions and no call, and it exits having touched nothing — which
is the one failure a pipe into a shell is otherwise genuinely bad at.

**The script is a release asset, not a branch.** `/releases/latest/download/install.sh` redirects
to the installer attached to the newest release, which is a file that went through the tests, was
built from a tagged commit and was signed in the same run as the bundles. The address itself never
goes stale — it names no version, so no release has to remember to correct this line — but what it
hands out is fixed per release. The old address ended in `HEAD`, the tip of the default branch at
the moment you fetched it: a push to that branch reached every installation started in the next
minute, as root, having passed nothing. That is the one gap this closes, and the price is that an
installer fix now needs a release of its own rather than a commit.

**To pin the script, name the release:**
`https://github.com/MCbabel/craftpanel/releases/download/v1.2.3/install.sh`. That is the address to
reach for when the newest release turns out to be bad and you want the one before it — and note
that it takes both halves, because the script asks GitHub for the newest release regardless of
where the script came from:

```bash
curl -fsSL -o install.sh https://github.com/MCbabel/craftpanel/releases/download/v1.2.3/install.sh
sudo CRAFTPANEL_VERSION=1.2.3 bash install.sh
```

A release asset is fixed the way a tag is fixed: nobody moves it by accident, and whoever holds
this repository could still replace it. The one address that no one can change the contents of is
a commit id — `https://raw.githubusercontent.com/MCbabel/craftpanel/<40 hex characters>/install.sh`
serves those exact bytes or nothing, because the name *is* the content. It is the strictest of the
three and it is still there; what is gone is the branch name that used to sit in that slot by
default.

**The bundle is checked, and the sum is printed.** The installer says
`checksum verified: sha256 <the number>`, so it can be held against what the release page shows. A
bundle whose `sha256` is not published is **not installed**: an empty answer for that one small
file used to be a warning you read afterwards, and it is a hard stop now. If the sum is genuinely
absent — your own bundle, copied without its `.sha256` — `CRAFTPANEL_NO_CHECKSUM=yes` says so out
loud, and there is no quiet way past it. What the sum is worth and what it is not is in
[SECURITY.md](SECURITY.md).

**Where a file came from can be checked too, and that is the part the sum cannot do.** Every
bundle *and the installer* is signed as the release is published, and the
[GitHub CLI](https://cli.github.com) says whether a file came out of a run of this repository's
release workflow:

```bash
gh attestation verify craftpanel-linux-x86_64.tar.gz \
  --repo MCbabel/craftpanel \
  --signer-workflow MCbabel/craftpanel/.github/workflows/release.yml
```

The same command with `install.sh` in place of the bundle answers the same question about the
script. The installer does not do this for itself, and could not honestly try — a fresh machine has
neither `gh` nor an account signed in, and a script that vouched for itself would be vouching with
whatever authority the script already had. So it is a step for whoever wants it, taken before the
install or long afterwards; the attestation is found by the digest of the file, not by its name or
its age.

Four variables are worth knowing. The installer reads a good many more (`CRAFTPANEL_PREFIX`,
`CRAFTPANEL_PORT`, `CRAFTPANEL_NONINTERACTIVE` and the rest of the unattended answers), but these
four decide *what* gets installed:

| | |
|---|---|
| `CRAFTPANEL_VERSION=1.2.3` | install exactly that release, instead of whatever is newest today. It pins the bundle, not the script — see [Doing it deliberately](#doing-it-deliberately) for pinning both |
| `CRAFTPANEL_REPO=owner/name` | fetch from another repository — a fork, or a mirror. Unset, it is `MCbabel/craftpanel` |
| `CRAFTPANEL_BUNDLE=<file>` | install a `.tar.gz` that is already on the machine; nothing is downloaded |
| `CRAFTPANEL_NO_CHECKSUM=yes` | install a bundle that has **no** published sum. Not a way past a sum that does not match — that always stops |

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

`cargo build --release` gives an ordinary binary for this machine, which is what tests and a local
run want. `scripts/release.sh` builds what a release carries instead: statically linked against
musl, so it does not ask the machine it lands on for a glibc of its own age. That target is fetched
once with `rustup target add x86_64-unknown-linux-musl` and `apt install musl-tools`, and
`CRAFTPANEL_TARGET` picks another one.

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
Resend, Adoptium); run them with `cargo test -- --ignored` when you have changed something they
cover.

More in [CONTRIBUTING.md](CONTRIBUTING.md).

## Documentation

`docs/` holds the design documents — what was decided, and why. They are the running record of
the project: `PLAN.md` (the whole shape), `api/CONTRACT.md` (the API contract), `DRIVE.md`,
`MAIL.md`, `PLAYIT.md`, `SIGN-UP.md`, `PASSWORD-RESET.md`, `JAVA.md` (the Java runtimes the panel
fetches), `INTERFACE.md` and `WIRING.md` (interface and wiring), and `AUDIT.md` (an audit measured
against a running build on 2026-08-13).

## Licence

**GPL-3.0-only.** The licence follows from the interface: it is built on Modrinth's UI library,
which is GPL-3.0-**only** — the "only" excludes the usual "or later" — and anything derived from
it inherits that. What was vendored, from where, and which parts of it are trademarks that may not
come back is in [COPYING.md](COPYING.md).

Not affiliated with or endorsed by Rinth, Inc. Nor with Mojang or Microsoft: the name points at
the game, and the Minecraft trademark is theirs.
