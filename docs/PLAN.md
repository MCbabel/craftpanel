# Plan

As of 2026-08-12. First version, written out of a survey of `modrinth/code`
(commit `2a43792f`, 11.08.2026).

## What gets built

A tool for setting up and running Minecraft servers on **one** machine.
One program, no daemon architecture. Install, update and uninstall through
one `curl` command. The interface is Modrinth's server interface.

Three decisions have been made:

| | |
|---|---|
| Scope | **Several servers** on the one machine |
| Backend | **Rust** — one binary, no runtime on the target machine |
| Access | **Accounts with roles**, like Modrinth's Access tab |

## The crux

Modrinth's UI library is split into two halves, and the difference decides
everything:

**`packages/ui/src/layouts/shared/`** — console, file manager, content, settings.
Around 21,000 lines, and **none of them talks to an API**. They talk to typed
contracts you hand them through `provide`:

```ts
interface FileManagerContext {
  items: Ref<FileItem[]>
  createItem: (name, type) => Promise<void>
  renameItem: (path, newName) => Promise<void>
  ...
}
```

**`packages/ui/src/layouts/wrapped/`** — around 7,800 lines that fill those contracts with
Modrinth's hosting. Everything we do not want sits in there too: billing, plan changes,
subdomains, node errors, the purchase flow.

### The proof that the seam holds

Modrinth's own **Tauri desktop app** (`apps/app-frontend`) is plain Vite and Vue, without
Nuxt, and uses the same library. For local instances it writes its own thin pages that fill
`provideFileManager`, `provideConsoleManager`, `provideContentManager`,
`provideInstallationSettings` with **local Tauri calls**, and then mounts Modrinth's
shared layouts. `pages/instance/files/index.vue` does exactly that.

That proves it: the layouts run outside Nuxt, outside the browser and **outside Modrinth's
API**. Counted: zero occurrences of `#app`, `#imports`, `useNuxtApp`, `useHead`,
`definePageMeta`, `useRuntimeConfig`, `useFetch` in `packages/ui`. The 16 `useRoute` and 15
`useRouter` are vue-router and run anywhere; all seven `navigateTo` are local functions, not
Nuxt's.

### What follows from that

**We serve the contracts, not the HTTP interface.**

The obvious route would have been to rebuild Modrinth's Archon and Kyros endpoints so their
`wrapped/` pages run unchanged. That would be the worse route: we would have to guess an
undocumented, foreign API byte for byte — error codes, pagination and authentication
included — and would get the billing logic thrown in on top, which we would then have to cut
back out.

Instead: **our own, clean REST interface**, and thin pages that pour its answers into the
contracts. Exactly the way the desktop app demonstrates.

| Part | Origin |
|---|---|
| Console, file manager, content, settings | **Modrinth's `shared/` layouts, unchanged** |
| Server cards, labels, state display, action buttons | **Modrinth's `components/servers/`, unchanged** |
| Base components, design variables, icons | **Modrinth's `components/base/`, `packages/assets`, unchanged** |
| The contract fillers (providers) | our own work, TypeScript, ~200–400 lines per area |
| Server list and server frame | our own work, from Modrinth's building blocks |
| Everything under `wrapped/` | **is not taken over** |

Mod and modpack search still goes to the **real** `api.modrinth.com`. For that,
`packages/api-client` (LGPL) is used unchanged, only with `labrinthBaseUrl` on the original.

## What goes in and what does not

Modrinth Servers is a paid product. Part of the interface exists only because of that.

**Goes in:** server list with search · server header with start/stop/restart/kill ·
metrics (CPU, RAM, disk) with history · console with filter levels, mclo.gs sharing and
crash analysis · content: install, enable, disable, delete, update, one at a time and in
bulk, straight from Modrinth · modpacks: install, update, unlink, repair, change the game
version · files: everything including editor, unpacking, undo · backups · accounts with
three roles and an audit log · settings: `server.properties`, Java runtime, startup command,
ports, reinstall.

**Does not go in:** billing, plans, renewals · availability and regions · migration between
nodes · buying a subdomain and DNS · the "support staff" role · friends list and
attribution.

**Decided against: SFTP.** On your own machine there is shell access anyway, and the file
manager covers the rest. The credentials drop out of the "Advanced" page along with it.

## Technology

**Backend.** Rust with `axum` and `tokio`. State in **SQLite** through `sqlx`: embedded, no
database to set up. The built interface is compiled in with `rust-embed`, so that one file
really does come out. Console and metrics over WebSocket.

**Every writing transaction starts with `BEGIN IMMEDIATE`.** Almost every write reads first —
which server, which row — and writes afterwards. Two such transactions cannot both carry on:
SQLite rejects the second one with `database is locked` the moment it goes from reading to
writing, and **that one** no is not retried by any wait: the `busy_timeout` of 10 seconds does
not apply there, the refusal arrives within a hundredth of a millisecond. Take the write lock
right at the start and the second caller waits its turn instead. Without that the panel answers
`500` where the contract calls for a queue (`crates/craftpanel/src/ops/store.rs:157-164`).

That has a consequence for the tests, and it matters more than it sounds: an in-memory database
lives only as long as the connection that opened it, so the test pool holds **one** connection, and
one connection lines every statement up behind the last and hides exactly what two writers do
to each other. A test about concurrent writers therefore needs a file on disk and several
connections; otherwise the yardstick comes out of the thing being measured
(`crates/craftpanel/src/ops/testing.rs:31-48`).

