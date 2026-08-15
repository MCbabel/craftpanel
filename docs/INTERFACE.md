# The interface

As of 2026-08-15. What is written here lived in the comments under `web/src/**` until today and
nowhere else. The code no longer carries it; the decisions still hold.

What is **not** here: the contract itself (`docs/api/CONTRACT.md` and the files next to it), the
comparison with Modrinth's own interface (`docs/INTERFACE-PARITY.md`, which also holds
everything the vendored third-party code prescribes to our code) and the subject areas playit,
Drive, mail, sign-up and forgotten passwords, which have documents of their own. What stands here
is only what the interface decided **itself**, with its reason.

Line numbers are the state before the comments were cleared out. If you cannot find one any more,
search for the identifier named in the same file.

---

## 1. How the code is split up

### 1.1 What computes lives next to the page, not in it

`vitest` runs here without the Vue plugin. Everything in a `.vue` file is therefore testable only
by rendering, and an `import` from `@modrinth/ui` pulls in half the component tree. That is why
every decision that deserves a test lives as a pure function in a `.ts` file **next to** the page:

| File | what it decides |
|---|---|
| `pages/account/limits.ts` | what the account page shows from `GET /me` |
| `pages/account/playit.ts`, `pages/account/drive.ts` | which of the three or four states a card shows |
| `pages/admin/mail.ts`, `pages/admin/users.ts`, `pages/admin/drive.ts`, `pages/admin/registrations.ts` | draft, submit and order of the admin forms |
| `pages/auth/register.ts`, `pages/auth/recovery.ts` | field rules, error code to sentence, token from the address bar |
| `pages/servers/backup-target.ts`, `file-link.ts`, `memory-ceiling.ts`, `restart-hint.ts` | one question of the server pages each |
| `pages/servers/settings/memory-gb.ts`, `playit-ports.ts` | conversion and locking on the settings pages |
| `layouts/server-notice.ts`, `router-guard.ts` | the notice above the server, the guard's decision |
| `components/table-widths.ts`, `mail-words.ts`, `playit-words.ts` | measurements and words that two pages share |

The words are kept separate for the same reason: the state card and the list need the same
sentences, and a second identifier for the same sentence would be a translation nobody asked for.

### 1.2 Modules of our own next to `client.ts`

`api/playit.ts`, `api/mail.ts`, `api/drive.ts`, `api/registration.ts` and `api/recovery.ts` sit
next to `api/client.ts` so that not every area touches the same file. The eleven playit calls
belong unchanged as `const playit = { … }` next to `admin` in `client.ts`; then all that is left of
`api/playit.ts` is type and rule. That is open, not an oversight.

### 1.3 One list that two things come out of

The same construction twice, for the same reason: a page that stands in only one of the two lists
is built and unreachable, and no test turns red:

* `pages/auth/routes.ts` carries the session-free pages. `router.ts` builds **the route and the
  guard's allow list** from it. Whoever creates a public page and forgets the guard has a page that
  the link in a mail never leads to.
* `pages/admin/routes.ts` carries the admin pages. `router.ts` builds the route, `AppShell.vue` the
  menu entry, and `meta.admin` falls out for all of them at once. A route without a menu entry does
  not exist for the operator (`docs/WIRING.md`); maintaining both in two files has produced
  exactly this fault once already. The panel settings stay the last entry: they are the catch-all,
  and in a menu the catch-all goes at the bottom.

### 1.4 Two kinds of cards, and why they differ

`PUT /admin/settings` (12.11) replaces the row as a whole. Both of these follow from it:

* The extra cards of the panel settings (`pages/admin/settings/sections.ts`) edit **the same** draft
  as the page and have no save button of their own. Two buttons would be two paths that overwrite
  each other.
* `pages/admin/settings/blank.ts` holds the initial value until `GET /admin/settings` answers. A
  field missing there is written back with the default on the first save, even if the operator
  never touched it. So whoever extends `PanelSettings` extends there too, with the same defaults
  as migration `0010`, so that a form submitted before the first answer does not open the door and
  does not switch approval off.

The account page's cards (`pages/account/sections.ts`) do it the other way round: each loads, saves
and reports its errors itself and gets **no** properties, because each of these areas has an
endpoint of its own and a secret of its own. The same goes for everything with an endpoint of its
own (mail 19, Drive 22): own page, own save button.

---

## 2. Reachable, not only built

The standing fault of this project is called "built, but no control". The same case three times:
21.4 was built, wired and tested and had no button; `/registration-pending` had a route, a guard
allowance, a green test and a green pass on a phone, and no way there, because no page linked to
it; `email` stood in `CreateUserRequest` **and** `UpdateUserRequest` **and** in
`web/src/api/types.ts` while the user page had no address field.

