# The operator's side of the Java runtimes

As of 2026-08-22. Build record for the switch and the page an administrator uses to look after the
Java runtimes under `<data_dir>/runtimes/`. The engine that fetches and unpacks them is
`crate::java` and is written up in `docs/JAVA.md`; this page carries only what an operator sees and
presses, and why.

Every statement about this tree carries `file:line`. The one measurement about running JVMs was
taken on this machine on 2026-08-22 and is printed in full in section 4.

---

## 1. What was missing

The engine writes into `<data_dir>/runtimes/`, and `Manager::java` reads from it. Between the two
there was nothing an operator could see or decide:

* no way to say **no**. A machine with no route out, or an operator who installs his JVMs with the
  package manager, had no switch — the panel would simply try.
* no way to see what is lying there. `du -sh` over a data directory the panel owns is not an
  answer for somebody who administers through a browser.
* no way to get rid of one, and no way to fetch a newer build of the same major.

## 2. One switch, and it starts open

`panel_settings.java_auto_install`, added by `0015_java_auto_install.sql`, default `1`.

Every other switch this table has learned since `0002` starts closed. The reason for the exception
is in the migration and in one sentence here: a panel that cannot start a 1.12 server is not a
careful panel, it is a broken one, and fetching a signed archive over https opens nothing — no
port is listened on, no account comes into being. `registration_enabled` starts closed because it
lets strangers through the front door; this one does not.

**It is not folded into `external_services_enabled`.** That switch is described to the operator as
Modrinth content and the crash log service, and the sentence under its off state promises "Servers
keep running" (`admin.settings.external.off`). A switch that also decided whether a server may
start would make that sentence untrue. They are two questions, and the operator who blocks
Modrinth is not the same operator who manages his own JVMs.

Where it is read: `Manager::fetches_java` (`servers/manager.rs:1082`), on the start path
(`:711`, `:1062`). Nothing else reads it. The buttons on the admin page deliberately do **not** ask
it: the administrator pressed them himself, and a button that silently does nothing because of a
setting on another page is worse than no button.

The control: `web/src/pages/admin/settings/Java.vue:12`, a section of the panel settings page
registered in `web/src/pages/admin/settings/sections.ts:15`. It sits there and not on the runtimes
page for the reason `0012_drive.sql` already wrote down: `PUT /admin/settings` replaces that row as
a whole, so a second page reading it, editing one field and writing it back would clobber whatever
somebody else had open in another tab. `docs/INTERFACE.md:62-67` says the same.

## 3. One page, one row per major

`web/src/pages/admin/Runtimes.vue`, reached from the admin menu at
`web/src/pages/admin/routes.ts:69-74` ("Java runtimes", the coffee cup).

The rows are **per major version**, not per directory. `runtimes/java-<major>` is the only name the
engine ever writes (`java/mod.rs:69`), the majors the panel ever asks for are the four
`default_major()` can answer (`java/inventory.rs:16`), and a row per major is what makes "fetch it"
and "there is none" the same line rather than two different lists.

Three facts sit in each row, and the third one is the one that stops a needless download:

* what the panel fetched: version, vendor, size on disk (measured, `files::measure`), when, where.
* **what the machine already has of its own.** A row for Java 21 with nothing under
  `runtimes/java-21` but a `/usr/lib/jvm/java-21-...` on the machine says so and names it. Without
  that field the page would read "Java 21: not here" on a machine that has had a perfectly good
  Java 21 all along, and the operator would download a second one.
* who wants it: how many servers resolve to that major, and the names of the ones running right
  now. The resolution is the same one the rest of the panel uses — the server's pinned
  `java_major`, else `default_major(game_version)` (`java/inventory.rs:235-264`). A major that only
  a server asks for gets a row too, even one the panel cannot fetch: a server pinned to Java 11 on
  a machine whose Java 11 came from the package manager would otherwise appear on no list at all.

Progress comes from `java::Progress` through `GET /admin/java-runtimes`; the page polls every
700 ms while a job says it is running and not at all otherwise
(`web/src/pages/admin/runtimes.ts:pollDelay`, tested in `runtimes.test.ts`). A failed attempt keeps
its reason in the row until the next attempt for that major (`java/inventory.rs:268-283`) — in
memory, so a restart clears it.