**Running servers.** Every Minecraft server is a process of its own with a directory of its
own, whose output is read along and whose input is served. **No Docker**: that would be a
dependency that contradicts the promise of "one `curl` command". The isolation comes from
system accounts and cgroups instead; see the next section.

The **version sources** have a section of their own further down.

**Frontend.** Vue 3. `packages/ui` and `packages/assets` are **vendored**, not installed: they
are not on npm (`"private": true`). So as a subdirectory or submodule in our own repo,
with a build of its own.

**Installer.** A shell script behind a URL. On the first run: asks for the directory, the
port and the first account, downloads the matching binary, creates a systemd unit, starts it.
On every further run: recognizes the installation and offers **update** (compare versions,
swap the file, restart the service) or **uninstall** (service gone, program gone, data on
request).

## License

`@modrinth/ui` is **GPL-3.0-only**, not "or later". Whoever takes the code puts their own
tool under GPL-3.0. That fits "fully open source", but rules out AGPL and GPL-2.
`packages/api-client` is LGPL-3.0.

**The trademark is excluded.** The logo, `components/brand/*` and `ModrinthServersIcon.vue`
are "All rights reserved. © 2020-2025 Rinth, Inc." (`vendor/modrinth/ui/COPYING.md`). They have to
be replaced with our own before any release. That is not a formality but trademark law. It holds regardless of the rest of the
code being free.

## Privileges and isolation

Principle: **only the bare minimum runs as root, and a Minecraft server never does.**

### One system account per panel user

When the administrator creates the panel user `max`, the system account `craft-<id>` comes
into being, not named after the name, because names change and collide. **All** servers
`max` creates run under this account.

That draws the boundary between *users*, not between individual servers. Within one user,
their servers still share everything. That is deliberate and matches who you trust.

**Whether an account is "ready" is decided by the account alone, not by its limits.** Once the
helper has created the system account, the account counts as ready, even if writing the cgroup
limits fails afterwards; the error is kept as text on the row. A machine without cgroup
delegation — usually a container — can run servers, and declaring the account broken over it
would stop everything, for a reason its owner cannot change
(`crates/craftpanel/src/auth/users.rs:530-533`).

Two states you have to keep apart: `provisioning` means "the helper has not had its turn yet",
`error` means "it had its turn and refused". Only the first is caught up on at every panel start.
`error` stays put and waits for 12.9: a helper that refuses permanently should not be asked
again at every boot (`auth/users.rs:566-569`).

### Directories and groups

```
/var/lib/<panel>/         root:craftpanel        1771   ← sticky
├── panel.db              craftpanel:craftpanel  0600
├── config.toml           craftpanel:craftpanel  0600
└── users/                root:craftpanel        0751
    └── <user-id>/        craft-<id>:craftpanel  2770   ← setgid
        └── servers/
            └── <server-id>/
```

The owner is the system account, the group is **a single** group `craftpanel` that holds only
the panel service. Everything follows from that:

- `craft-<id>` reaches its own directory as its **owner**.
- `craft-<id>` does **not** reach other users' directories: there it is neither owner nor in
  the group, and nothing is set for "others".
- The panel reaches all of them through the group. That is how the file manager works
  without root.
- No server process reaches `panel.db` with the password hashes.

The two **upper** levels belong to root, and the helper's whole promise rests on that.
Everything below `users/<id>` belongs to the managed account (2770). It may rename in there at
any time. If `users/` belonged to the panel, a compromised panel could do the same one level up:
move an account aside, put a link in its place and let root's `chown-tree` run wherever it
points. Root as the owner of `users/` nails those names down.

`/var/lib/<panel>` itself cannot be closed off the same way: `panel.db` lies in it, the panel
has to write there. That is what the **sticky bit** is for: whoever may write in a directory may
still only rename and delete what belongs to them, and `users/` belongs to root. The panel keeps
the database, the cache and the backups; it does not keep the ability to move the accounts aside.
The helper also opens `users/` **once at startup** and works on that descriptor for the rest of
its runtime. A descriptor follows the inode and not the name. It puts owner and permissions
right while doing so, in case an older installation had set them differently, and writes a
warning to the log if the level above could still move them.

The descriptor only covers the running session, though. If somebody took the name `users/`, only
the **next** start of the helper would open whatever stands in that place by then. That is what
the sticky bit is for, and that is why the warning at startup is not a courtesy
(`crates/craftpanel-helper/src/main.rs:81-96`, `say_if_the_root_can_be_moved`). The helper takes
the first grip with `O_NOFOLLOW` (`crates/craftpanel-helper/src/beneath.rs:114-118`,
`Root::open`): if a link were already lying there because the panel was faster, it gets a no
instead of a root of the panel's choosing. The `1` in `0751` is no decoration either: owner
root, so nobody else can bind a name in it, and traversable by everyone, so a managed account
reaches its own directory (`beneath.rs:40-43`, `ROOT_MODE`). And because that one name belongs to
root, `O_NOFOLLOW` is the whole promise for the step into an account directory: nobody but root
can turn it into a link, and were it one after all, the kernel answers `ELOOP` instead of walking
through (`beneath.rs:128-134`, `Root::home`).

Two details that would bite later otherwise: the **setgid bit** on the directories makes
newly created files inherit the group `craftpanel`, and the server processes get
**`umask 007`** so the group may write too. And the reason for *one* fixed group instead of
one per user: group membership is fixed when a process starts. A running service would only
see a new group after a restart.