Every time the core was right, and every time every test was green because it was cut from the same
wood as the thing being measured. That is why in the guards the **list** comes from outside and the
**evidence** comes from the pages:

| Guard | list (from outside) | evidence |
|---|---|---|
| `api/recovery-reachable.test.ts` | the four calls out of `recovery` itself | a trigger in the template, in the same function the call stands in |
| `pages/admin/users-reachable.test.ts` | the fields of the Rust structs in `crates/craftpanel/src/api/admin.rs` | control in the template **and** field in the submitted body |
| `pages/auth/routes.test.ts` | names and paths **by hand**, not from `publicPages` | a file that really contains the way there |
| `pages/servers/backup-drive-path.test.ts` | the chain 22.10 → 22.3 → 22.4 as text | backup page → account page → Drive card → device flow, link by link |
| `pages/servers/config-button-reachable.test.ts` | the way from the installed row to the folder | the row has exactly one place meant for a button of its own (`getOverflowOptions`) |
| `providers/browse-manager.test.ts` | the way from "already on the server" to the search page | button, selection and bar at the bottom |

Four things that go wrong when such a guard is built again:

* The cut is made at the **last** `</template>`, because `<template v-else>` closes inside the
  template just as well. A trigger in the script is not a button.
* The Rust structs are read with `pub` optional, the client types with `readonly` optional. That is
  not cosmetic: the existing code writes request fields with `pub` too (`auth/cli.rs`), and a
  `pub email: Option<String>` would otherwise have slipped through unseen, exactly the case the
  guard is there for.
* A body is counted by matching braces so that a call spanning several lines is caught whole, and
  comments are dropped beforehand, otherwise a field name in prose counts as proof. `updateUser` is
  sent from two places, and `password` travels along only at the second.
* Line breaks, indentation and quotes are the formatter's business and not the statement's — the
  text guards clear them away first.

Two of these chains end in the vendored code: at the config button and in the search page the last
links belong to Modrinth. If one of them falls away in a vendor update, our button disappears
silently. That is exactly when a line there should fail.

And a field the endpoint takes and the client does not know is just as unreachable as one without a
control; only nobody notices, because the interface knows nothing about it.

---

## 3. The guard, and what comes out of a mail

### 3.1 The decision

`router-guard.ts` decides without a router and without a browser, so that it is testable;
`router.ts` only turns the result into a destination. Every page's reachability hangs on it.

* Every session-free page says in `pages/auth/routes.ts` what happens to a **signed-in** visitor.
  `bounce` is right for forms that are moot for them. `allow` is right for every page that redeems
  a token: otherwise the redemption is lost in the redirect, and an account stays unverified
  because its owner was still signed in. Whoever clicks the link in their brother's mail has to be
  able to finish the verification.
* The token pages come **before** the password requirement: setting a new password satisfies it.
  The other way round, the password requirement comes before the administration, otherwise an admin
  sits in the administration with the installer password before having changed it.
* `?redirect=` takes our own paths only and sends nobody to a page they would be sent away from
  again straight away.
* The `401` handler outside the guard stays quiet on the very first call: that is the guard's own
  session check running, and the guard's redirect still knows the real destination.

### 3.2 The token

Four rules that belong together and must not be rolled back one at a time:

1. **In the body, never in the query** (1.2, 20.9, 21.5). In a URL it would sit in every access log
   — `main.rs` hangs `TraceLayer` over everything — and a mail client that prefetches links could
   use it up. A `GET` on the link loads the interface and nothing else.
2. **In the fragment, not in the query** (20.9, 21.5): a fragment reaches no server. `?token=` is
   still read all the same: a mail client that swallows fragments, and a link from an older mail,
   should not lead nowhere.
3. **Clear it out of the address bar at once**, fragment as well as query. The fragment does not
   keep it out of the history or out of a screenshot. Clearing goes through `history.replaceState`
   and not through the router, so that the page is not rebuilt and the redemption is not lost.
4. **One code for unknown, expired and used up** (21.2/21.3). Three outcomes in the interface would
   be the oracle about other people's tokens that the server is at pains to avoid.

`pages/auth/recovery.ts` stands apart from `pages/auth/register.ts` because the rules are
deliberately different: the reset token works **once** and lives 30 minutes, the verification token
24 hours and several times. Whoever copies one from the other builds a back door.

The cooldown (one mail per 60 seconds per account) stands on the button as the time remaining, so
that the server's brake does not look like an error.

### 3.3 The paths are the contract with mail delivery

20.9 builds `<link_base>/verify-email#<token>`, 21.5 builds `<link_base>/reset-password#<token>`,
and `link_base` comes from 19.2. Whoever renames one of these paths makes every mail already on its
way worthless. That is why the names stand by hand in `pages/auth/routes.test.ts` and are not
derived from `publicPages`.

