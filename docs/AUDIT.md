# Status

As of 2026-08-13, measured against the running service on `127.0.0.1:8099`, binary
`/usr/local/bin/craftpanel` (md5 `a762368996f0f712d03a453256a39726`).

The basis is `docs/WIRING.md` and the six playthrough reports with their re-checks. What
stands here, somebody executed. Where I measured myself, it says **own**: my own account
`bilanz`, my own servers, my own calls. Where an area measured and its re-check held, the
area is named. "Should work" appears nowhere.

Two numbers up front: **21 functional areas hold, 14 are broken.** Nine of the fourteen are
the same thing: built, tested, and nobody calls it.

---

# 1 · What works

| Function | Evidence |
|---|---|
| Create an account, sign in, forced password change | **own**: `admin create --print-password`, `POST /auth/login` → 200 with a session, `POST /me/password` → 204 |
| Create a server, all three loaders | **own** paper 1.21.4 → `201`, done in 24 s (cold) or 5 s (jar in the cache). Area 1: `vanilla 1.21.4`, `paper 232`, `fabric 0.19.3`, 201 each |
| Operation model (5.x) | **own**: `server_create` runs `queued → done` with `started_at`/`finished_at`; Area 1: snapshot `revision:7, state:done, progress:1, phase:writing_config` |
| Starting | **own**: `POST /power {start}` → `202 {"power_state":"starting"}`, JVM `-Xmx1024M -jar server.jar nogui`, port 25569 bound. Area 1: `Done (3.5 s / 12.0 s / 5.5 s)` |
| Stopping and deleting | **own**: stop `202`, JVM gone; delete `202`, directory gone, `GET` afterwards `404 server_not_found`, list empty |
| The socket (13.4) | **own** — 40 s on a running Paper: `server 1, state 1, operations 1, console_history_start/end 1, console 1, stats 30`. Exactly one sample per second |
| Measurements, five fields | **own**: `{"type":"stats","cpu_percent":0.75,"ram_usage_bytes":1445814272,"ram_total_bytes":1073741824,"storage_usage_bytes":245016431,"storage_total_bytes":31593590784}`. **K2 from WIRING is done**: `main.rs:177` starts `sample_stats` |
| Ring buffer on reconnect | Area 1: exactly 10 samples out of the ring before the live frames |
| Reading and writing the console | **own**: `POST /console/command` → `204`. Area 1: `[Panel/INFO]: > say …`, then `[Not Secure] [Server] …` |
| Console `seq` without gaps during a run | Area 2: `seq 0…180` without a hole across two process runs; after reattaching `81+20 → 101`, no jump back |
| Console logs, limits | Area 2: 25,000 lines, first `line 275001`; 12 MiB without a line break capped at exactly `8388608`; `logs/./latest.log` and `logs//latest.log` → `409 log_file_in_use` |
| `crash-analysis` from `latest_log` | **own**: `200` with `"title":"Paper 1.21.4 Server Log"`, `analysis.information[]`, `prefix:"[05:49:53] [S…"` |
| Files: read, download, upload, rename, delete | Area 4: download `200` + `ETag` + `Accept-Ranges`, `Range` → `206`, `max_bytes` → `413`; a 3 MiB upload comes back md5-identical, `409 already_exists`, `on_conflict=overwrite` → `204`; delete `409 not_empty` → `recursive=true` → `204` |
| Files: escape barrier | Area 4: `../..`, `a/../b`, `%2e%2e%2f`, `..%2f..%2f` → `400 invalid_path`; `/etc/passwd`, `panel.db` → `404`; a null byte → `400`; a 300-byte segment → `400 path_too_long` |
| Files: symlink containment | Area 4 — links to `panel.db`, `/etc/passwd`, `/etc`, `../..`: reading and listing `403 forbidden_path`; writing replaces the link, the target is unchanged; deleting removes only the link |
| Content: search, install, on/off, update | Area 3: search `total_hits 13094`; `lithium-…jar` on disk with `-rw-rw---- craft-…:craftpanel`; bulk on/off, ULIDs unchanged; 7.1.1 → 7.1.3 with the same row ULID |
| Content: dependencies | Area 3: Chunky → Fabric API yields `{"dependents":[{"id":"…GXSC…","depends_on":["…GYC0…"]}]}` |
| Install a modpack from a file and unlink it | Area 3: `files_processed 83`, 66 entries in `/modpack/contents`, `unlink → adopted_items 66`, all on `local` |
| Settings: `server.properties` | Area 5: comments and unknown keys preserved on PATCH, `null` deletes, `server-port`/`query.port` → `409`, `known 24` / `custom 40`, `-Xmx` cleanup with `stripped_flags`, shell injection has no effect |
| Settings: ports, version, loader | Area 5: port matrix complete (8099 → `port_unavailable`, `<1024`, duplicate, limit 8); the primary swap writes both keys, `ss -ltnp` confirms; catalog exact (vanilla 906, fabric 518, paper 66, purpur 40, velocity 17, folia 12, leaf 8); family change with `loader_change_needs_wipe` |
| Permissions and roles | Area 6 — 31 calls on somebody else's server: 31× `404 server_not_found`, no 403, no leak. Viewer 13× read `200` / 13× write `403`. Editor 20 calls as in the contract. Administrator endpoints for non-administrators 14× `403` |
| Budget and limits | Area 6: 3× 512 MiB `201`, the fourth `409 budget_exceeded`; `-Xmx` against the same budget; lowering the limit below what is allotted yields `over_limit true`. Area 5: the upper bound probed directly, `22528` → `200`, `22529` → `409` |
| cgroup containment | Area 1: for a non-administrator `memory.high 2147483648`, `memory.max 2684354560`, `cpu.max 150000 100000`, `pids.max 256` really do stand in the kernel; `/me` reports `used_bytes 1287733248, used_cores 1.33, pids 41` under load |
| Backups: create, download, delete, accept a schedule | Area 4: the archive lies under `/var/lib/craftpanel/backups/<server>/`, `0700 craftpanel:craftpanel`, zero `*.tar.zst` in the server directory; 10.8 header with `filename*=UTF-8''…`; `999` → `400 invalid_schedule` |
| Rate limiter | Area 2: 20 get through, the 21st `429` with `retry-after: 10`. Area 4: `429` on file calls |
| Two operations on two servers both run through | **own**: kicked off at the same time, both `done`. Only one after the other, see N9 |
| `cargo build -p craftpanel` | **own**: `Finished dev profile`, 8 warnings |