### How the privileges are split

Two processes instead of one:

**`<panel>`** — the actual program. Runs as `craftpanel`, **unprivileged**. Web interface,
database, API, file manager, Modrinth calls. Never becomes root.

**`<panel>-helper`** — tiny, runs as root, is started by systemd and speaks a **fixed, short
command vocabulary** over a Unix socket:

| Command | does |
|---|---|
| `create-user <id>` | create the system account, create the directory, set owner and permissions |
| `delete-user <id>` | the same in reverse |
| `apply-limits <id>` | write the memory, CPU and process limits into the cgroup |
| `spawn <id> <steps> <argv>` | switch user, enter the cgroup, start the supervisor |
| `chown-tree <id> <steps>` | reset owner **and** permissions of a subtree: hand back to the account what the panel wrote, and open up to the panel what the game wrote |

**No command takes a path.** Where a directory is meant, the request names the account and the
**steps** from that account's own directory to it: `["servers", "<server-id>", …]`. The root
comes from the helper's configuration and never from the request. The reason is measured: a path
would have to be *proven* to be "inside the account", and a name proven once stays true only
until the account renames a **middle** segment. `O_NOFOLLOW` protects the last segment and none
before it. The helper therefore walks the steps with
`openat2(RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS)` from the descriptor of the account directory
(`crates/craftpanel-helper/src/beneath.rs`, built like `Root::open_beneath` in the panel), and
what happens after that happens on the **descriptor**, not on the name a second time. `spawn`
too enters its working directory with `fchdir` (`main.rs:329-334`) and not with a second
resolution.

Three words carry that, and there are no more. A **root** (`Root`) is `users/`, opened once at
startup. A **home** (`Home`) is an account's own directory, reached from the root. A **held**
thing (`Held`) is what a request pointed at: a descriptor, not a name. All three
carry a path with them, and it stands in error messages only; nothing is ever
opened with it (`beneath.rs:84`, `users.rs:236`). A **step**, in turn, is exactly one name: not
empty, not `.`, not `..`, at most 255 characters, without `/` and without a null byte
(`crates/craftpanel-proto/src/lib.rs:106-113`, `is_valid_step`). Anything else would be a way of
naming something the account does not own. And user ids are ULIDs and nothing else
(`lib.rs:90-92`); that is exactly what keeps them out of shell words, file paths and account
names.

`RESOLVE_BENEATH` and not `RESOLVE_IN_ROOT`, and the account directory as the anchor instead of
the server directory: everything below it belongs to the account anyway, so a link that stays
underneath leads nowhere it was not already allowed to go. `RESOLVE_NO_MAGICLINKS` also shuts
`/proc/self/fd` as a way out (`beneath.rs:33-38`). Kernels before 5.6 do not know `openat2`;
there the helper does one `openat` with `O_NOFOLLOW` per step (`beneath.rs:221-228`, `step_in`).
This second route is **stricter** — it rejects every link, even one that stays in the tree — and
must never get looser; that it is being taken, the helper says once in the log
(`beneath.rs:254-258`). `create-user` too works through descriptors from its first line and never
through names: an account that already exists owns its home and could put a link where `servers`
stood. A `chown` on that name would follow it, and root would give away a stranger's directory
(`users.rs:119-137`).

Panel and helper tell each other a **protocol version** when they connect, today 2: the two
commands that used to name a directory now name steps inside an account. A version 1 helper and a
panel of this one do not understand each other, and the line at startup is the place where that
shows (`crates/craftpanel-proto/src/lib.rs:8`).

`spawn` starts **only** the supervisor and nothing else (`main.rs:274`). The game's command line
travels along as data and is only executed there, unprivileged; that way even a compromised panel
can never get this root process to exec something of its choosing. The helper waits on the
started supervisor in a thread of its own (`main.rs:357-359`). If it did not, every server ever
started would leave a dead entry in the process table until the helper restarts.

The last command closes a gap that was overlooked during design and only turned up when the
interface contract was reviewed. It is measured and it was fatal: the panel writes as
`craftpanel`, so a freshly installed mod belongs to it, and the game process, running as
`craft-<id>`, is neither its owner nor in the group `craftpanel`. It gets
`Permission denied` on its own mod.

Considered and rejected: a shared group for panel and game process breaks the isolation
between two users, because then B may go into A's directory too. Adding group membership
later fails because it is fixed when the process starts. Default ACLs would solve it, but
they require the `acl` package, a dependency that contradicts the `curl` promise.

So: after every write the panel makes into a server directory — upload, install a mod, write
a configuration, unpack an archive — **one** call follows that resets the subtree to
`craft-<id>:craftpanel` with `2770`/`0660`. One call per operation, not per file. Symlinks
are left untouched; following them would mean serving a pointer out of the tree.

Only what the panel could have written at all is touched: directories and **regular** files. A
socket, a named pipe or a device file in a server directory comes from the game and already
belongs to it; there is nothing to hand back there
(`crates/craftpanel-helper/src/users.rs:373-397`, `hand_back_file`). Every entry is therefore
opened with `O_NONBLOCK` — were there a pipe without a writer, the helper would hang on it, and
it answers one request after another — and with `O_NOCTTY`, so it does not catch a terminal while
tidying up (`users.rs:413-417`, `beneath.rs:47-52`). A socket gives out no descriptor at all and
answers `ENXIO`; that is not an error but "there is nothing here for you"
(`users.rs:378-380`).