The trailing slash of `link_base` falls away while building, otherwise the mail would carry
`//verify-email`.

### 3.4 What happens after the redemption

21.3 issues **no** session. So the page does not say "you are in", it sends you to the sign-in with
`?reset=done`, and the sign-in page says the sentence for it there, otherwise the whole path looks
like a failure.

20.8 takes "verified, but not approved yet" to a page of its own, because the same state is reached
two ways (verification mail, sign-in attempt); a third sentence under the form would be the same
information a third time. The redirect happens **before** the state is set, otherwise an
intermediate state flashes up. `email_unverified` stays on the form, because there is something
attached to it there and the person is already standing in it.

`/registration-pending` is deliberately plain text with no detail about a person: that makes the
page linkable without giving anything away about an account.

---

## 4. What an answer means

**A `404` is not always the envelope from 1.7.** The router answers a path that does not exist in
plain text. Measured against `craftpanel.service` ourselves: `HTTP/1.1 404 Not Found`,
`content-type: text/plain`, body "not found". Every facade that starts from the envelope has to
catch this case first: it means "this panel does not know the endpoint", not "your account is
gone".

**A `404` can be a statement.** 22.5 and 8.3 answer that way as soon as no sign-in flow is open any
more, **including after the successful one**. Whoever reads that as an error ends every successful
sign-in with an error message. Whether it worked is said by 22.3 and 8.1 alone: if a token is
there, that was the success. 8.7 answers `404` to two different questions; one means "this server
does not exist any more" and is none of the integration's business, every other one switches the
section off. Since "one account per user", "not connected" comes from `409 playit_not_configured`
alone.

**Only a `401` answers the session question for good.** A network error or a `500` leaves it open,
so `settled` stays false and the next navigation asks again. Signing out, by contrast, is
idempotent (3.2): if the call fails, the session in the interface is over all the same.

**A timestamp that cannot be read clears no operation away.** It counts as open then, and the
countdown shows `0:00` at progress 0 — the same rule at playit's `claimCountdown` (our deadline,
1.6) and at Google's device flow (Google's deadline, 22.4). A `NaN` in the bar would be the other
extreme.

**`null` as the interval means: do not ask again.** Without that exit every server page knocked
every thirty seconds at an endpoint that will never exist on a panel without playit. While
something is moving, we look more often; otherwise rarely, because 8.1 and 22.3 read one row each.

**Where a missing setup is the normal case, a failure must not be loud.** A panel whose operator
never set Google up is the normal case; the backup page has to look complete without that answer.
Likewise: without an answer from 19.2 nothing is warned about and nothing is locked. A warning or
a lock that rests on a network error is wrong and keeps somebody from something that would have
worked. 21.4 checks for itself anyway.

---

## 5. One empty field, three meanings

`api_key` (19.3), `email` (12.5) and `client_secret` (22.12) all three know the same distinction:
**leaving the field out means unchanged, `null` or `''` deletes, text sets.** The reason they exist
at all is the mistake behind them — otherwise every save of the sender address would take the
Resend key with it, and every change of the Drive rule the client secret; the panel would quietly
be back at "not set up".

For the forms this means:

* A draft is text throughout: a form field has no `null`. The translation into the three meanings
  happens in exactly one place per area (`pages/admin/drive.ts`, `pages/admin/mail.ts`,
  `pages/admin/users.ts`), and that place is testable.
* A secret field is **always empty** on load, because the secret never comes out of the panel. So
  "empty" means unchanged there and not delete, and there is no placeholder in it that would
  promise anything else.
* An empty address field sends nothing at all: 12.3 takes the address optionally, `null` would be
  the same, only more roundabout. Leading and trailing spaces fall away in the browser, so that the
  check there sees the same thing as the endpoint.
* Limits are never sent along for an admin; 12.3 would reject that. `PUT …/limits`, by contrast, is
  a full replacement, and every field has to stand there.

**Changing only the spelling is not a change.** 20.10 folds addresses to lower case, so
`Max@Example.test` is the same as `max@example.test`. A `PATCH` that changes nothing but the
spelling still counts in the core as an address change and throws the account's open reset token
away (21.8) — for nothing. That is why the comparison is done folded and not with `===`.

The thin address check in the browser (exactly one `@`, no whitespace, a dot in the domain) is
deliberately the same as in the backend (19.3, 20.10) and no stricter: the real check is Resend's
answer, and being stricter would mean rejecting addresses that work.

---

## 6. Sentences instead of identifiers

* **A sentence for every state, never the identifier** (10.2, 22.9). Whoever is affected is
  addressed, not described.
* **`null` is not a finding, it is a missing one.** Nothing connected is blue and not red, no key
  is not a fault but a step nobody has taken, and "not set up" is the normal state of a fresh
  panel: a sentence, no button, no red box. A row without a connection comes about as soon as
  somebody presses "Connect"; it once stood there as "Not working" before anything was connected
  at all.