---

# 2 · What does not work

Ordered by severity.

## N1 · A panel restart orphans every running server — and you cannot get rid of it

**What the user sees.** After `systemctl restart craftpanel` the server says "stopped",
although Minecraft keeps running and players are on it. And now it gets bad: all
**measured myself** on server `01KZWK83RT64QSB7XGAGBR2K04`:

| Action | Answer | Reality |
|---|---|---|
| Open the socket | 6 frames, `state` once, **0 measurements** | JVM 1195212 is running |
| "Stop" | `409 invalid_power_transition — the server is stopped and cannot stop` | the JVM keeps running |
| Send a command | `409 server_not_running` | the JVM keeps running |
| "Start" | `202` — a second JVM 1197158 starts, dies after 6 s on the occupied port | the panel says "stopped" again |
| "Delete" | `202`, row gone, directory gone, `GET` → `404` | **the JVM keeps running**: `/proc/1195212/cwd -> …/01KZWK83RT64QSB7XGAGBR2K04 (deleted)`, port 25569 stays bound |

The server has vanished from the panel, the process keeps eating 1 GiB and holds the port.
Only `ssh` and `kill -9` get it back. On top of that the orphaned supervisor fills the log:
every two seconds `rejected a supervisor claiming to be …`, measured 34 lines in 65
seconds — around **43,000 lines a day per orphaned server**. 243 such lines were already in
the journal before my attempt.

**Cause.** `crates/craftpanel/src/main.rs:151` calls `manager.reconcile().await` instead of
`manager.spawn_recovery()`. `spawn_recovery` (`crates/craftpanel/src/servers/manager.rs:1469`)
is the only place that calls `adopt_tokens`, and the only one that reaches `Hub::load_tokens`.
So `hub.tokens` is empty after the start, the returning supervisor is rejected at
`crates/craftpanel/src/servers/hub.rs:151`, and `reconcile` sees `attached=0`, writes
"stopped" and deletes the secret (`crates/craftpanel/src/servers/manager.rs:1406`). After
that nobody can ever attach again.