## 4. Why a running server blocks both buttons

**Measured, 2026-08-22, on this machine.** A copy of `/usr/lib/jvm/java-21-openjdk-amd64` was made
under `/var/tmp`, a small program started from that copy, and the whole copy then removed with
`rm -rf` while the process ran:

```
up
late class: java.util.zip.Deflater
Exception in thread "main" java.lang.UnsatisfiedLinkError: 'int sun.nio.fs.UnixNativeDispatcher.init()'
	at java.base/sun.nio.fs.UnixNativeDispatcher.init(Native Method)
	…
	at java.nio.file.Files.createTempFile(Files.java:924)
```

Read it in the order it happened. Loading a class the JVM had not touched yet **worked**:
`lib/modules` is mapped, the inode outlives the directory entry. Then the program asked for a
temporary file, the JVM went to `dlopen` `libnio.so` for the first time — a file, not a mapping —
and that one was gone. `UnsatisfiedLinkError`, and the process was dead.

So the honest sentence is not "it crashes at once" and not "nothing happens": **it survives until
the first thing it had not yet needed**, and for a Minecraft server that is a matter of minutes,
with a stack trace that names netty or NIO and never Java.

That settles the question the brief asked. Both `POST` (fetch again, which replaces the tree) and
`DELETE` refuse with `409 java_runtime_in_use` while a server that resolves to that major is
running, and the message names the servers (`java/inventory.rs:215-232`). The alternative —
leaving the old tree standing until nobody uses it — needs a reference count over processes the
panel does not own, a sweeper, and a second directory name per major; three moving parts to avoid
one sentence that says "stop the server first".

The page does not rely on the refusal to be correct: it disables both buttons and explains why,
out of the `running` list it already has (`Runtimes.vue:99`, `:112`, `canFetch`/`canRemove` in
`runtimes.ts`). The `409` is the guard for anything that is not our page.

**Fetching when nothing is there is never blocked**, even if a server is running on a *system* Java
of that major: `undisturbed` returns early unless the panel's own tree exists
(`java/inventory.rs:216-218`). Laying a new directory down beside a JVM that is running out of
`/usr/lib/jvm` takes nothing away from it.

## 5. Fetching again

`Runtimes::install` returns early when the runtime is already there — that is section 3 of
`docs/JAVA.md` and it is right for the start path. The page needs the other thing, so the engine
grew a second entry point: `Runtimes::reinstall` (`java/mod.rs:95`) runs the same work under the
same per-major gate but without the early return. `lay_out` is now just "the early return, then
`replace`" (`:117-131`), so both paths fetch, verify, unpack and swap through exactly the same
code, and the swap already handles a tree standing in the way (`swap_in`).

The button therefore has one meaning in both states: **make this the current runtime.** When
nothing is there it installs; when something is there Adoptium is asked for the newest build of
that major and the tree is replaced atomically, the old one going only after the new one stands.
`api/runtimes.rs` needs no second route for it, and the interface only changes the word on the
button.

## 6. The endpoints

All three are administrator-only and carry the same-origin guard.

| Call | Answer |
|---|---|
| `GET /api/v1/admin/java-runtimes` | `200` `RuntimeOverview` |
| `POST /api/v1/admin/java-runtimes/:major` | `202` + the overview; the work runs on |
| `DELETE /api/v1/admin/java-runtimes/:major` | `200` + the overview |

`POST` and `DELETE` answer with the whole overview so the page does not need a second round trip
to show what changed — the same shape `PUT /admin/drive` uses.

Refusals: `403 forbidden` (not an administrator), `404 java_major_unknown` (a version the panel
does not fetch, and the sentence lists the ones it does), `404 java_runtime_not_here` (delete with
nothing there), `409 java_runtime_in_use`, `409 java_install_running`, and for a fetch that fails
the reason lands in the row rather than in the response, because the response has long been sent.

## 7. "Java {major} (will be downloaded)" is gone