* **"at least", as soon as a directory was closed to the panel while counting** (3.3). The game
  process may close its own folders; what lies in them takes up disk all the same. The number is a
  lower bound then, and passing a lower bound off as a measurement would be a lie.
* **Numbers travel on as numbers, not as finished text.** "1 core" against "1.5 cores" is decided
  by the plural rule, and that needs the value itself.
* **The reason for a rejection stays in the panel** (20.7). A rejection with a reason is an
  invitation to write something quotable.
* **A choice that files hang on is not made silently** (22.7): the two answers differ in whether
  the user's files are deleted.
* **A greyed-out button carries its reason underneath it**, not in a tooltip: on a phone there is
  none. The same goes for the warning above a list and for the sentence in the body of a dialog.
  The tooltip is the extra everywhere, never the carrier.
* **The order of the reasons is the endpoint's.** `auth/reset.rs` checks `no_email_address` first,
  then `mail_not_configured`; if the greyed-out button names the other reason, it sends the
  operator into the mail area, where there is nothing to get.
* **What the server says more precisely, we do not say after it.** Sentences of our own go only to
  the cases where the way out is not in the message: `email_taken`, for one, can also sit on a
  *pending application*, and you approve or reject that (20.6) instead of passing over it.
* **What may give nothing away gives nothing away when it fails either.** `invalid_credentials`
  also covers unknown names; 20.2 answers for a known address as for a new one, so that the form
  does not become a directory of names; 20.4 always answers `202`, so there is no error branch
  there; a failure shows the same sentence too. Only the two states from 20.8 may be explicit:
  they come **after** the password check.
* **A form that is certain to get a `409` is worse than one line of waiting.** As long as 20.1 has
  not answered, nothing is offered.
* **Do not take over what the operator is typing.** Otherwise a field jumps away under the finger
  when an answer arrives.

---

## 7. A way belongs to whoever can walk it

Drive and playit hang on the account of a server's **owner**, not on the account of whoever is
looking at it right now. The same pattern follows everywhere:

* Only the owner gets the button on their account page: it is their account that is missing, and
  an editor can change nothing about it. If the panel's Google project is missing, even the owner
  can do nothing; then the explanation stays without a way.
* An editor learns nothing about the other account beyond the one fact that a backup of this server
  currently has nowhere to go. That is why `BackupTarget` is read and not your own Drive state; a
  revoked access has arrived as `not_connected` since 22.9 anyway.
* The panel-wide switch from 12.10 can make a connected account ineffective. A user may not read
  the panel settings, so the page does not say it after it and the user learns it from the
  rejection, and gets no button leading there either.
* The admin overviews carry **no** `user_code` and **no** `claim` (22.11, 8.10): whoever confirms
  somebody else's code hangs their Drive on somebody else's panel account or gets themselves access
  to somebody else's playit account. An admin may only **disconnect** another account (8.11,
  22.13), because a port debt would otherwise stand for ever, and nothing is deleted in a
  stranger's Drive (22.14), so there is no button for that either.
* The user's own six playit calls do **not** sit under `/admin`. A path there would be a `403` for
  an ordinary user, and then there would be no sign-in of their own for them.

---

## 8. Backups: the target of the next run

`pages/servers/backup-target.ts` answers exactly one question: **can a backup come about on this
page at all?** No exactly when the next run would have to go into the Drive and nothing can go
there (10.2 then answers `409 drive_not_connected` or `409 drive_not_configured`).

* `reason: 'policy'` is expressly **not** an obstacle: with `drive_only` and a connected Drive that
  is the healthy state. The previous rule read `reason !== 'ok'` and locked every button on a
  `drive_only` panel, even when everything held.
* **Every** trigger on the page that starts a run is locked, each with the sentence next to it.
  Without that the button stood open next to a page that already explained the impossibility, and
  whoever pressed it got the `409`. A retry is a new run here: it cannot succeed while nothing can
  go there.
* Switching the target exists only with `user_choice`: with `drive_only` and `local_only` the
  operator has decided, and a switch that produces nothing but an error message is not a switch.
  Turning a schedule **off**, by contrast, always stays possible, otherwise somebody sits stuck in
  a schedule that fails every night.
* The comparison happens only when both sides are really there: `undefined === undefined` would
  otherwise be "yes".
* The fallback values of the three fields from 22 are those of a local backup. A row without these
  fields is one from before section 22 — and that one lies here on the disk. `DEFAULT 'local'` is
  the same reason why no old row had to be touched.
* `unreachable` means "disconnected", not "gone": that is what a row looks like after the user has
  disconnected with `?files=keep`, and connecting again finds the file. It cannot be restored in
  the meantime, and the page says so before anybody presses, not the failing run afterwards.