**Evidence**, my restart at 05:46:02:

```
INFO craftpanel::servers::manager: servers reconciled attached=0 cleared=1 broken=0 resumed=0
WARN craftpanel::servers::hub: supervisor connection ended: rejected a supervisor claiming to be 01KZWK83RT64QSB7XGAGBR2K04
```

**Size of the fix.** Replace one line (not add one) in `main.rs:151`. Two follow-ups belong
with it and are not the same line: the delete path (`servers/manager.rs:1351`) must not
believe a database row but has to look before clearing up whether a process is still running
(~20 lines); and `hub.rs:151` should end a supervisor it does not know instead of letting it
knock forever (~10 lines).

## N2 · The panel does not serve its own interface at all

**What the user sees.** `http://127.0.0.1:8099/` answers `404 not found` in `text/plain`.
Every asset URL likewise. There is no panel — only an API. **Measured myself.**

**Cause.** `crates/craftpanel/src/web.rs:9-11` embeds `web/dist` with `rust-embed`, without
the `debug-embed` feature. In a debug build `rust-embed` embeds nothing but reads from the
path at runtime. The installed binary is a debug build: it contains the string
`/root/MinecraftServerManager/crates/craftpanel/../../web/dist` and weighs 75 MB against 30 MB
for the release build. And `/root` is `drwx------ root`: the service user `craftpanel` never
gets in there (`runuser -u craftpanel -- ls …/web/dist` → Permission denied).

Important for honesty: `scripts/release.sh:15-23` builds the interface, checks
`web/dist/index.html` and then calls `cargo build --release`. That way everything would be
embedded. The way has just never been taken (see part 3).

**Size of the fix.** For this machine: install a release binary, one command. So that it
never happens unnoticed again, two options, both small: give `rust-embed`
`features = ["debug-embed"]` (one line in `crates/craftpanel/Cargo.toml:23`), or abort at
startup when `Assets::get("index.html")` is empty (three lines in `main.rs`). The second is
the better one: it says what is missing.

## N3 · Operations stall forever and lock the server for good

**What the user sees.** An operation sits on `queued` and never moves. Every write to this
server answers `409 server_busy` afterwards — files, content, settings, everything. There is
no button that resolves it.

**Evidence.** In the database stood `01KZWJE46JEBBM0DWGE7QGNQDT`, `install_modpack`, `queued`
since `2026-08-13T03:25:14Z`, `payload: none` — never started, never expired.

**Cause, twofold.**

1. `crates/craftpanel/src/main.rs:298-303` offers the work only to `manager` and `backups`:
   ```rust
   for id in queued {
       if manager.run(id).await { continue; }
       backups.run(id).await;
   }
   ```
   Content operations otherwise run along right in the API handler. If one of those survives
   a restart, nobody ever picks it up again.
2. `Operations::spawn_housekeeping` (`crates/craftpanel/src/ops/mod.rs:353`) has no caller.
   So `store::sweep_timeouts` with `STALL_LIMIT` (5.11) and `PAYLOAD_LIMIT` (5.7) never runs,
   and the watchdog that was supposed to clear exactly that away is asleep.

That a stalled operation really does lock is in
`crates/craftpanel/src/ops/store.rs:349-357`: `busy_reasons` counts `queued` **and**
`ongoing`, and `guard_write` (`crates/craftpanel/src/ops/mod.rs:185`) turns that into
`409 server_busy`.

**Size of the fix.** The watchdog: one line in `main.rs`. The content branch in the queue: a
`content.run(id).await` arm plus the matching `run` method in the content service — half a
day, because `Content` knows no resumption today.

## N4 · Automatic backups never run

**What the user sees.** The schedule can be switched on, the panel shows it as active and
names a next date. It never comes. `last_run_at` stays `null`, `keep_last` never clears up.

**Evidence.** **own**: `PUT …/backups/schedule {"enabled":true,"interval_hours":1,…}` → `200
{"next_run_at":"2026-08-13T04:44:56Z","last_run_at":null,"last_status":null}`. In the whole
database of this machine: `select count(*) from backups` → **0**. An automatic backup has
never existed.