`Advanced.vue` carried that label since the page was built. It could never be shown: the version
list is built from the runtimes the backend found, and every entry a found runtime produces has
`installed: true` (`settings/runtimes.rs:read_home`). The brief asked for a decision, and the
decision is **out**, together with the `&& runtime.installed` that was always true
(`Advanced.vue:262-266`).

The reasoning, in the order it decided the matter:

1. **Nobody needs the entry.** The dropdown is an override. The ordinary path is "Automatic", and
   automatic is exactly the case the fetching was built for: the game version decides the major and
   the start path lays it down. A 1.12 server on a machine that only has Java 21 is served without
   anybody touching this dropdown.
2. **The override that is missing has a better home.** "I want Java 17 on this 1.20 pack, and this
   machine has only 21" is an operator's decision about the machine, not a per-server one — and it
   is now one button on the admin page, after which 17 appears in everybody's dropdown, with a
   version, a size and a way back out.
3. **Offering it here would need three more refusals to be taken out.** `write_startup` rejects a
   major that is not on the machine (`api/settings.rs:invalid_java_version`), a vendor that is not
   (`invalid_jre_vendor`), and a pair that exists only as two halves (`runtime_not_installed`,
   whose message reads "and this panel fetches none"). Those three belong to the start path's
   contract, and loosening them from the interface side, while the start path is being rebuilt, is
   how a control comes to promise something no endpoint keeps.
4. **The switch can be off.** With `java_auto_install` off, "(will be downloaded)" is a lie, and
   the user reading it is not the administrator who turned it off and cannot see the setting.

**This is a known deviation from the written contract.** `docs/api/CONTRACT.md:2309` and
`docs/api/settings.md:652` say `installed: false` means "known and obtainable, but not on disk
yet", and that picking such a runtime makes the backend fetch it on save as an `install_java`
operation. Neither file is touched here — the contract is the measure, not the record of what we
built. What we build instead: the same `install_java` operation, raised on the start path where the
server that needs the runtime is the thing being waited on, and the manual way on the admin page.
The field `JavaRuntime.installed` stays in the answer; it is documented, it is `true` for
everything the endpoint lists, and no page reads it any more.

The help text under the dropdown was corrected with it: "Only versions this machine already has are
offered. On Automatic the panel takes the one your Minecraft version needs."
(`craftpanel.settings.advanced.java-version-help`). It deliberately does not promise the download,
because from where the user stands it depends on a switch he cannot see.

## 8. What is tested

`java/inventory.rs` (8 tests, no network): a row for every major in order; size, vendor and
version read off the tree that is there; the count of servers that want a major and the names of
the ones running; both buttons refused with `java_runtime_in_use` and the tree still standing
afterwards, then accepted once nothing runs; delete on nothing; delete taking the minute-long
discovery cache with it (`runtimes::forget`); an unfetchable major refused before anything is
touched; a row of its own for a major only a server asks for; the switch reported as the
administrator set it.

`api/runtimes.rs` (7 tests, against the fake Adoptium of `java/harness.rs`): signed out is `401`
and a plain user is `403` on all three; the overview's shape; `404` for a major we do not fetch and
for a path segment that is not a number; **one press lays 21.0.12+7 down and a second press
replaces it with 21.0.13+9, with two downloads served** — which is the test that proves
`reinstall` does not take the early return; a failed fetch leaving `failure_code` in the row; and
delete, followed by `404 java_runtime_not_here`.

`web/src/api/runtimes-reachable.test.ts` holds the house rule of this project: each of the three
calls is on the client, is made from `Runtimes.vue`, and sits in the function a person sets off —
plus the menu entry that leads to the page and the toggle bound to `java_auto_install`. An endpoint
without a control counts as not built here, and this test is what says so out loud.

`web/src/pages/admin/runtimes.test.ts` (5 tests): polling only while something runs, telling a
fetched runtime from one the machine already had, both buttons off while a server runs, nothing
fetchable on an architecture Adoptium does not build for, and a failure shown only once the attempt
has stopped.

The German catalogue carries all 53 new messages; `scripts/locale-extract.py --check` and
`web/src/locales/catalogues.test.ts` are clean.