* The panel does not transfer a backup in the Drive (22.19): the link takes the place of the
  download.
* Every keystroke in the schedule sends a row, and 10.10 has limits for every field. An emptied
  field would be a `0` and every intermediate stage while typing a `400 invalid_schedule`. That is
  why values are clamped before they are sent.
* The frame holds the full operation state from the socket (13.5); an interval of its own next to
  it would be the same progress, only later. A finished operation changes a backup's size, state
  and quota, and the snapshot reports that at once; the queue itself only asks every 30 s.

---

## 9. Console and socket

* `seq` counts per server and never runs backwards (13.5). After a dropped connection the whole
  ring buffer comes again, so `console_history_start` resets the counter instead of discarding the
  repeat as a duplicate.
* Only the identifier is checked on a socket message. The shape of the fields is guaranteed by the
  contract; a validator per message would be a second truth next to section 14.
* The browser holds 25 000 lines or 8 MiB: **mclo.gs takes no more than that when sharing** (6.7),
  and holding more would mean tying memory up for something that never goes out. A server that
  writes 6 000 lines while a modpack starts fills that in four runs.
* Trimming goes not to the limit but 20 % below it: the layout redraws the whole terminal as soon
  as the array gets shorter. Dropping line by line would mean redrawing at every block; this way it
  happens once every 5 000 lines.
* The frame holds the console buffer, not the overview: the backlog comes exactly once per
  connection (13.2), and the connection belongs to the frame. Otherwise the console would be empty
  after every tab change: the socket would still stand, but the lines would have gone with the
  page.
* If the connection already stood when we subscribed, the backlog is over and does not come again.
  That is exactly what happens the second time the console tab is entered.
* Nothing more comes once it has given up, been refused or been closed with a final code (13.6).
  `4404` and `4429` set neither "given up" nor "refused"; without the close code the loading state
  would stand for ever. No close code means: before the first attempt. Three of these codes end the
  page, and a banner with a button would be a dead end then.
* The very first connect is not a reconnect: console and metrics already show their own loading
  state for it, a banner on top would only be a flash.
* The only lines the browser writes into the console itself are the ones about a loss — otherwise
  it would disappear without a sound.
* mclo.gs takes seconds. Whoever presses "Start" in that time has already cleared the box away; the
  answer then belongs to a run that no longer exists and must not pull it up again.
* Above an old log file the input field stays mute: the echo of a command (6.1) lands in the live
  buffer, which you are not looking at. And the list of files moves away under the user as soon as
  `latest.log` is hidden on start.

---

## 10. Files

* 7.3 delivers one page. An incomplete state is stale at once, so that the call reloads when the
  folder is entered instead of keeping the missing entries quiet, and afterwards it looks once
  more: if the user enters the folder while this one page is in flight, the complete list is
  already there and must not fall back to the first page.
* The abort signal is passed through: `listAll` pages through up to 20 000 entries, and whoever
  leaves the folder should not pay for the remaining pages any more.
* The cache lives only long enough for the prefetch on hover to count: a `latest.log` ages in
  seconds. That is why reading goes through `fetchQuery` and not through `ensureQueryData`, which
  would also hand back a state five minutes old.
* Downloads go through the browser and not into memory: a world file is too big for that, and the
  browser can resume a broken transfer over `Range` itself (7.7).
* Before every wait the target folder is captured. The user may navigate on while the `201` is in
  flight; without that the new entry lands in the cache of the folder that happens to be open, and
  the actual folder stays old. The same for uploads (7.8).
* Only your own handle is cleared away: with two overlapping calls a blind reset would take the
  cancel button from the second run.
* There is no "file has changed" event (7.10). The only moment the tree changes without our doing
  is the end of an unpack.
* The `revision` from 5.2 is not reconciled: on one connection the snapshots arrive in the order
  they came about in. The reconciliation would only be needed if we mixed 5.2 in as well.
* A linked file is read by the file page **itself** instead of letting the editor open: only there
  is it known that the file comes from a link and nobody picked it out. The editor would otherwise
  merely say "Could not load file contents", and whoever comes from the properties of a freshly
  created server would never have learned that its `server.properties` does not exist at all before
  the first start. It is read once on entry and not as a watcher, otherwise the editor opens again
  after being closed.
* The link there is `?path=/&editing=server.properties` — Modrinth's form. Without `editing` the
  same address means a **folder**; that is how the content page links to a plugin's config folder.
  `?editing=a&editing=b` arrives as an array, and the editor can show one file only. An absolute
  path in `editing` wins over the folder from `path`.

---

## 11. Content

### 11.1 Where a plugin puts its configuration

Modrinth does not decide that, the plugin itself does, and it only creates the configuration when
the server runs for the first time. That is why `providers/config-location.ts` guesses nothing but
compares with what 7.3 really lists in the config folder.