**Cause.** `Backups::spawn_scheduler` (`crates/craftpanel/src/backups/schedule.rs:206`) has
no caller. It is the only place that calls `tick()`; `tick` the only one that calls
`run_scheduled`; `run_scheduled` the only one that reaches `store::record_run` and `prune()`.
Four links, no beginning. `TICK` is set to 60 seconds and does not tick.

**Size of the fix.** One line in `main.rs`.

## N5 · The line counter jumps back to zero at every restart

**What the user sees.** After a panel restart, `seq` in the console starts at 0 again.
Section 13.5 explicitly promises that it never does. A client that spots gaps through `seq`
does not spot them any more after that.

**Evidence.** All eight servers in the database have `console_seq = 0`, including the ones
with long console output.

**Cause.** `persist_console_counts` (`crates/craftpanel/src/ops/mod.rs:371`) is the only
writer of `servers.console_seq` and runs only inside `housekeeping()` — see N3. The same
missing line also takes out `purge()` (5.12, finished operations are never cleared away) and
`sweep_work_dirs()` (working directories stay behind).

**Size of the fix.** The same one line as N3.

## N6 · `crash-analysis` from the ring buffer always answers 502

**What the user sees.** "Analyze crash" fails as soon as you take the ring buffer — that is,
exactly after a real crash, when the seam has reset the buffer to the headerless Paperclip
and JVM lines.

**Evidence.** **own**, on a freshly started Paper 1.21.4, both calls one after the other:

```
POST /console/crash-analysis {"source":"buffer"}
  502 {"error":"upstream_unavailable",
       "message":"mclo.gs: mclo.gs answered something unreadable: invalid type: null, expected a string"}

POST /console/crash-analysis {"source":"latest_log"}
  200 {"title":"Paper 1.21.4 Server Log","analysis":{…"prefix":"[05:49:53] [S…"}}
```

**Cause.** `crates/craftpanel/src/console/mclogs.rs:53`: `pub prefix: String`. mclo.gs
delivers `analysis.information[].entry.prefix: null` as soon as the matched line has no
`[HH:MM:SS] [Thread/LEVEL]:` header. The neighboring field `time` is already an `Option`,
`prefix` is not. `docs/api/CONTRACT.md:3071` writes `prefix: string` and shares the blame.

**Size of the fix.** One word — `Option<String>` — plus the contract line.

## N7 · Nobody can accept an invitation

**What the user sees.** The owner invites somebody and sees an entry "invited" in
`Access.vue` that never changes. The invited person sees nothing at all: no inbox, no badge,
no page. Section 11 is a dead end.

**Cause.** The three endpoints answer (`GET /api/v1/invitations` → `200
{"invitations":[]}`, `accept`/`decline` → `404 invitation_not_found`), and `client.ts` has
all three functions. In `web/src`, outside `api/client.ts` and `api/types.ts`, there are
**zero** hits on `invitations`, `acceptInvitation`, `declineInvitation` — **counted
myself**. No route in `router.ts`, no page.

**Size of the fix.** One page, one route entry, one badge in the header bar — one to two days
of interface work. There is nothing to do on the backend.

## N8 · Modpacks from the catalog cannot be reached from the interface

**What the user sees.** There is no way to get a modpack from Modrinth onto a server. The
create wizard offers only loaders, the search tab never shows the type `modpack`.

**More precise than the wiring report said.** The file variant **is** wired up:
`web/src/providers/installation-settings.ts:431` calls `client.content.uploadModpack`, and
that goes to the same route `/content/modpack/install` (`web/src/api/client.ts:749`). Only
the catalog route has no caller: `client.content.installModpack`
(`web/src/api/client.ts:731`), and `New.vue:810` produces `kind: 'loader'` exclusively.
`modpack_project` and `modpack_upload` come into being nowhere. `browse-manager.ts:265` sets
`projectType = list.value?.content_type ?? 'mod'`, which never yields `modpack`.

`PUT …/operations/{op}/payload` (5.7) falls out with it: `client.ts` has `putPayload`, zero
callers, because the state that would need it is unreachable.