Whatever **disappears** under the walk is skipped: a tree under a running server may move, and
`chown-tree` walks right into it. A backup calls it on a tree the server is writing into. So
`ENOENT` and `ENOTDIR` mean "the tree has moved" here (`users.rs:430-432`). Every **other** answer
from the kernel ends the walk and lets the request fail: the helper is root, it gets into a
directory locked by the game anyway, and a refusal then means something else.

The walk is a **stack and not a recursion**: a few hundred levels of nesting is a tree and no
reason for the helper to die on its own stack (`users.rs:203`). At **256** levels it breaks off
and names the path where it stood still: one descriptor stays open per level, and without that
ceiling the helper would run out of descriptors somewhere in the middle of the tree, and that
would be a message nobody can do anything with (`users.rs:28`, `MAX_DEPTH`). Listing is done with
`getdents64` straight from the held descriptor, because `std` offers no way to read an already
open directory and `readdir` would want to give the descriptor away to a `DIR*`; the buffer for it
is a `Vec<u64>` and not a `Vec<u8>`, because `getdents64` promises nothing about the alignment of
a byte buffer (`users.rs:506-509`).

**And files with a second name likewise.** A hard link is not a symlink: there is nothing to
follow, `O_NOFOLLOW` and `AT_SYMLINK_NOFOLLOW` have nothing to refuse, the name in the tree **is**
the file. Anyone who may place `ln /etc/shadow <tree>/loot` and then triggers any write gets
`/etc/shadow` moved from root to `craft-<id>:craftpanel` with `0660`. That was measured, not
assumed. The entry itself does not say where its other name lies; it only says that there is one.
So: `chown-tree` does not touch a regular file with `st_nlink > 1`. In the tree it is skipped and
the helper **says so** in its log: the count and the first **eight** paths, because how many
there are is up to the account, and a journal is not a place a stranger may fill
(`users.rs:256-268`). Rejecting the whole operation, on the other hand, would permanently kill
every file action of the server for a mod
that hard-links within its own tree. If the individually named file is itself the one with the
second name, the call is **rejected**: the caller named exactly that file, and a success that did
nothing would be a lie. Counted: in the operator's real tree, 0 of 311 files with more than one
name; the cost of the descriptor per file is 93 instead of 67 ms over 20,000 entries.

The host has the same rule one level down: `fs.protected_hardlinks=1` (the default everywhere)
forbids an account a second name for a file it does not own and may not read and write.
Nothing has rested on that since this round, but the helper reads the setting and warns once
if it is off.

And the same call **before** reading or deleting a whole tree, for the opposite reason: what the
game process creates belongs to it, and it is more generous with itself than with us: Java
writes `level.dat` with `0600`, WorldEdit unpacks its languages as `drwx--S---`. The panel does
not get into a directory like that, so it can neither back it up nor delete it. Because
`chown-tree` resets not only the owner but the permissions too, and because it runs as root, it
is the one command for both directions. A **sixth** command that deletes a tree would be the most
dangerous kind of root power for a problem the fifth already solves.

Nothing else. No free paths, no free commands: ids are checked, `argv` comes out of the loader
definition, and the helper **refuses every uid it did not create itself**, the 0 above all. That
way even a compromised panel can only do what it is allowed to do anyway: run as a managed user.

### Why a supervisor stands in between

The obvious route would be for the panel to hold the server process as a child of its own and
serve its pipes directly. That fails on a requirement stated further up the plan: **the
update through one `curl` command.** A child process dies with its parent or becomes an
orphan whose input and output nobody can reach any more. So a panel update would either shoot
down every running server or lose their consoles forever.

That is why a **supervisor** stands in between for each server, the same binary in a
different mode, started by the helper as `craft-<id>` in the user's cgroup:

```
Panel (craftpanel)  ←── Unix socket ──→  supervisor (craft-<id>)  ──→  Java
```

The supervisor owns the pipes to the server and reports in to the panel. When the panel
restarts, **it connects again**: the Minecraft servers keep running without interruption,
the console simply attaches again. That turns an update into an operation you can do while
everything is running.

The supervisor identifies itself with a one-time key that the helper generates and tells both
sides. Otherwise any process of the same user could pose as the supervisor of somebody else's
server.

**That is why the socket they report in on is `0666`.** A supervisor runs as `craft-<id>` and
shares no group with the panel; the socket is reachable for it only if it is reachable for
everyone. What keeps strangers out is the key and not the file mode, and the directory above it
is traversable but not listable, so the path has to be known already
(`crates/craftpanel/src/servers/hub.rs:111-117`).

A server's slot in the hub table is cleared at the end of a connection **only if it is still the
same one**. A second supervisor for the same server takes the slot as soon as it arrives; if the
older one ends after that, a blind removal would have thrown the live one out and the panel would
have declared a running server crashed (`hub.rs:189-201`).

After a panel start an order applies that was once the wrong way round. First the hub gets the
keys back from the database: without them no surviving supervisor can identify itself. Only
**six seconds** later is the reckoning done over who is still there: supervisors report in every
two seconds, and any shorter deadline declares one dead that is knocking right now
(`crates/craftpanel/src/servers/manager.rs:47-50`, `:1414-1430`). Settle up immediately instead
and you declare every running server stopped, throwing away exactly the key its supervisor would
have identified itself with (`crates/craftpanel/src/main.rs:189-193`).