Measured on a Paper 1.21.1 after the first start, twelve plugins: ten rows find their own folder
this way. `PAPIProxyBridge` creates none, and `VeinMiner Enchantment` writes into the folder
`Veinminer` of its sibling project — both get the containing folder and no false promise. A
comparison on substrings would have caught those two, but would have sent `Chunky` into
`ChunkyBorder`; the two of them lay side by side there.

On top of that four rules, each of which prevents one wrong guess:

* A jar is never configuration: otherwise every row would find itself, because it lies in exactly
  this folder. A disabled one is called `.jar.disabled` (8.4) and is just as little configuration.
* If only jars lie there, nothing has written yet: the folder is the one 8.7 installed into itself.
  Only what stands next to them proves a server that has run.
* **Unread is not empty.** A missing folder is an answer (then nothing lies there), every other
  error is not: without access or without a network the row stays without a button instead of
  claiming "there is nothing here"; that would be the same claim as "nothing found".
* Only on a file is the dot an extension; a folder may be called `.paper-remapped`.

There is **one call per folder** for the whole page, not one per row: 8.7 puts all content into
exactly one directory (`install.rs:directory_of`), so twelve rows become one request. And a plugin
creates its folder on the first run, which 13.5 does not report as a content change — without a
look at the run state the page would go on saying "no configuration yet" until the next page load,
even though it is there by now.

### 11.2 Searching and installing

* There is **no** tab bar over project types: 8.1 derives the type from the loader and 8.7 puts
  everything into exactly one directory. A data pack on a Fabric server landed in `mods/`.
* Exactly the server's loader is preselected, never its relatives. Modrinth's alias group
  `paper/purpur/spigot/bukkit` holds in one direction only: Paper runs a Bukkit plugin, a
  Purpur-only plugin it does not. The user can still unlock it and choose for themselves; for a
  loader without a Modrinth name (Leaf) the filter stays empty.
* A server plays nothing: what runs in the client only does not belong in the result list.
* Whether a project already lies there is decided by `project_id` from **our** row (8.1) and not by
  the map out of Modrinth's cache: otherwise an installed plugin would count as not installed as
  soon as Modrinth does not deliver its map. "Downloading" stands separately next to it, because it
  is something else: the row is created, the file is still loading. Both lock installing, but only
  one of the two is already there.
* Whatever has been installed in the meantime drops out of the selection — from a second window, by
  another member, or because it was the dependency of another project in the same selection. 8.7
  would reject it with `already_installed`; the bar at the bottom should not offer what is already
  there.
* One request for the whole selection, not one per project: `install_content` locks the server
  (5.8), two clicks in a row would give a `409`.
* On the search page the installed list serves the badges only. Its error does not color the page:
  the page stays usable, the badges are simply missing. If nothing is scheduled, nothing is loaded
  either — returning early would look like success.
* 8.6 knows no `409 server_running`: the file is replaced at once, but the running process holds
  the old one until somebody restarts. Without the hint about that the update would look
  ineffective. The modpack does not stand in `items` but in a field of its own with its own
  `has_update`; without it the hint would be missing exactly when the modpack alone is out of
  date.
* An upload keeps running after the tab is left (7.8), but its display does not: the progress
  therefore hangs on the server context and not on the page, otherwise the frame's banner would
  stay up because nobody updates it any more — or the cancel button would disappear mid-run.
* A rejected unknown file drops out of the selection, the rest go up all the same. Otherwise one
  "no" costs the whole selection.
* An operation counts as given up after ten minutes without progress (8.6). While the socket is not
  open, 5.3 is asked at a five-second interval; if the component disappears, both the listening and
  the polling end; otherwise both run on for up to ten minutes and the listener hangs on a socket
  that outlives the page. Changes (enable, delete, upload), by contrast, deliberately run to the
  end.
* Across family boundaries the backend insists on `wipe_mods` (9.14); if the platform stays, it is
  the game version change 8.14: only there is there `incompatible_content`, and the new build has
  to travel along, otherwise every Paper change would fall back to 9.14, where the content stays
  untouched. No silent bail-out on save: otherwise the editing closes and the page claims to have
  saved.
* Choose first, then lock: if the user cancels the file dialog, not every browser reports the
  `cancel` event, and the lock would stand for ever.

---

## 12. The server that was meant to be deleted

15.1 has no `notices` here; the notices above a server come out of its state. The special case is
4.5: **a delete that fails leaves the server on `deleting` and brings it back into the list.**
"Being deleted" would be a lie then: nothing is being deleted any more, the space stays taken, and
the way there is the same button under settings. That is why the reason from the failed operation
stands there.