**Size of the fix.** One branch in the wizard and one call in the Content tab — one day.

## N9 · One long operation halts every other one in the whole panel

**What the user sees.** Several operations sit on `queued` although the panel allows two at a
time, and nothing moves until the first is finished.

**Evidence.** **own**: two `server_create` kicked off in the same second on two different
servers, `max_concurrent_operations = 2`:

```
A  created 03:48:51   started 03:48:51   finished 03:48:56
B  created 03:48:51   started 03:48:56   finished 03:49:00
```

B starts in the second A finishes. The jar was in the cache; on a cold 51 MB download B waits
a minute.

**Cause.** `crates/craftpanel/src/main.rs:298-303` awaits every run inside the loop.
`Manager::spawn_dispatcher` (`crates/craftpanel/src/servers/manager.rs:619`) was built for
exactly that and has no caller. Its own comment says it: *"a download that takes a minute may
not hold up a delete."*

**Size of the fix.** One line in `main.rs`, but only together with N3, otherwise there is
still no route for content operations.

## N10 · The download button on a backup leads nowhere

**What the user sees.** On a panel with TLS you click "Download" and get the start page as a
file. On `http` it is intact, because `Backups.vue:423` sets `downloadHost` only under TLS
and shows the substitute button otherwise.

**Cause.** `api::backups::compat_router` (`crates/craftpanel/src/api/backups.rs:88`) is
built, has four tests of its own and is not merged in `main.rs`. It is the only path outside
`/api/v1` (10.11). **own**: `GET /modrinth/v0/backups/{id}/download` → `404` in `text/plain`,
the same answer as a made-up route. `BackupItem.vue:116` from Modrinth's library builds the
link itself and writes `https://` into it hard.

**Size of the fix.** Two lines in `main.rs`: the merge has to stand **before**
`web::router()`, otherwise the SPA fallback route swallows it.

## N11 · A warning in the log after every normal stop

**What the user sees.** Nothing — until they look in the log. Then after every clean stop
there is a WARN line about a rejected supervisor.

**Evidence.** **own**, out of the journal, twice on other people's servers, the pattern is
always the same:

```
05:26:14  INFO  supervisor detached server=01KZWHC3WT0J0CMA3RMEX1YZA1
05:26:16  WARN  supervisor connection ended: rejected a supervisor claiming to be 01KZWHC3WT0J0CMA3RMEX1YZA1
```

Exactly two seconds later, exactly once per stop. The supervisor reports in one last time
after the panel has already forgotten its secret.

**Cause.** `crates/craftpanel/src/servers/manager.rs:807` forgets the token on stop, before
the supervisor has really ended its connection; `hub.rs:151` then rejects it.

**Size of the fix.** Small: hang the forgetting on the end of the supervisor connection, or
lower the line to `debug`. The second variant hides N1, though, which writes the same line.

## N12 · The memory tile can go over 100 percent

**What the user sees.** A server with 1024 MiB shows 1379 MiB used. The bar overflows.

**Evidence.** **own**: `ram_usage_bytes 1445814272` against `ram_total_bytes 1073741824`,
135 %.

**Cause.** `ram_total_bytes` is the allotted amount, not the cap. For ordinary accounts
`memory.max` stands at 1.25 times it (`memory.high` at exactly it), for panel administrators
at `max` — `crates/craftpanel/src/auth/limits.rs:113-118`, `Budget::Unlimited`. On top of
that the RSS of a JVM always lies above its `-Xmx`.

**Size of the fix.** Small, but a decision is needed: either report the cgroup limit as the
denominator, or cap the display and show the overflow as a note of its own.

## N13 · The console seam is missing on the first attach after a panel restart

**What the user sees.** If a server starts while the panel was down, the separator between
the old and the new process is missing in the console. Only from the second start on is it
right.

**Cause.** `crates/craftpanel/src/ops/follow.rs:52`:

```rust
let fresh = last_pid.insert(server, link.pid).is_some_and(|pid| pid != link.pid);
```

`insert` returns `None` the first time, so `is_some_and` returns `false`. The first attach
after every panel start therefore never counts as a new process.

**Size of the fix.** One line.

## N14 · `curl … | sh` installs nothing

**What the user sees.** The one-line installer promised in the project goal aborts.