The hub has **no hook for "a supervisor is there"**. Whoever carries the lines on to the sockets
therefore looks every 50 ms to see whether one is new, and because the hub keeps nothing for a
listener that does not exist yet, that interval is at the same time the width of the window in
which the very first output of a starting server is lost. That is named and accepted, not
overlooked (`crates/craftpanel/src/ops/follow.rs:22-25`).

And because seconds can pass between "the helper was asked for a supervisor" and "the supervisor
reports in", a **power request out of that gap is kept** rather than dropped: 4.6 allows `stop` on
a starting server, and a starting server is precisely the one whose supervisor is still on its
way. The guard passes the request on as soon as somebody is there, and only then does it count as
delivered. With `kill`, dropping it would be the worst of the four cases: the process would keep
running while the panel holds it for dead
(`crates/craftpanel/src/servers/manager.rs:229-240`, `:1349-1368`).

Along the way that spares us handing file descriptors over through `SCM_RIGHTS`: the
supervisor holds the pipes itself and only passes lines on over the socket.

### Resource limits per user

The games' groups sit in a tree of their own, **beside** the two services and inside neither: when
systemd stops a service it takes the whole control group with it, and a tree under
`craftpanel.service` would tear down every running server at every panel update
(`crates/craftpanel-helper/src/main.rs:22-25`). They are created and written by the helper, which
is root anyway; the panel only reads the same tree to show what an account is currently using, and
that is why both units name it. (The panel unit still carries `Delegate=yes`, but the games'
groups no longer hang under it. The plan originally had it that way, and the reason above changed
it.)

```
/sys/fs/cgroup/system.slice/craftpanel-games/
└── user-<id>/     memory.max · memory.high · cpu.max · pids.max
    └── all servers of this user
```

Because all of a user's servers sit in the same cgroup, the limit applies to their **sum**, exactly
what "limit per user" is meant to mean.

The three controllers (`cpu`, `memory`, `pids`) have to be **delegated** to the subtree, and the
helper adds them on every call instead of only at creation: a tree that already exists is exactly
the case in which they can be missing: one somebody else made, or one whose delegation failed
once. Never fatal: the kernel refuses `cgroup.subtree_control` for a group that holds processes of
its own, which is why a root pointing at a systemd service can never be delegated at all. That is
worth a line in the journal and not a server that does not start
(`crates/craftpanel-helper/src/cgroup.rs:29-48`). If the controllers are missing anyway, there are
no files to write, all four limits are skipped silently, and the panel is told they are set. That
silence is exactly the difference between a limit and a number in a table, so the helper warns
when there is no controller file in a group (`cgroup.rs:78-83`). A field that arrives empty is
**written** as `max` and not left out: for an account with no limits at all, a panel
administrator, all four files end up carrying `max`; leave them out and the previous number would
stay (`cgroup.rs:75-84`).

So that a limit really catches the process, the **supervisor itself** enters the group, in the
breath before its `exec`. For that the helper opens the group's `cgroup.procs` file as root and
hands the descriptor on to the child; the kernel judges a write to it by the rights of whoever
**opened** the file, not of whoever writes (`crates/craftpanel-helper/src/cgroup.rs:56-62`,
`open_roll`). A `0` there means "whoever is writing right now". That is the only moment at which
the game and everything it later forks are certainly inside: the supervisor forks the game as its
first act, and a child that a later sorting-in would leave behind would run without its owner's
memory, CPU and process ceilings and would die with the panel instead of surviving it
(`main.rs:334-345`).

**Principle: throttled, not killed.** Whoever hits their limit gets a slower server, not a
shot-down one. For CPU and for RAM, though, that is technically something different.

**CPU — exactly the way you picture it.** `cpu.max` hands out compute time. Whoever exhausts
their share gets fewer cycles: the tick rate drops, the game stutters, the server keeps
running. Nothing dies, it gets slower. That is the normal case on any oversubscribed server
and needs no special handling.

**RAM — the throttling sits in the JVM, not in the kernel.** Memory cannot be portioned out
like compute time: a page a process needs is either there or not. So the actual tool here is
**`-Xmx`** per server. When the heap gets tight, the garbage collector clears up more often:
the server gets slower and the tick rate drops, but it runs. Exactly the wanted behavior, and
the JVM can do it better than the kernel, because it knows what it may clear away.

The cgroup only catches what lies beside it (off-heap, native threads, metaspace):

| Knob | Value | Effect |
|---|---|---|
| `-Xmx` per server | out of the user budget | **the actual limit** — the GC slows down, nothing dies |
| `memory.high` | the user limit | throttles allocation, **does not kill** |
| `memory.max` | well above it | **emergency brake**, should never fire |
| `cpu.max` | the user's share | throttles, does not kill |
| `pids.max` | generous | against fork bombs |

The emergency brake stays in, set high enough that it does not fire in normal use. Without it
a process gone haywire could grow until the **system-wide** OOM killer strikes, and that one
picks its victim from the whole machine, possibly another user's server or the panel itself.
So the emergency brake is not there to punish anybody, but to keep the damage with whoever
causes it.

If the `-Xmx` of several servers add up past the user limit, that is allowed, but a warning
belongs there, because then the emergency brake comes into range.