The **running** operation beats the failed one here: on the second attempt the first run still
stands next to it, and "could not be deleted. Delete again" on a server that is disappearing right
now would be the same lie with the sign reversed. The same question decides `IN_VIEW` in the panel
about whether the server stands in the list at all: is a delete running?

Dismissing means "read", not "never happened" (5.5): the run stays in the snapshot as long as the
server stands on `deleting` (`ops/store.rs`, `STANDING_FAILURE`), and `dismissed_at` must therefore
change nothing here.

---

## 13. Memory, limits and numbers

* Memory stands in MiB everywhere in the panel (9.3), but in gigabytes on the slider: "4 GB" is the
  number a player thinks in, "4096 MiB" the one the core computes in. It is shown with one decimal
  place so that 11776 MiB reads as 11.5 GB and not as 11; the upper end is **rounded down**,
  because the slider runs in whole steps: at "2.5 GB" that would be an end the knob never reaches.
  The server's value comes back untouched, otherwise the slider would already stand at "changed"
  when the page opens.
* Without the machine numbers from `GET /admin/host` there is no honest ceiling. An account **with**
  a budget is measured against its budget; the machine only raises the ceiling because an admin may
  create beyond that budget for this account (4.2). An account **without** a budget — today every
  panel admin (12.7) — has the machine only, and if that call fails it may still create: then a
  fallback ceiling stands there, marked as one. Without that branch an admin faced a slider without
  an upper bound and a button that never released; and without a budget "no memory left" flashed up
  before the answer, which was wrong. Only an admin may read 12.1 at all — for everybody else the
  number never exists, and waiting for it would be the same dead end.
* For memory the account page counts what is **allocated** and not what is measured: the budget is
  the permission, and a server that starts today starts tomorrow too. For disk it is the other way
  round: there what really lies on it decides. Only what this account pays for is counted; 4.1
  also lists shared servers.
* `limit === 0` does not occur on the wire, but a division by it would be `Infinity` and the bar
  therefore an empty box instead of a warning. Without a limit there is no bar at all: a share of
  nothing is not a number, and for an account without limits (12.7) there is no form either.
* The share is clamped, the number itself is not.
* A password stands there exactly once (12.3): after the dialog is closed it is no longer in the
  page's memory either. There is deliberately no input field for it: the control is the place
  where you can copy it down.

---

## 14. The phone is the narrow side

`composables/breakpoint.ts` keeps Tailwind's `md` (768 px) in one place, so that pages do not each
invent the number for themselves. Above it a table fits side by side, below it it does not, and
"below" here does **not mean "show less", it means the same thing stacked.**

Measured, each at 390 px device width:

| Place | measurement | consequence |
|---|---|---|
| `pages/account/Account.vue` | two margins on top of each other took 32 px out of 390 | the page sets no margin of its own, `AppShell` already gives it (`main px-6 py-6`) |
| `pages/admin/Users.vue` | seven columns are 72 rem wide; a quarter was visible | below `md` the row carries name and menu, everything else stands underneath |
| `pages/servers/settings/Network.vue` | 28 rem: 40 % name, action column at its own width, 4 rem for the port | below that the table is swiped sideways instead of getting four buttons cut off in 33 % of a phone's width |
| `components/table-widths.ts` | 3 rem are 48 px, 16 px of which go to the padding — the button lost 4 px | the width of an action column is computed from the buttons that really stand in it |

`components/table-widths.ts` computes with the measurements of the vendored building blocks:
`h-9 w-9` for an `IconButton` at `md`, `px-2.5` around a `size-5` icon for a button with a
screen-reader-only label, the cell's `last:pr-4`, `gap-2` between two buttons and `ring-4` at
`focus-visible`: the cell is `overflow-hidden` and would otherwise cut the focus ring off. A
column set by eye alone costs half the tap target. In the test the measurements stand in pixels, so
that it does not repeat the same rem arithmetic it is checking.

On top of that, the same everywhere:

* A label next to an icon only from `md` up; below that the column carries exactly the button, and
  "Actions" would stand there as "A…". What the menu does, it says when it is tapped.
* A button with an icon **and** a screen-reader-only label is wider than a plain icon button — that
  belongs in the column width.
* The padding sits on the link, not on the row: on a phone the name is the tap target, and a thumb
  does not reliably hit 24 px of text height.
* Below `md` there is room for the error message too, which gets cut off in the cell above.
* There are no tooltips. Every reason stands in the body.

Two further measurements concern Modrinth's own building blocks and therefore stand in
`docs/INTERFACE-PARITY.md`, section 7.

---

## 15. The clipboard without a secure context

`navigator.clipboard` exists in a secure context only. Whoever uses the panel over its LAN address
(`http://192.168.…`) does not have it, and every copy button calls `writeText` without a fallback,
eight of them in the vendored code alone. The click throws, nothing is copied, and nobody learns
why; the operator tripped over this.