**Evidence.** **own**: `install.sh:6` points at `MCbabel/craftpanel`. The repository is
there — `https://api.github.com/repos/MCbabel/craftpanel` answers `200` — and
`…/releases/latest` answers `404`: nothing has been published from it yet, which is the
one that makes the installer stop. (The first version of this line said neither existed.
That was wrong about the repository, and re-measured on 2026-08-24 it reads as above.)

**Size of the fix.** No code — publish. But as long as that has not happened, `install.sh`
(349 lines) is entirely unchecked, see part 3.

## Small stuff, evidenced and undisputed

- The "send to mclogs" button is missing in the file manager: `canShareToMclogs` and
  `shareToMclogs` are not filled in the contract filler, so `FileNavbar.vue:199` hides it.
  The backend can do it (`POST /console/crash-analysis`), only not from here.
- `pnpm --filter @craftpanel/web build` checks no types (`web/package.json:9` is `vite
  build`); `typecheck` is a script of its own and part of no flow. Today zero type errors
  in `web/src`. A missing required field in a contract would still not show at build time.
- `Retry-After: 61` instead of `6` on the backup rate limiter (Area 4) — a value, not a bug.
- An account cannot delete itself (`403 cannot_delete_self`), and `craftpanel admin`
  knows only `create`. Every review run leaves a dead record behind.

---

# 3 · What went unchecked

This is the part you leave out. It stands here in full.

**Restoring a backup.** The route is there
(`crates/craftpanel/src/api/backups.rs:78`), the event `BackupRestored` is recorded
(`api/backups.rs:204`), and in the audit log of this machine there is not a single
`backup_restored`. In total there is exactly one `backup_created` and one `backup_deleted`.
Nobody has ever restored a backup. Why not: it destroys a world, and no area reserved a
throwaway server for it. That is the most dangerous gap in this list, because a backup you
cannot restore is none.

**Automatic backups for real.** As long as N4 stands, there is no way to check whether
`run_scheduled`, `record_run` and `prune` do the right thing. Their tests are green; whether
they are right on real data, nobody knows.

**The interface as the panel serves it.** Every browser check of the six areas ran against
`vite preview` — two of those run on this machine, ports 5199 and 4319, both answer
`200 text/html`. The service on 8099 answers `404`. **Nobody has ever seen the interface the
panel serves itself.** Every statement about click paths, navigation and page errors holds
for the preview mode, not for the product.

**The release build and the bundle.** `scripts/release.sh` builds `--release`, checks
`web/dist/index.html` and ties up a `.tar.gz`. No report has run the script and installed its
bundle. So we do not know whether the embedded interface really is served in a release
build, only that it is not in a debug build.

**The installer.** 349 lines of `install.sh` with account creation, two systemd units, a
cgroup root, an update path and a removal path. Never executed, because the release does not
exist (N14). The removal path is the touchiest of them and the least looked at.

**playit.gg.** `playit_tunnels` 0 rows, `playit_released` 0 rows. There are 70 KB of
`docs/PLAYIT.md` and not a single tunnel. A real test needs somebody else's account; nobody
had one. The one sign-in that was started by accident was withdrawn again.

**Rebooting the machine.** `craftpanel-helper.service` is set to `Before=craftpanel.service`,
and `main.rs:156` waits for it with `await_helper`. Whether that holds after a real cold
start — with the cgroup root, system accounts and `/run/craftpanel` — nobody has tried. A
service restart is something other than a boot.

**Upgrading an existing installation.** Five migrations are in place, `0005_servers.sql` is
the newest. Nobody has ever pulled an older database up. Every check ran on a schema that was
complete from the start.

**The two silent background tasks.** `audit::spawn_purge` (24 h, `main.rs:183`) and
`sweep_upload_parts` (1 h, `main.rs:184`) demonstrably stand in `main.rs`. Both stay silent
when they succeed, both take hours to have an effect. Nobody has seen them tick — I have not
either. One `tracing::debug!` line per round would settle that forever.

**Real players.** No Minecraft client has ever connected. We know that the port is bound and
that `say` arrives in the console. Whether anybody can play, we do not know.