One more choice open to the administrator: `cpu.max` is a **hard ceiling**: two cores stay two
cores, even when the machine is bored. `cpu.weight` would be a **share** instead: when it is idle
you may take more, under contention you get cut back. For a tool among friends the share is often
friendlier, for bounded promises the ceiling. The interface offers both. From share mode, though,
only half the promise reaches the kernel today: `cpu.max` is dropped, but `cpu.weight` is not
written, and under contention the accounts split evenly instead of by their cores. That is named
and open: `docs/api/CONTRACT.md` 17.16.

What the machine keeps for itself before anything is handed out at all: **two gibibytes, on a
small machine a quarter**, whichever is less. The kernel, the page cache and the panel itself
need room, and on a 4 GiB box the fixed number would have given away half the memory twice over
(`crates/craftpanel/src/auth/usage.rs:156-161`).

Usage is read out of three files per user — `memory.current`, `cpu.stat`, `pids.current` — and
**the CPU number is a difference between two visits**. That is why there is exactly one reader for
the whole process: a freshly created one per request would have no first visit and would answer
`0.0` forever (`crates/craftpanel/src/auth/usage.rs:41-43`). And because a cgroup does not survive
a reboot of the machine — and certainly does not notice that the panel has changed its mind about
who gets a ceiling at all — every start writes the limits of every ready account once more. In the
normal case those are the same four numbers as before; what it puts straight is the drift: an
account that was promoted while the panel was down, or one whose ceilings still come from a
version that gave administrators a budget (`crates/craftpanel/src/auth/users.rs:593-598`).

**The fifth limit has no kernel file.** For disk space there is nothing in cgroup v2.
`io.max` is throughput, not room. So the panel holds it itself, at the doors that lead through
the panel (the list is in `docs/api/CONTRACT.md` 12.7). It is measured with a walk over the
account's server directories, and because that is expensive there is **one** meter for the whole
process with a window of 60 seconds: long enough that a page with fifty accounts does not walk
fifty trees, short enough that an upload of a few hundred megabytes shows up before the next one
(`crates/craftpanel/src/auth/disk.rs:29-31`). It is deliberately **not** a `OnceLock`: a test that
cannot hand its own number in would have to build the tree it measures, and then the yardstick
would come out of the thing being measured (`disk.rs:10-14`).

A directory the game process closed off from the panel is not accepted but **opened up** with
`chown-tree` and then counted a second time; if it stays locked even for root, the number is a
lower bound and nothing new is written any more. A limit you get under with `chmod 0700` would be
none. **Considered and rejected: a sixth helper command that measures.** That would be new root
vocabulary for something the fifth can already do, the same reasoning as above with deletion
(`disk.rs:176-191`, `:275-287`). And backups only count as long as they lie here: one that went
into the user's Drive keeps its `size_bytes`, because the interface shows the size, but occupies
not a byte of this disk (`disk.rs:227-237`, `CONTRACT.md` 22.18).

The smallest disk limit accepted at all is **one gibibyte**, roughly what a bare Vanilla world
plus a loader costs; below that it would be a limit in which nobody can run a server
(`crates/craftpanel/src/auth/limits.rs:29-31`).

### What still needs root — and what does not

**Root:** create and delete system accounts · set the owner of new directories · switch user
before the start. That is the complete list.

**No root:** cgroups (through `Delegate=yes`) · file access (through the group) · ports. Minecraft
uses 25565 upwards. For the web interface on 80/443 either a reverse proxy in front
or `CAP_NET_BIND_SERVICE` on the binary. Neither is root.

### What that is proven with

The helper's promises have been put to the test, not hoped for, and in such a way that the
yardstick itself is measured.

**Probe 7 of the contract review**, set up by hand instead of run on luck, and twice over the
same tree: once on the route the helper used to take, and once on today's
(`crates/craftpanel-helper/src/main.rs:464-522`). The swap happens at the same place both times:
at a **middle** segment, after the target was already fixed, so exactly what an account may do
with its own directories at any time. The first half **must** lose the file outside the tree; if
it stopped losing it, the second half would prove nothing. That is why the old route —
canonicalize, pass the name on, resolve a second time — is kept in the test and must not be
cleared away as dead code (`main.rs:489-514`). Instead of `RENAME_EXCHANGE` as in the probe, the
test swaps with a rename and a symlink; either way the name is never missing.

**And the same in a race.** One thread walks the tree, four keep swapping names in the tree for
links out of it, with `RENAME_EXCHANGE`, so the name is never missing and the walk sees nothing
but the swap (`crates/craftpanel-helper/src/users.rs:803-894`). Whether a swap falls into the
window of a particular entry is up to the scheduler, so the test asserts the property and not the
hit: nothing outside may move, and the walk must not fall over either. Why there are eight passes
and not one is measured: **one** pass caught the old walk, which resolved names twice, in two out
of three attempts, **eight** caught it in twelve out of twelve (`users.rs:871-882`).

Two small things somebody who touches these tests later would otherwise get stuck on. Where
`O_NOFOLLOW` and `O_DIRECTORY` stand together, the kernel answers `ENOTDIR` where either alone
would have said `ELOOP`; both mean the link was not walked
(`crates/craftpanel-helper/src/beneath.rs:372-381`). And tests that need root say so and skip
themselves; where the rights are missing, the mode carries the claim alone, because a `chown` to a
foreign group is root's business and a `chown` to your own is anybody's (`users.rs:437-446`).

### What that achieves — and what it does not