The replacement therefore hangs on the one place they all share (`main.ts`) and not on every
caller. Four things about it are deliberate:

1. It is installed only when the real one is missing.
2. `document.execCommand('copy')` copies the *selection*, so it needs a field that carries the
   text. It stands outside the viewport but inside the tree: `display: none` cannot be selected,
   and a field in view would make the page jump.
3. Selection and focus belong to the user again afterwards: otherwise they lose the caret
   mid-typing to a field they never saw.
4. If the replacement fails too, it **rejects and does not resolve**: `CopyCode.vue` sets its
   checkmark only after the `await`, and a checkmark over an empty clipboard is a lie.
   `AppShell.vue` installs the reporter for it, because copy buttons also stand on the account
   page, the administration page and the server list; without it there would be nothing but silence.

---

## 16. Small things that have a reason

* **One theme, one class.** `.oled-mode` and `.retro-mode` build on `.dark-mode` in
  `variables.scss` via `@extend`; the compiled output enters them into the same selector list. So
  exactly one class belongs on the `<html>`. A browser with site storage blocked throws on access:
  then the mode holds for this session only, because `startTheme()` runs before `createApp()` and
  the panel must not fail on it.
* **`'failed'` and not `'error'`:** six checks write `state.startsWith('fail')`.
* **A search field over what is loaded** needs big pages: the table's free-text search works on
  what is already there (11.9). While reloading, what is loaded stays; an error card replaces an
  empty page only.
* **We have no friends list and no user page.** Whoever is found counts as known — the "Also send a
  friend request" checkbox stays off — and `profilePath` stays empty, because the link would lead
  nowhere.
* **Whoever deletes the last entry on a page** would otherwise face "no accounts": the pagination
  goes back one page then.
* **Password, link and `PATCH` on the same user must not overtake each other.**
* **The dialog after 21.4 stays open:** there is nothing to copy down, and the sentence "on its way
  to this address" belongs where the button stood. The link goes to the address **of the account**,
  not to the one in the form: 21.4 has no body. Whoever types and then sends would otherwise get
  the mail at the old address and would never notice.
* **A reloaded tab picks up a running sign-in flow; a closed dialog does not**: the dialog *is*
  the flow, whoever closes it cancels it (8.4, 22.4).
* **Two toggles that cannot do anything wrong need no validation.**
* **A row that was just approved or rejected disappears from the list** (20.6, 20.7). Without this
  removal it would stand there until the next load, and a second click would give a `404`. Verified
  ones stand at the top, because only they can be approved, and the addresses per sender are
  counted over **all** rows, so that the number also stands on a row that looks alone (20.5).
* **The menu item for limits is missing on an account without limits** (12.7): the dialog is not
  there for that one.
* **`?section=` in the address:** without a section `NavTabs` would find no active tab, and typed
  nonsense would stay mutely on "General".
* **A proxy reads no `server.properties`** (9.11): the tab would have nothing to show.
* **Reset stands in the danger zone under "General"**: a second button would be a second way.
* **The record and the socket belong to the frame.** A second read in a tab would stand still after
  every installation, because only the WS message `server` brings the state along. Likewise the
  server header fetches the tunnel from the same source as the network section; two fetch paths
  could drift apart.
* **9.10 returns the new primary port in the answer**, the WS message only updates the record
  object afterwards. Without that precedence the old port would stand in one breath as "Primary"
  and as an ordinary assignment in the same table — with the same `row-key`.
* **4.5, 9.16 and 9.17 require a server at rest.** `isServerRunning` covers `running` only; whoever
  clicks during `starting` or `stopping` otherwise ends up with the `409`. And only a panel admin
  may change the startup command: greyed out it would be a button you press and that says nothing;
  a sentence stands there in its place. A message from 9.4 belongs to the save that triggered it;
  the `GET` does not know it, and a sentence left standing names a number the server has long since
  stopped having.
* **A tunnel is a hole onto the primary port of the moment it came about.** Where it points lies
  with playit and cannot be moved from here; so its own number is shown and not the one that is
  primary today. And switching over is locked as soon as the row stands in `playit_tunnels` at all:
  18.7 calls only its absence `none`, every other state, `offline`, `missing` and `failed`
  included, means "the row stands". A button that locks on `online` only would release exactly when
  that helps least.

---

## 17. What is still open

* The eleven playit calls from `api/playit.ts` belong in `api/client.ts` (1.2).
* Loaders from later waves stay out of the selection while the installer step is missing (9.11).
* `shown` in `layouts/ServerFrame.vue` is the place where a tab would be hidden if a permission does
  fall away one day. Today every tab stands on `BASE_READ`, and without that bit the page does not
  exist at all; what a viewer may not do is locked by the building blocks themselves.