**TLS, a reverse proxy, a host other than `127.0.0.1`.** Everything ran on
`http://127.0.0.1:8099`. That is no small thing: `same_origin`
(`crates/craftpanel/src/api/ws.rs:84`) compares `Origin` against `Host`. Behind a proxy that
rewrites `Host`, every socket closes with `4403`. Unchecked. Likewise the `Secure` attributes
on the session cookie and the TLS branch from N10, which is exactly the opposite of the case
that was checked.

**Load and edges.** Never more than three servers at once. The port range has 136 slots
(25565–25700): exhaustion was only checked with an artificially narrowed span. A full disk,
an OOM kill through `memory.max`, a network failure in the middle of a download: all
unchecked.

**The test suite.** I did **not** run `cargo test -p craftpanel` again: the working
directory carries eight changes from other agents, and a red run would have said nothing
about the state. The last recorded run is `909 passed; 0 failed; 7 ignored`
(WIRING); the brief names 907. Nobody has brought the two numbers together.

**The interface on a phone** and the **39 unfilled provider fields**: rated harmless, but
never looked at on the running screen.

---

# 4 · The pattern

## The count

Of the fourteen faults in part 2, **nine** are the same fault: *built, compiled, tested — and
nobody calls it.*

| # | Fault | What is missing | Built, not wired up? |
|---|---|---|---|
| N1 | Restart orphans servers | `spawn_recovery` never called | **yes** |
| N2 | The interface is not served | `web/dist` built, not served | **yes** |
| N3 | Operations stall | `spawn_housekeeping` never called, no content branch | **yes** |
| N4 | No automatic backups | `spawn_scheduler` never called | **yes** |
| N5 | Line counter jumps to 0 | `persist_console_counts` unreachable | **yes** |
| N6 | `crash-analysis` 502 | `prefix: String` too narrow | no — type error |
| N7 | Invitations with no inbox | three `client.ts` functions with no caller | **yes** |
| N8 | Modpacks from the catalog | `installModpack` with no caller | **yes** |
| N9 | Operations run serially | `spawn_dispatcher` never called | **yes** |
| N10 | Download button leads nowhere | `compat_router` not merged | **yes** |
| N11 | Warning after every stop | the order of the forgetting | no — sequencing error |
| N12 | RAM tile over 100 % | wrong denominator | no — display error |
| N13 | Missing console seam | `is_some_and` the first time | no — logic error |
| N14 | Installer downloads nothing | the release is missing | no — process |

**9 of 14.** Take only the seven a user notices immediately (N1–N5, N7, N9) and it is seven
out of seven.

## Why the compiler stayed silent

That is the actual finding. Rust knows this fault and has a name for it: `dead_code`. It
would have named every one of these points at build time. It did not, because **twelve files
begin with `#![allow(dead_code)]` on the first line**:

```
crates/craftpanel/src/servers/manager.rs:13    crates/craftpanel/src/files/mod.rs:17
crates/craftpanel/src/loaders/mod.rs:3         crates/craftpanel/src/backups/mod.rs:20
crates/craftpanel/src/settings/mod.rs:10       crates/craftpanel/src/api/mod.rs:6
crates/craftpanel/src/model.rs:7               crates/craftpanel/src/auth/mod.rs:11
crates/craftpanel/src/ops/mod.rs:10            crates/craftpanel/src/audit/mod.rs:23
crates/craftpanel/src/content/mod.rs:14        crates/craftpanel/src/playit/mod.rs:12
```

A `#![…]` at the top of a file applies to the whole module including its submodules. **Every
single dead entry point from N1, N3, N4, N5, N9 and N10 lies inside these twelve files.**
`cargo build -p craftpanel` reports eight warnings today, and not one of them concerns any
of these places. The signal was there. It was switched off by hand.

## The one measure

A script, `scripts/check-nothing-dead.sh`, that runs in the delivery chain and before every release.
It levers the twelve `allow` lines out with `--force-warn` without touching them, does the
same for `client.ts` against the interface, and compares the result with a checked-in
baseline. Anything new makes the build fail; the baseline only gets shorter.