**Done:** a plugin of user A does not reach the files of user B. No server process reads the
panel database. A server gone haywire does not starve the others. The panel itself is no
longer a worthwhile target, because it owns nothing elevated.

**Remains:** a plugin is arbitrary Java code and does not go through our API: our **roles
bound the interface, not the JVM**. Whoever may install content on somebody else's server has
code execution *as that server's system account*. Sharing a server with somebody therefore
stays a decision about trust. All that is new — and this is the gain — is that the damage ends
at the owner's boundary.

### A new surface

Modrinth has **no** interface for resource limits; there they come out of the plan you
bought. So the user administration with memory, CPU and process limits is our own work, built
from Modrinth's building blocks, modeled on `user-profile/layout.vue` and
`edit-user-modal.vue`.

## Who may do what, and how a server comes about

### Two levels that must not be confused

**Panel role** — `Admin` or ordinary user. Applies to the whole panel.
**Server role** — owner, editor, viewer. Applies per server; that is Modrinth's model.

The two are independent. An ordinary user is the owner of their own servers and can be an
editor on somebody else's server at the same time.

### The budget is the permission

An ordinary user may create as many servers as they like, **bounded by their budget alone**.
The administrator hands out 8 GiB and 4 cores; how the user divides that up is their business.
There is no need for a second counter capping the number.

The check is against the **allotted** sum, not against momentary usage: the sum of the `-Xmx`
of all their servers plus the new one has to fit into the budget. That is predictable: a
server does not start today and fail tomorrow because another one happens to be busy.

### The flow for an ordinary user

Modrinth's `onboarding.vue` is exactly this flow; the purchase step before it drops out with
nothing in its place.

1. **New server** in the server list
2. **Name**
3. **What do you want to play** — loader and version, or upload a modpack *(Modrinth's
   step, unchanged)*
4. **Memory** — a new step. The slider ends where the budget ends.
5. **Port** — the panel hands it out itself, out of the range the administrator set
6. **EULA** — Mojang's license has to be accepted, or no server starts. A tick here
   spares you the most puzzling of all error messages later.
7. The panel creates the directory under their system account, downloads the jar, writes
   `eula.txt` and `server.properties`, sets `-Xmx` → *"Setup server (~2 min)"*

What they **cannot** do along the way: choose a different owner, take a port outside the
range, exceed their budget or change their own limits.

`-Xmx` stays a field the panel manages. They may set their own Java flags beside it: that is
no extra power, because as their own system account they can run arbitrary code anyway. Only
`-Xmx` itself stays out, otherwise the throttling would be void.

### The flow for an administrator

The same flow, three fields more:

| | ordinary user | administrator |
|---|---|---|
| Owner | always themselves | **freely chosen** — decides the system account the server runs under |
| Port | automatic, out of the range | free, outside it too |
| Memory | up to the budget | up to the machine, with a warning |

Only the administrator can, on top of that: create and delete panel users (which creates and
removes the system account with them), set the limits per user, define the port range and see
every server of every user.

The installer creates the first administrator, through `craftpanel admin create`, **before**
either of the two services runs (`install.sh:1336`). Three things follow from that, which together
are a small interface:

* **Standard output carries the password and nothing else.** The installer catches it and
  shows it to whoever is installing; everything a human is meant to read goes to standard
  error (`crates/craftpanel/src/auth/cli.rs:1-6`).
* **The invented password is twenty characters out of an alphabet without `i`, `l`, `o` and `u`**:
  it gets copied off a terminal, and those are the four you mix up while doing it. 32
  divides 256, so a byte modulo the alphabet length favors no letter
  (`auth/cli.rs:371-378`).
* **The helper is not running at that point.** The account therefore stays on
  `provisioning` instead of `error` — otherwise it would wait for an administrator who cannot
  sign in yet — and the first panel start catches the system account up. systemd, by the way,
  reports the helper as active as soon as its process has started, a while before it listens;
  the panel therefore waits briefly for an answer instead of writing off every waiting account
  at the first boot after an installation (`auth/cli.rs:354-357`,
  `crates/craftpanel/src/main.rs:195-206`).

### Two cases you have to decide up front

**The administrator lowers a limit below what is already allotted.** Running servers are
**not** shot down. That would contradict the principle. The user counts as over: they can
create no new servers and start no stopped ones until they are back under their limit. What
runs keeps running.

**A panel user who owns servers is deleted.** Not silently deleted along with them. The panel
demands a decision: transfer to another user or delete them explicitly too. Only after that
does the system account disappear.

## Loaders and their sources

All endpoints checked on 2026-08-12, all public and without a sign-in.

### First wave — no installation step

Each of them is a single download. Four sources cover **seven** loaders.

| Loader | Source | Note |
|---|---|---|
| Vanilla | Mojang `version_manifest_v2.json` | |
| Paper | PaperMC `fill.papermc.io/v3` | |
| **Folia** | PaperMC v3, project `folia` | Paper with region threading; **the same code**, only a different project name. Fewer versions than Paper (from 1.19.4 on). |
| Purpur | `api.purpurmc.org/v2` | |
| **Leaf** | `api.leafmc.one/v2` | A Paper fork aimed at performance. **Its own API in the old Paper v2 format** — needs a small adapter of its own, not the v3 path. Delivers `sha256` and knows the channels *stable* and *experimental*; the default is stable. |
| Fabric | `meta.fabricmc.net` | Delivers a **ready-made server jar** (checked: 168 KB, starts directly). That makes modpacks possible from the first wave on. |
| **Velocity** | PaperMC v3, project `velocity` | A proxy — the download is free, the work sits in the interface. See below. |