```sh
#!/bin/sh
# Every entry point needs a caller. Aborts as soon as something is dead
# that is not in the baseline; the baseline only gets shorter.
set -eu
cd "$(dirname "$0")/.."
BASE=scripts/dead.txt
OUT=$(mktemp -d)

CARGO_TARGET_DIR=target/dead RUSTFLAGS="--force-warn dead_code" \
	cargo check -p craftpanel --message-format short 2>&1 |
	grep '^crates/' |
	grep -E 'never used|never constructed|never read' |
	sed 's/:[0-9]*:[0-9]*: warning:/ /' |
	sort -u >"$OUT/rust"

( cd web/src
  grep -oE '^	[a-zA-Z][a-zA-Z0-9]*:' api/client.ts | tr -d '\t:' | sort -u |
  while read -r fn; do
	grep -rqlE "\b$fn\(" --include='*.ts' --include='*.vue' --exclude=client.ts . ||
		echo "web/src/api/client.ts  $fn"
  done ) | sort -u >"$OUT/web"

cat "$OUT/rust" "$OUT/web" | sort -u >"$OUT/dead"
diff -u "$BASE" "$OUT/dead" && exit 0
echo "Something is built and nobody calls it. Wire it up - or, with a reason, add it to $BASE." >&2
exit 1
```

**Executed, not proposed.** I ran the script against an empty baseline: return value 1, 106
entries, among them verbatim

```
crates/craftpanel/src/api/backups.rs        function `compat_router` is never used
crates/craftpanel/src/backups/schedule.rs   methods `tick`, `run_scheduled`, …, and `spawn_scheduler` are never used
crates/craftpanel/src/content/mod.rs        methods `check_updates_gently`, `sweep_updates`, and `sweep_once` are never used
crates/craftpanel/src/servers/manager.rs    methods `operations`, `spawn_dispatcher`, `dispatch`, `adopt_tokens`, `spawn_recovery`, and `spawn_metrics` are never used
crates/craftpanel/src/ops/mod.rs            constants `RETENTION`, `STALL_LIMIT`, `PAYLOAD_LIMIT`, `HOUSEKEEPING` are never used
web/src/api/client.ts                       acceptInvitation
web/src/api/client.ts                       aliasDownloadUrl
web/src/api/client.ts                       declineInvitation
web/src/api/client.ts                       installModpack
web/src/api/client.ts                       invitations
web/src/api/client.ts                       putPayload
```

The box is the output from back then and stays as it is. For its third line the opposite now
holds: `main.rs:175` calls `content.sweep_updates(live.clone())`, so `sweep_once` and
`check_updates_gently` are alive too (WIRING, T1 resolved).

That is N1, N3, N4, N5, N7, N8, N9 and N10 — at build time, with file and line, months before
a user presses the start button. Two moves belong with it: the short format lumps
`spawn_housekeeping` under `ops/mod.rs multiple methods are never used`, and a `cargo check`
without `--message-format short` shows the names; and the interface half catches a dozen
property names (`base`, `path`, `total`) along with it, which is what the baseline is for.

Filling the baseline when you create it is not a surrender but the point: it turns an
invisible pile of dead paths into a file that has to get shorter and whose growth somebody has
to justify.

---

# The bench

| | |
|---|---|
| `cargo build -p craftpanel` | **green**, 8 warnings (**own**) |
| `cargo check` with `--force-warn dead_code` | 89 dead spots in `crates/craftpanel/src`, 17 in `web/src/api/client.ts` (**own**) |
| `cargo test -p craftpanel` | not run again; the last recorded run 909 green |
| `pnpm --filter @craftpanel/web build` | green according to WIRING; `web/dist/index.html` is in place |
| Service | `craftpanel` and `craftpanel-helper` active, API fully reachable, interface `404` |

Tidied up: servers `bilanz-probe`, `bilanz-a`, `bilanz-b` created, started, stopped,
deleted; the JVM orphaned at N1 and its supervisor ended by hand, port 25569 free;
`GET /api/v1/servers` gives `{"servers":[],"users":{}}` again. Not one source file changed.
The test account **`bilanz`** (`01KZWK3MKVWCJ9YAMYNNEKTKKC`) remains: it cannot delete
itself, another administrator clears it away with
`DELETE /api/v1/admin/users/01KZWK3MKVWCJ9YAMYNNEKTKKC`.