Two variants of the PaperMC v3 source (Paper, Folia, Velocity) and Leaf's v2 variant share
almost everything: pick a version, pick a build, pull the file, compare the checksum.

### Second wave — with an installation step

| Loader | Route |
|---|---|
| NeoForge | Installer jar from `maven.neoforged.net`, then `--installServer`; produces a start script and an argument file, the startup command differs |
| Quilt | Installer jar from `meta.quiltmc.org` |
| Forge | Like NeoForge, but the route differs between before and from 1.17 on. The most unpleasant one — and the one older modpacks need. |

### Deliberately not

**Spigot and CraftBukkit.** They may not be redistributed; they would have to be built from
source with BuildTools on the user's machine: minutes of compute time, needs Git and a JDK.
Paper is the successor and the better choice in every respect. That is the only gap we leave
on purpose.

**Hybrids** (Mohist, Arclight, Magma, Ketting, CatServer) — Forge mods and Bukkit plugins
together. Shaky in practice and behind on versions. Modrinth does not list them.

**Bedrock** (BDS, PocketMine, Nukkit) — a different protocol, a different mod world. That
would be a second product. Anybody who wants Bedrock players installs **Geyser** as a plugin
on Paper; that runs through the normal Content tab like anything else.

### Velocity is selectable, nothing more

Velocity is an entry in the loader dropdown like any other: **no proxy administration**, no
automatic registration of servers, no surface of its own.

A proxy has no world and no `server.properties`. That still costs us almost nothing, because
the existing pages hold:

| Page | Velocity |
|---|---|
| Console, metrics, files, access | unchanged |
| Content | holds — Modrinth lists `velocity` as a plugin platform |
| Backups | holds; there is just no world to back up |
| Settings → Properties | Modrinth's page already has the empty state *"No properties found"*. Without `server.properties` it shows that — correctly and with nothing to do. |

`velocity.toml` and `forwarding.secret` are edited through the file manager, like any other
configuration file. The only real difference is the startup command: Velocity runs without
`nogui` and with flags of its own. That is one line in the loader definition.

## Phases

Every phase leaves something behind that runs. The done criterion is checkable each time.

**P0 — Scaffolding and the privilege split.** A Rust binary that serves the built interface,
creates SQLite and has a sign-in. Plus the two-part split from the start: an unprivileged
service and a root helper with the three commands, the directory layout with system account
and group, the delegated cgroup.

That belongs in the first phase because it is hard to retrofit: the startup path of a
process is exactly the place you do not want to write twice.

*Done when:* the binary starts on an empty machine, the sign-in screen appears, `ps` shows the
service as `craftpanel` and a test process demonstrably runs as `craft-<id>` in its cgroup.

**P1 — The proof.** Create a server (Vanilla or Paper), start, stop, restart, kill. Console
and metrics live. It is driven by Modrinth's **unchanged** `ServersManageOverviewPage`.
*Done when:* a real server runs through Modrinth's page, the console writes along and the
input line sends commands, without a line in `packages/ui` having been changed.

> This phase decides the whole plan. If the assumption does not hold, better to find out
> here than in P5.

**P2 — Files.** The Kyros file interface, and behind it the file manager with editor,
upload, unpacking.
*Done when:* `ServersManageFilesPage` runs unchanged, including undo and the editor.

**P3 — Content.** Search against the real Modrinth, installation onto disk, updates through
the Modrinth API, enable/disable/delete one at a time and in bulk. Modpacks after that.
*Done when:* a mod from the search is installed, disabled, updated and deleted, and a modpack
can be installed and updated.

**P4 — Settings and the second wave of loaders.** `server.properties`, Java runtime and
startup command, ports, reinstall and version change. Plus NeoForge, Quilt and Forge: they
need an installation step and a startup command of their own, and that is exactly what
becomes visible on this page.
*Done when:* all five settings pages write, the server picks the change up, and a NeoForge
server starts out of the installer.

**P5 — Backups.** Create, restore, rename, delete, queue.
*Done when:* a world has been backed up, destroyed and restored.

**P6 — Accounts, roles and limits.** Our own sign-in, invitations, the three roles over the
ten permission bits, an audit log. Plus the user administration for the administrator: create
panel users — which creates the system account with them — and set the memory, CPU and
process limits per user.
*Done when:* an editor can restart but cannot delete files. Checked, not assumed. And: a
user who maxes out their CPU limit gets a measurably lower tick rate, **without a server
crashing**, while everybody else's servers keep running unimpressed. The emergency brake must
not have fired while doing it.

**P7 — Installer and delivery.** The script complete, binaries for the usual architectures,
a version check.
*Done when:* install, update and uninstall have each run through once on a fresh machine.

A minimal installer comes into being back in P0. Otherwise every later phase gets tested in
a way that has nothing to do with delivery.

## Open points

- **The name.** The folder is called `MinecraftServerManager`; that is a working title.
- **Loaders beyond the eight.** Sponge and the hybrids are deliberately out; if they are
  wanted after all, Sponge is the easiest latecomer.
- **SFTP** — in or out.
- **Limits per server**, not only per user. The cgroup layout allows a further level
  below; not needed today.
- **Tied to Linux.** System accounts, cgroups and systemd require Linux. For a Minecraft
  server that is the normal case, but it is a commitment.
