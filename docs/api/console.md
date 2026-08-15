# Interface: console

As of 2026-08-12. The "Console" area of the overview page. Source references `path:line` are
relative to `/root/MinecraftServerManager/` (vendor) or to the reference clone `/root/ref-modrinth/`.

The layout to be driven is `vendor/modrinth/ui/src/layouts/shared/console/layout.vue`. It is
embedded **unchanged**. Everything written here is derived from it.

Two models, both read:

- `vendor/modrinth/ui/src/layouts/wrapped/hosting/manage/overview.vue:122-162` — Modrinth's
  server page, live-only, without a log file picker.
- `/root/ref-modrinth/apps/app-frontend/src/pages/instance/logs/index.vue:146-165` — the
  desktop app, with a log file picker and without command input.

Our console is the union of the two: command input **and** a log file picker.

---

## 1. The provider contract

`vendor/modrinth/ui/src/layouts/shared/console/providers/console-manager.ts:8-34`. Every field
one by one, with where the layout uses it and where the value comes from.

| # | Field (source) | Required per the type | Required in truth | Where the value comes from |
|---|---|---|---|---|
| 1 | `logLines: Ref<LogLine[]>` (`console-manager.ts:9`) | yes | yes | Our console state in the browser, filled from the WS messages `console_history_start` / `console` / `console_history_end` (section 3). With a file selected, from `GET …/console/logs/content` instead. **Has to be a `shallowRef` that is appended to in place, with `triggerRef` called afterwards** — reasoning below the table. |
| 2 | `logSources?: ComputedRef<LogSource[]>` (`:11`) | no | **yes, as soon as `onDelete` is served** (`layout.vue:249`) | Index 0 is an invented live source `{ id: "live", name: "Live console", live: true }`, then the response of `GET …/console/logs`. That is exactly how the desktop app builds the list (`logs/index.vue:51-66`). |
| 3 | `activeLogSourceIndex?: Ref<number>` (`:12`) | no | yes, when `logSources` is set | Pure client state. No endpoint. The layout **writes** into it (`layout.vue:31`), so it has to be a writable `ref`, not a `computed`. The combobox appears only when `logSources` **and** `activeLogSourceIndex` are set (`layout.vue:26`). |
| 4 | `sendCommand?: (cmd: string) => void` (`:14`) | no | yes (otherwise the input line has no effect) | `POST /api/v1/servers/{id}/console/command`. The return is `void`, so the layout cannot evaluate an error (`layout.vue:386-389`) — our provider produces the error messages itself through `injectNotificationManager`. |
| 5 | `showCommandInput?: boolean \| Ref<boolean>` (`:15`) | no | yes | Constant `true`, as in `overview.vue:132`. Without it the input line is not rendered at all (`layout.vue:208-213`, `BaseTerminal.vue:21`). |
| 6 | `disableCommandInput?: …` (`:16`) | no | yes | `computed(() => !can_exec_commands || power_state !== 'running')`, as in `overview.vue:133-135`. `can_exec_commands` from the server context (access area), `power_state` from the WS message `state` (overview area). |
| 7 | `disableCommandInputTooltip?: …` (`:17`) | no | no | The text "No permission", only when the lock is down to the permission (`overview.vue:136-138`). Careful: the layout also derives the placeholder text from it — with a tooltip "Command input disabled", without a tooltip "Server is not running" (`layout.vue:239-241`). So for "server is not running" set **no** tooltip. |
| 8 | `loading?: Ref<boolean>` (`:19`) | no | yes | `true` for as long as the WebSocket is not connected or the history between `console_history_start` and `console_history_end` is running. See section 3; to this day the layout has no clean edges for it (`layout.vue:226`: *"needs historical log start/end flags on ws to be properly useful"*) — we supply them. |
| 9 | `onClear?: () => void` (`:21`) | no | yes | `POST /api/v1/servers/{id}/console/clear`. The layout clears the terminal itself and calls `onClear` afterwards (`layout.vue:391-398`); our call touches only the **server buffer**. |
| 10 | `clearDisabled?: Ref<boolean>` (`:22`) | no | no | `!can_exec_commands`, as in `overview.vue:154`. |
| 11 | `clearDisabledTooltip?: …` (`:23`) | no | no | A text, only when `clearDisabled` (`layout.vue:263-265`). |
| 12 | `onDelete?: () => Promise<void>` (`:24`) | no | only with `logSources` | `DELETE /api/v1/servers/{id}/console/logs?file=…`. **Important:** the delete button exists only when a non-live source is selected (`layout.vue:249`). Errors have to be thrown as a **string** — the layout shows `typeof err === 'string' ? err : 'Unknown error.'` (`layout.vue:414-417`). An `Error` object yields "Unknown error." |
| 13 | `deleteDisabled?: Ref<boolean>` (`:25`) | no | no | `!can_write_files` or "`latest.log` and the server is running" — the latter exactly as in `logs/index.vue:130-134`. |
| 14 | `deleteDisabledTooltip?: string` (`:26`) | no | no | **A plain string only**, no Ref — the layout passes it through unwrapped (`layout.vue:52`), unlike every other tooltip. Fixed text: "Cannot delete latest.log while the server is running". |
| 15 | `shareDisabled?: Ref<boolean>` (`:28`) | no | yes | `!ws_connected` (as in `overview.vue:158`) **or** an administrator has switched outbound services off. That is our only lever against the share button: it is always rendered as soon as there are lines (`ConsoleActionButtons.vue:26`). |
| 16 | `emptyStateType?: 'server' \| 'instance'` (`:30`) | no | yes | Constant `'server'` (`overview.vue:159`). Steers the empty state with the frog (`BaseTerminal.vue:82-95`). Without the value the terminal stays completely empty in the empty state (`BaseTerminal.vue:133`). |
| 17 | `crashAnalysis?: Ref<InsightsResponse \| null>` (`:32`) | no | yes | The response of `POST /api/v1/servers/{id}/console/crash-analysis`, **verbatim** the mclo.gs structure. Only `analysis.problems[].message` and `analysis.problems[].solutions[].message` are consumed (`layout.vue:136-147`). |
| 18 | `onDismissCrash?: () => void` (`:34`) | no | yes, when `crashAnalysis` is set | Pure client state, no endpoint: `crashAnalysis.value = null` plus a marker in `localStorage` for 30 minutes, as in `overview.vue:92-95,117-120`. |

### How `logLines` has to be carried forward

Otherwise the terminal stays empty or gets slow. The layout watches with
`watch(ctx.logLines, …)` **without `deep`** (`layout.vue:328`). Two traps follow from that:

- An ordinary `ref([])` whose array you `push` into **never** triggers the watcher: the identity
  of `.value` does not change. The console would stay empty.
- An array that is **replaced** per batch (`logLines.value = [...old, ...new]`) does trigger, but
  then `lines !== oldLines` (`layout.vue:343-347`) applies and the layout redraws the **whole**
  terminal every time (`rewriteTerminal`, `console-filtering.ts:195-221`) — with 25,000 lines
  every 100 ms.

The right way is the one both models take: `shallowRef`, keep the same array, append,
`triggerRef` (`composables/server-console.ts:222,275-290`; `logs/index.vue:105-109`). Only then
does the fast path `layout.vue:353-370` run, which writes the new lines and nothing else.
Both measured against the Vue in this tree (3.5.41): `ref([])` plus `push` triggers **zero**
passes, `shallowRef` plus `push` plus `triggerRef` exactly one, and `new === old`.

### What else the layout needs

These four injections run in the layout **without a fallback** and throw otherwise
(`create-context.ts:66`):

- `injectModrinthClient()` (`layout.vue:130`) — needed for mclo.gs, see below.
- `injectModalBehavior()` (`layout.vue:131`)
- `injectNotificationManager()` (`layout.vue:133`)
- `injectPageContext(null)` (`layout.vue:132`) has a fallback and may be missing.

### No `world_id`

The whole console area knows no `world_id` (checked: no hit in
`layouts/shared/console/`). No constant needed.

### What the interface can do itself — and what we therefore do not do

`detectLogLevel` (`console/composables/log-level.ts:5-14`) determines the level purely from the
line text: `/INFO` or `[System] [CHAT]` → info, `/WARN` → warn, `/DEBUG` → debug,
`/TRACE` → trace, then the error triggers `/ERROR`, `Exception:`, `:?]`, `Error`, `[thread`,
`\tat` → error. On top of that, a continuation line without a timestamp inherits the level of its
parent entry (`composables/server-console.ts:345-361`).

**It follows that the server detects no levels.** We send raw text. A `level` field on the wire
would be duplicated work, and LogLine only knows `text` and `level` anyway
(`console/types.ts:3-6`): there is no field for a timestamp of our own.

**It follows, secondly, that the start of the line is sacred.** Two places check
`/^\[\d{2}:\d{2}:\d{2}\]/` — the grouping of continuation lines
(`composables/server-console.ts:14`) and the block highlighting of errors
(`console/composables/log-highlight-addon.ts:24`). So we must put **nothing** in front of the
Minecraft line start (no ISO timestamp, no stream prefix), or the rendering of stack traces falls
apart.

For the lines the panel produces itself, the same shape applies:

```
[15:04:22] [Panel/INFO]: > say hello
[15:04:25] [Panel/INFO]: Server process started (pid 21044)
[15:07:01] [Panel/ERROR]: Server process exited with code 1
```

`[Panel/INFO]` contains `/INFO` → level info; `[Panel/ERROR]` contains `/ERROR` → level error
(`log-level.ts:3,6`). The time is the local time of the panel machine, so that it lines up with
the Minecraft lines.

---

## 2. The endpoints

Six of them. Everything under `/api/v1/`, the session cookie, errors uniformly
`{ "error": "<code>", "message": "<text>" }`.

The following also applies to every endpoint, without being repeated below:

| Status | `error` | when |
|---|---|---|
| 401 | `unauthenticated` | no session cookie, or an expired one |
| 404 | `server_not_found` | id unknown **or** the user does not even have `BASE_READ` on this server (no difference on the outside) |

The permission bits are Modrinth's names, which we adopt per the plan (P6):
`BASE_READ`, `EXEC_COMMANDS`, `FILES_WRITE` (`composables/server-permissions.ts:15-32`).

### 2.1 Send a command

```
POST /api/v1/servers/{server_id}/console/command
```

Permission: `EXEC_COMMANDS`.

Request:

```json
{
  "command": "say hello"
}
```

Response: `204 No Content`, no body.

The command is **not** returned in the response. It comes back over the WebSocket as an ordinary
console line, to everyone connected including the sender:

```
[15:04:22] [Panel/INFO]: > say hello
```

That way there is exactly one path by which lines enter the buffer, and no double display.
Without this echo the user would face an empty field for commands that produce no output — the
input field clears itself at once (`BaseTerminal.vue:237-243`).

Error cases:

| Status | `error` | Meaning |
|---|---|---|
| 403 | `forbidden` | no `EXEC_COMMANDS` |
| 409 | `server_not_running` | no running process, stdin is closed |
| 422 | `command_empty` | empty or only whitespace (the layout already trims, `BaseTerminal.vue:240-241`) |
| 422 | `command_too_long` | over 8192 bytes of UTF-8 |
| 422 | `command_invalid` | contains `\n`, `\r` or a control character — otherwise you could smuggle several commands into one |
| 429 | `rate_limited` | more than 20 commands in 10 seconds per user and server |

```json
{ "error": "server_not_running", "message": "The server is not running." }
```

The command belongs in the audit log (P6), as `console_command_executed` with
`{ "command": "say hello" }` — the name and the mandatory field are in the catalog of the access
area (`docs/api/auth.md`; renderer `ConsoleEvent`, `access/events/parser.ts:255`).

### 2.2 Clear the console (server buffer)

```
POST /api/v1/servers/{server_id}/console/clear
```

Permission: `EXEC_COMMANDS`.

Request: no body. Response: `204 No Content`.

Effect: the ring buffer in memory is emptied and `{"type":"console_cleared"}` is sent to everyone
connected. **`logs/latest.log` is not touched.** The counter `seq` keeps running
(section 3). Audit log: `console_cleared`, without metadata (`docs/api/auth.md`,
`access/events/parser.ts:37`).

The difference from `onDelete`, taken from the code:

| | `onClear` | `onDelete` |
|---|---|---|
| Button visible | only for the live source and when there are lines (`layout.vue:43`, `ConsoleActionButtons.vue:4`) | only for a **non**-live source and when `onDelete` is set (`layout.vue:249`) |
| Confirmation | none | modal "Delete log file", "This is irreversible" (`layout.vue:75-93`) |
| Display | the layout clears it itself (`layout.vue:393-396`) | none |
| Server | empties the live buffer (`overview.vue:145-153`, `logs/index.vue:152-155`) | deletes a file (`logs/index.vue:136-144`) |

| Status | `error` | Meaning |
|---|---|---|
| 403 | `forbidden` | no `EXEC_COMMANDS` |

### 2.3 Crash analysis

```
POST /api/v1/servers/{server_id}/console/crash-analysis
```

Permission: `BASE_READ`.

Request (body optional):

```json
{
  "source": "latest_log"
}
```

`source` is `"latest_log"` (the default) or `"buffer"`. Both variants exist in the models: the
server page reads `/logs/latest.log` (`overview.vue:101-106`), the desktop app
analyzes the live buffer (`logs/index.vue:113-126`).

Response `200`: the response of `POST https://api.mclo.gs/1/analyse`, **cut down to the fields of
`InsightsResponse`** (`api-client/src/modules/mclogs/types.ts:35-42`), so that it fits into
`crashAnalysis` without reshaping:

```json
{
  "id": "vanilla/server",
  "name": "Vanilla",
  "type": "Server Log",
  "version": null,
  "title": "Vanilla Server Log",
  "analysis": {
    "problems": [
      {
        "message": "You are using an outdated Java version.",
        "counter": 1,
        "entry": {
          "level": 6,
          "time": null,
          "prefix": "[15:04:22] [main/ERROR]:",
          "lines": [
            { "number": 42, "content": "java.lang.UnsupportedClassVersionError" }
          ]
        },
        "solutions": [
          { "message": "Update to Java 21 or newer." }
        ]
      }
    ],
    "information": []
  }
}
```

The header fields are not called what you would guess. Measured against the real API on
2026-08-12: `id` is the **kind** of log (`"vanilla/server"`, `"unknown/unknown"`), not a storage
id; `name` is the detected loader (`"Vanilla"`); `type` is `"Server Log"`; `title` is both
together. **`name` and `version` come back as `null`** as soon as mclo.gs does not detect the
loader or the version — for an unknown log, both. Modrinth's type declares `string` there
(`mclogs/types.ts:36,39`); that is too narrow, and a Rust struct with `String` fails on the very
first real response. For us: `Option<String>` and `string | null` respectively
(section 4).

**Two fields are cut away**, `success: true` and `entries`. `entries` contains **every** parsed
log entry — measured: 202 entries and 33 KB of JSON for a 20 KB log. Passed through untrimmed,
the response would be as big as the file we were trying not to send through the browser in the
first place, and the cache would hold megabyte-sized chunks per server.
`InsightsResponse` does not know `entries` anyway, and the layout reads only
`analysis.problems[].message` and `.solutions[].message` (`layout.vue:136-147`) — so cutting it
down costs nothing and is not a reshaping of the structure but the omission of two keys.

`analysis.problems` may be empty; the provider then sets `crashAnalysis` to `null`, exactly as
`overview.vue:107-110` does. That is the **normal case**: mclo.gs recognizes only a limited number
of known patterns — for "FAILED TO BIND TO PORT", "Could not reserve enough space for object heap"
and a missing EULA, an empty list came back in each case on 2026-08-12. So the red box appears
rarely, and that is right.

**Why through our backend and not from the browser?** Because otherwise the browser would first
have to download `latest.log` only to upload it again straight away — with a modded server that
is quickly double-digit megabytes over two legs instead of one. The backend already has the file,
trims it to the last 2 MiB at a line boundary and sends only that on.
The trim is harmless: mclo.gs shortens to 10 MiB and 25,000 lines itself
(`GET https://api.mclo.gs/1/limits`, checked on 2026-08-12:
`{"success":true,"storageTime":7776000,"maxLength":10485760,"maxLines":25000}`).

But that only holds for the **way out**. On the way back, without the cutting of `entries`,
almost the same volume would come out again, and in the end the browser would have loaded the
whole log after all — only wrapped in JSON. Only both together, trimming out and cutting back,
turn double-digit megabytes into a few kilobytes.

Cache: 10 minutes, with a different key per source — for `latest_log` (`server_id`, mtime + the
length of the file), for `buffer` (`server_id`, the `seq` of the last buffer line + the line
count). A file timestamp does not exist for the buffer. mclo.gs throttles per IP,
and with us all users of one panel come from **one** IP, and that is why the cache is mandatory and
not optional. The exact limit is not in `/1/limits` and is not measured; the responses carry no
`X-RateLimit` headers. On a `429` from mclo.gs we pass on
`429 upstream_rate_limited` and remember the lock per panel for 60 seconds instead of
knocking again.

Trigger: our provider calls the endpoint when `power_state` jumps to `crashed`, exactly as
`overview.vue:164-177` does. The state `crashed` belongs to the overview area; we only hang
ourselves onto it.

| Status | `error` | Meaning |
|---|---|---|
| 404 | `log_file_missing` | `logs/latest.log` does not exist (`source: "latest_log"`) |
| 409 | `console_buffer_empty` | `source: "buffer"` and the ring buffer is empty |
| 409 | `external_services_disabled` | an administrator has switched outbound calls off |
| 502 | `upstream_error` | mclo.gs does not answer, or answers with an error |
| 429 | `upstream_rate_limited` | mclo.gs throttled (60/min/IP) |

```json
{ "error": "external_services_disabled", "message": "Crash analysis is disabled by the administrator." }
```

### 2.4 List log files

```
GET /api/v1/servers/{server_id}/console/logs
```

Permission: `BASE_READ`.

`GET /api/v1/servers/{server_id}/console/logs?limit=200&offset=0` — `limit` is 1…500, the default
is **200**, `offset` defaults to 0.

Response `200`:

```json
{
  "total": 214,
  "truncated": true,
  "files": [
    {
      "file": "logs/latest.log",
      "name": "latest.log",
      "kind": "log",
      "size_bytes": 184320,
      "modified_at": "2026-08-12T13:04:22Z",
      "compressed": false
    },
    {
      "file": "logs/2026-08-11-2.log.gz",
      "name": "2026-08-11-2.log.gz",
      "kind": "log",
      "size_bytes": 20481,
      "modified_at": "2026-08-11T22:59:10Z",
      "compressed": true
    },
    {
      "file": "crash-reports/crash-2026-08-11_22.58.59-server.txt",
      "name": "crash-2026-08-11_22.58.59-server.txt",
      "kind": "crash_report",
      "size_bytes": 40961,
      "modified_at": "2026-08-11T22:58:59Z",
      "compressed": false
    }
  ]
}
```

Sorting: `modified_at` descending, `logs/latest.log` always first. Only what ends in `.log`,
`.log.gz` or `.txt` inside `logs/` and what ends in `.txt` inside `crash-reports/` is taken up.
Subdirectories and symbolic links are skipped. The filter matches the one in the
desktop app (`logs/index.vue:54-61`).

**Why there is pagination at all, although the interface cannot paginate.** A
Minecraft server rotates `latest.log` away on **every start** and additionally at midnight
(`2026-08-11-1.log.gz`, `-2`, …), and nobody cleans that up; crash reports come on top. After
a year with a daily restart those are four-digit numbers. The combobox builds **every** option
into the DOM (`Combobox.vue:122-127`, no virtualization) and gets the list raw as
`logSources` — without a cap the picker becomes unusable and the response megabyte-sized.
Hence: sort, **then** cap. `total` is the number before the cap, `truncated` is
`offset + files.length < total`. The provider shows the first page; whoever needs older files
takes the file manager. Without pagination this would be a list that grows with the runtime of
the server and never shrinks again.

`logs/latest.log` is always shipped along; whether it turns up in the picker is the provider's
decision — as long as the server is running, the desktop app hides it, because the live console
shows the same thing (`logs/index.vue:89-93`). We copy that.

### 2.5 Read a log file

```
GET /api/v1/servers/{server_id}/console/logs/content?file=logs/2026-08-11-2.log.gz
```

Permission: `BASE_READ`.

Response `200`:

```json
{
  "file": "logs/2026-08-11-2.log.gz",
  "size_bytes": 20481,
  "content_bytes": 184320,
  "truncated": false,
  "content": "[22:41:03] [Server thread/INFO]: Starting minecraft server version 1.21.4\n[22:41:09] [Server thread/INFO]: Done (6.213s)! For help, type \"help\"\n"
}
```

`size_bytes` is the file on disk, `content_bytes` the length of `content` — with `.gz`
those are two very different numbers, hence both.

`content` is the unpacked plain text; `.gz` is unpacked on the server, the caller notices
nothing of it. At most the **last 25,000 lines or 8 MiB** are delivered, whichever bites
first; then `truncated: true` is set — the cut is always at the **front**, the end is the
interesting part. The 25,000 are no accident: that is exactly how many lines mclo.gs accepts, and
the share button sends the displayed content there (section 2.7). So nothing can be cut off at
the top without a word.

**Unpacking needs a ceiling.** To get the *last* lines of a `.gz`, the stream has to be run all
the way through. But `logs/` does not only hold what the server writes there — every file can get
there through the file manager, and 200 KB of gzip can expand into gigabytes.
So: unpacking is **streamed** into a rolling window of the last 25,000 lines or 8 MiB,
and at **512 MiB** of unpacked bytes the operation aborts with `413 log_too_large`. The
files area handles unpacking archives the same way (`docs/api/files.md`, E8,
`archive_too_large`).

**Paths as in the files area.** `file` is normalized by the rules from `docs/api/files.md` 2.1
(N1–N7) and opened through `openat2(root_fd, …, RESOLVE_BENEATH|RESOLVE_NO_MAGICLINKS)`.
A leading `/` is therefore **allowed** and means nothing — `logs/latest.log` and
`/logs/latest.log` are the same file. Reinventing everything else here would mean spelling the
same file differently depending on the endpoint. One rule applies only here: the normalized path
has to start with `logs/` or `crash-reports/` and must not lie one directory deeper.

| Status | `error` | Meaning |
|---|---|---|
| 400 | `invalid_path` | normalization fails (`files.md` N2, N3, N5) or `file` does not point directly into `logs/` or `crash-reports/` |
| 403 | `forbidden_path` | resolution leaves the server root — a link pointing outside, a magic link |
| 404 | `log_not_found` | the file does not exist |
| 413 | `log_too_large` | the unpack ceiling was exceeded |
| 422 | `log_not_text` | the file is not readable as UTF-8 (only isolated gibberish is replaced lossily) |

### 2.6 Delete a log file

```
DELETE /api/v1/servers/{server_id}/console/logs?file=logs/2026-08-11-2.log.gz
```

Permission: `FILES_WRITE`. Deleting is a file access, not a console right: a viewer may read
logs but not clear them away.

Response: `204 No Content`.

The path model is as in 2.5. Deletion goes through `unlinkat` on the parent descriptor, so a link
is deleted as a link and never its target (`files.md` 2.1, point 6).

| Status | `error` | Meaning |
|---|---|---|
| 400 | `invalid_path` | as in 2.5 |
| 403 | `forbidden` | no `FILES_WRITE` |
| 403 | `forbidden_path` | as in 2.5 |
| 404 | `log_not_found` | the file does not exist |
| 409 | `log_file_in_use` | `logs/latest.log` and the server is running |
| 409 | `server_busy` | a blocking file operation is running (`files.md` 2.4) |

The message in the 409 case has to be usable, because it lands in front of the user unchanged
when the provider rethrows it as a string (`layout.vue:414-417`).

This goes into the audit log as `file_deleted` with `{ "path": "logs/…" }` — the name from the
catalog of the access area (`docs/api/auth.md`, event `file_deleted`, renderer `FileEvent`).
A console name of our own for it would be "Unknown event", because `parseAuditEvent` only renders
known names.

### 2.7 No endpoint: sharing to mclo.gs

Sharing runs **straight from the browser**, and not because we want it that way but because
the layout hard-wires it: `handleShare` builds the text from `ctx.logLines` and calls
`client.mclogs.logs_v1.create(content)` (`layout.vue:422-443`). That module has
`api: 'https://api.mclo.gs'` and `skipAuth: true` fixed in the code
(`api-client/src/modules/mclogs/logs/v1.ts:10-17`). To route that through our backend you would
have to change `layout.vue`, and that is exactly what is ruled out.

Three consequences:

1. **The log file does not have to be fetched for it.** The content is already in memory as
   `logLines`; in fact only what search and the level filter currently leave over is shared
   (`layout.vue:423-425`). A backend endpoint would have nothing to do here.
2. **We have to provide a `ModrinthClient`**, otherwise the injection alone throws
   (`layout.vue:130`, `create-context.ts:66`) — even though we use nothing else from Modrinth's
   API.
3. **Our buffer size in the browser is the size limit.** mclo.gs takes 10 MiB and 25,000
   lines and shortens anything beyond that in silence (values above, checked live). So we limit
   our console state to **25,000 lines and 8 MiB** instead of the 500,000 lines that
   Modrinth's own state holds (`composables/server-console.ts:12`). Then nothing that the user
   can still see is ever cut off.

If the panel machine has no way out, or an administrator has switched outbound services off,
we set `shareDisabled` (field 15). The button cannot be hidden.

---

## 3. WebSocket messages

One socket per server, `/api/v1/servers/{server_id}/ws`, shared with the other areas. The
console area uses four message types, **all from the server to the client**. In this area the
client sends nothing; the command goes over HTTP (reasoning in section 5).

Connection setup (`BASE_READ` assumed, checked when the socket is set up — the overview
area):

```
→ {"type":"console_history_start","total_lines":8421,"dropped_lines":0}
→ {"type":"console","seq":0,"lines":["[13:00:01] [main/INFO]: Starting…", "…"]}
→ {"type":"console","seq":500,"lines":["…"]}
   … further blocks …
→ {"type":"console_history_end"}
→ {"type":"console","seq":8421,"lines":["[15:04:22] [Server thread/INFO]: Done (6.213s)!"]}
```

### `console_history_start`

```json
{ "type": "console_history_start", "total_lines": 8421, "dropped_lines": 0 }
```

The first message after the connection is set up, **before** any live line. The provider sets
`loading = true` and clears its state. That way there are no duplicates after a connection drop.
`dropped_lines` is the number of lines pushed out of the ring buffer since the last clear; if
anything other than 0 is there, the history is incomplete.

With that we solve exactly what Modrinth noted in the layout as a shortcoming
(`layout.vue:226`) and today guesses at with two timers (700 ms of quiet, a 2000 ms cap,
`composables/server-console.ts:55-56`).

**The race between the replay and live output has to be won by the server, not by chance.**
While the 8,421 lines go out in 17 blocks, the server process keeps writing. Whoever registers
the listener first and then reads out the buffer delivers the same line twice; whoever reads out
first and then registers loses the lines in between; and in both cases `seq` gets muddled.
Binding, therefore: on setup, **under the same lock**, the end position of the ring buffer is
recorded and the listener is registered at exactly that point. What comes after that goes into
the queue of this connection and goes out **behind** `console_history_end` — never in between.
`seq` thereby stays gapless and ascending over the whole connection, and the check
`seq + lines.length` on the client is a real loss indicator instead of a false alarm.

### `console`

```json
{ "type": "console", "seq": 8421, "lines": ["[15:04:22] [Server thread/INFO]: Done (6.213s)!"] }
```

Carries both the history and the live output — the difference lies solely in the bracketing by
`console_history_start` / `console_history_end`.

- `lines` are complete lines **without** a line break, in arrival order. stdout and
  stderr are merged; no stream marker is sent along, because the interface throws it away anyway
  (`composables/server-manage-core-runtime.ts:212-216` does not evaluate `stream` from
  `WSLogEvent`).
- `seq` is the running number of the **first** line in the array. It counts **per server**, not
  per server process, and is **never** reset — neither by `console_cleared` nor by the start of a
  new server process. On a panel start it begins at 0 (the seed from
  `logs/latest.log` gets the first numbers); where exactly it starts is irrelevant, because it is
  only compared within one connection. The next expected number is
  `seq + lines.length`, and a jump means lines were lost. A jump backwards must not occur — a
  counter per process would produce exactly that on a restart and turn the loss indicator of a
  browser that stayed connected into noise.
- Block size: up to 500 lines or 64 KiB, then a new message. Live output is batched every 100 ms,
  so that not every line becomes a frame of its own.

Rules for the producer, all derived from section 1:

1. Complete lines only. An incomplete remainder is held until `\n` arrives — but delivered after
   250 ms at the latest, otherwise a prompt without a line break would stay
   invisible.
2. `\r\n` and `\r` become nothing, a leading BOM is removed.
3. ANSI sequences and control characters other than tab are removed. Reason: the layout colors
   the line itself by wrapping the whole line in SGR codes (`console-filtering.ts:8-20`); an
   embedded `\x1b[0m` from the server process would end the coloring halfway through.
4. Lines over 8 KiB are cut to 8 KiB, with ` [truncated]` appended.
5. No prefix in front of the Minecraft timestamp (see section 1).

### `console_history_end`

```json
{ "type": "console_history_end" }
```

The provider writes everything it buffered into `logLines` in one go and sets `loading = false`.
The layout then redraws once completely (`layout.vue:380-384`) instead of many times.

### `console_cleared`

```json
{ "type": "console_cleared" }
```

The server buffer was emptied — through `POST …/console/clear` by another user or through the
start of a server process. The provider clears its state; the layout notices that itself
and shows the empty state again (`layout.vue:332-341`).

On the **start** of a server process we empty the buffer and send this message. The
desktop app does the same (`logs/index.vue:206-211`: on `launched` → `liveConsole.clear()`),
and it keeps the output of two runs from running into each other with no visible seam. `seq`
keeps running while it does (see above) — the seam is the message, not a jump in the counter.

If the message arrives **during** the replay (somebody clears while we are still sending), it
still applies at once: the provider clears its state, discards everything received so far and
treats the following blocks as a new beginning. `console_history_end` comes afterwards as planned
and ends `loading`.

### The history: ring buffer and file

**A ring buffer in memory, with the file as a seed.**

| | Value | Reasoning |
|---|---|---|
| Ring buffer per server | **10,000 lines or 4 MiB**, whichever bites first | Starting a large modpack produces 3,000–6,000 lines depending on its size; 10,000 hold the complete start plus the most recent runtime. At around 100 bytes per line that is ~1 MB per running server, ~10 MB for ten servers — nothing compared with the JVMs next to it. The byte limit catches the case where a mod spits out JSON line by line. |
| Initial transfer | the complete buffer, in blocks of 500 | ~1 MB of JSON, just under a second on a LAN. 500 rather than 256, so that a block stays in the same ballpark as Modrinth's `initialBatchSize` (`composables/server-console.ts:54`) and the number of frames stays small. **Not** because large blocks would pass straight through on the client: `addLines` takes the direct path only while `output.value.length === 0` (`server-console.ts:313-317`) — so at most for the first block. With us our own state decides anyway, not Modrinth's: between `console_history_start` and `_end` we collect and write once. |
| Buffer in the browser | 25,000 lines or 8 MiB | The browser keeps collecting beyond the initial transfer. The limit comes from mclo.gs (section 2.7), not from memory. |
| Seed from the file | the last 10,000 lines or 4 MiB from the **end** of `logs/latest.log` | It kicks in exactly when the ring buffer is empty: the panel has just started, or the server is down. Without it the console of a stopped server shows nothing — and the crash message would sit above an empty terminal. The lines that are read land in the ring buffer, so there is only one delivery path. |

No HTTP endpoint for the history: it comes over the socket the page opens anyway. For that, the
console state in the browser has to live across pages (Modrinth does that with
`createGlobalState`, `composables/server-console.ts:418`), otherwise everything is lost when you
switch to "Files" and back.

---

## 4. Data types

To be taken over verbatim, `web/src/api/console.ts`.

```ts
// ---- WebSocket, Server -> Client -------------------------------------------

export interface ConsoleHistoryStartMessage {
	type: 'console_history_start'
	total_lines: number
	dropped_lines: number
}

export interface ConsoleLinesMessage {
	type: 'console'
	seq: number
	lines: string[]
}

export interface ConsoleHistoryEndMessage {
	type: 'console_history_end'
}

export interface ConsoleClearedMessage {
	type: 'console_cleared'
}

export type ConsoleSocketMessage =
	| ConsoleHistoryStartMessage
	| ConsoleLinesMessage
	| ConsoleHistoryEndMessage
	| ConsoleClearedMessage

// ---- HTTP ------------------------------------------------------------------

export interface SendCommandRequest {
	command: string
}

export type CrashAnalysisSource = 'latest_log' | 'buffer'

export interface CrashAnalysisRequest {
	source?: CrashAnalysisSource
}

/**
 * The response of POST https://api.mclo.gs/1/analyse, cut down by `success` and
 * `entries` (section 2.3). Structurally identical to Mclogs.Insights.v1.InsightsResponse
 * (api-client/src/modules/mclogs/types.ts:35-42) — deliberately duplicated so that our
 * type file does not depend on @modrinth/api-client.
 *
 * A deviation from Modrinth's type, and a deliberate one: `name` and `version` are
 * nullable. The real API returns `null` there as soon as it does not detect the loader
 * or the version (measured on 2026-08-12). Modrinth's `string` is simply wrong; at
 * runtime it does not show over there, because both fields stay unread.
 */
export interface CrashAnalysisResponse {
	id: string
	name: string | null
	type: string
	version: string | null
	title: string
	analysis: {
		problems: Array<{
			message: string
			counter: number
			entry: {
				level: number
				time: string | null
				prefix: string
				lines: Array<{ number: number; content: string }>
			}
			solutions: Array<{ message: string }>
		}>
		information: Array<{
			message: string
			counter: number
			label: string
			value: string
			entry: {
				level: number
				time: string | null
				prefix: string
				lines: Array<{ number: number; content: string }>
			}
		}>
	}
}

export type LogFileKind = 'log' | 'crash_report'

export interface LogFile {
	file: string
	name: string
	kind: LogFileKind
	size_bytes: number
	modified_at: string
	compressed: boolean
}

export interface LogFileListResponse {
	total: number
	truncated: boolean
	files: LogFile[]
}

export interface LogFileContentResponse {
	file: string
	size_bytes: number // file on disk
	content_bytes: number // length of content; a different number for .gz
	truncated: boolean // cut at the front
	content: string
}

export interface ApiError {
	error: string
	message: string
}

// ---- Limits ----------------------------------------------------------------

export const CONSOLE_SERVER_BUFFER_LINES = 10_000
export const CONSOLE_SERVER_BUFFER_BYTES = 4 * 1024 * 1024
export const CONSOLE_CLIENT_BUFFER_LINES = 25_000 // = mclo.gs maxLines
export const CONSOLE_CLIENT_BUFFER_BYTES = 8 * 1024 * 1024 // < mclo.gs maxLength (10 MiB)
export const CONSOLE_HISTORY_CHUNK_LINES = 500
export const CONSOLE_MAX_LINE_BYTES = 8192
export const CONSOLE_MAX_COMMAND_BYTES = 8192
export const LOG_LIST_DEFAULT_LIMIT = 200
export const LOG_LIST_MAX_LIMIT = 500
export const LOG_GUNZIP_MAX_BYTES = 512 * 1024 * 1024
```

Lines produced on the server, so that everyone uses the same shape:

```ts
export const PANEL_LINE_TAG = 'Panel'
// [HH:MM:SS] [Panel/INFO]: <text>   -> level info   (log-level.ts:6)
// [HH:MM:SS] [Panel/WARN]: <text>   -> level warn   (log-level.ts:7)
// [HH:MM:SS] [Panel/ERROR]: <text>  -> level error  (log-level.ts:3,10-12)
```

---

## 5. Open questions and assumptions

### Decided

**The command over HTTP instead of over the socket.** Modrinth sends it over the socket
(`overview.vue:127`, `WSCommandMessage` in `api-client/src/modules/archon/types.ts:1193-1196`).
We take HTTP, because then permission, state and length errors come back as a clean status code
with a stable `error` code; the contract returns `void` (`console-manager.ts:14`), so the
layout cannot evaluate an error anyway, and our provider needs an evaluable response for the
notification. The price: two commands sent in quick succession can in theory swap order.
With commands typed by hand that does not matter; the backend writes to stdin under a mutex.

> **A contradiction with the overview area that has to be resolved.** `docs/api/servers.md:717`
> writes: "The only inbound message in the whole socket is `command` from the console area" —
> and `servers.md` 3.1 introduces a WS message `error` specifically for it, whose example reads
> verbatim `missing permission EXEC_COMMANDS`. This document decides the opposite: **no**
> client→server traffic, the command over HTTP. Both cannot hold. Whoever looks after
> `servers.md` deletes that sentence there and checks whether the `error` message still has a
> sender without the command. Equally out of date: `servers.md:753` names `log` and `log4j` as
> console messages; here they are called `console*` and there is no `log4j` (our servers
> deliver raw text on stdout, not a Log4j stream).

**Level detection stays in the client.** Reasoned in section 1 with `log-level.ts:5-14`. We
send no `level`, no timestamp of our own and no `stream`.

**Log sources are coming, but only after P1 — and they are an extension of the plan.** This has
to stand in the open: Modrinth's own server interface has **no** picker for log files
(`overview.vue:122-162` supplies neither `logSources` nor `onDelete`), and `PLAN.md:88` names
only "filter levels, mclo.gs sharing and crash analysis" for the console. The file picker comes
from the desktop app. That puts three of the six endpoints (2.4–2.6) beyond the model; whoever
understands feature parity as the limit has to allow that explicitly or strike it. Without them
the console remains fully usable. The interface does not need them — without `logSources` it
simply shows no picker (`layout.vue:26`) and reports the live source as active
(`layout.vue:172-177`); Modrinth's own server page does exactly that
(`overview.vue:122-162`). We serve them anyway, for three reasons: (a) without them
`onDelete` is an unreachable, dead part of the contract (`layout.vue:249`); (b) our ring buffer
dies with the panel process, and everything before it would be unreachable in the console;
(c) the file manager can open logs in the editor and even share them to mclo.gs
(`files-tab/components/FileNavbar.vue:196-204, 404-410`), but it cannot unpack `.log.gz` and it
colors and filters nothing. For P1 ("the console records, the input line sends
commands") 2.4–2.6 are not needed.

**`console/logs/*` deliberately duplicates the file manager.** In theory the three endpoints
could run over the general file endpoints. Against that: they unpack `.gz`, they trim at the end
instead of at the beginning, they know only two directories, and they deliver a list with `kind`
and `compressed` that exists nowhere else. Modrinth's desktop app splits the same way
(`helpers/logs.js:19-49` next to the file manager).

**No `after_seq` resume.** After a connection drop the whole buffer comes again.
That is a deliberate omission in favor of a socket without client state; Modrinth does the same
(`composables/server-manage-core-runtime.ts:326-327`: on connect first `clear()`, then fill
again). `seq` is in the message anyway, so that a later resume is not a break in format.

### Assumptions somebody has to confirm

1. ~~**`crashed` comes from the overview area.**~~ **Confirmed.** `docs/api/servers.md` 3.1 sends
   `{"type":"state","power_state":…}` with `crashed` in the value range and states explicitly
   that `crashed` triggers the crash analysis (`servers.md:480`). Worth noting for us: after
   `kill` the overview area deliberately reports `stopped`, not `crashed` (`servers.md:479`) —
   so a server the user shot down triggers no analysis. Right as it is.
2. **`BASE_READ` is enough to open the socket.** Confirmed in `servers.md` 2.8; the
   close codes 4401/4403 belong there too. The console area checks nothing further on
   the message itself.
3. ~~**mclo.gs is reachable from the browser.**~~ **Measured on 2026-08-12, with one
   condition.** `POST https://api.mclo.gs/1/log` and `/1/analyse` with a foreign `Origin` answer
   `access-control-allow-origin: *`. The **`OPTIONS` preflight, by contrast, answers `200` with
   no `access-control-*` headers at all** — so a preflighted call would be dead. It works only
   because `URLSearchParams` plus `application/x-www-form-urlencoded` is a *simple*
   CORS request (`mclogs/logs/v1.ts:14-15`). From that follows a condition on the
   `ModrinthClient` we provide: **configure no headers of your own.**
   `config.headers` flows through `buildDefaultHeaders()` into *every* request
   (`api-client/src/core/abstract-client.ts:367-371`), including the ones to mclo.gs; a single
   extra header forces the preflight and the share button dies. (`User-Agent` is
   harmless — the browser does not allow it in the first place.)
4. **The line estimate of ~100 bytes** comes from eyeballing typical Vanilla and
   Paper lines, not from a measurement. It carries only the memory arithmetic, not the format.

### Somebody else has to decide

1. **A switch for "outbound services"** (mclo.gs) in the panel settings — the settings/admin
   area. I need it for `409 external_services_disabled` and for
   `shareDisabled`. Proposal: default on, an administrator can turn it off.
2. **Does a server process survive a panel restart?** The plan does not say so explicitly. I
   assume: no — hence the seed from `logs/latest.log`. If it does survive, the
   ring buffer also has to be kept up to date from a cursor into the file, the way the desktop
   app does it with `get_latest_log_cursor` (`helpers/logs.js:62-64`).
3. ~~**Roles onto bits** (P6)~~ **Done, and it matches.** `docs/api/auth.md` 1.2 keeps
   `EXEC_COMMANDS` and justifies it with our contract precisely
   ("we wire it ourselves through `ConsoleManagerContext.disableCommandInput`"), and `FILES_WRITE`
   stays the write right on files. So we do **not** adopt Modrinth's bundling onto
   `POWER_ACTIONS` (`overview.vue:125,154` via `canUsePowerActions`,
   `composables/server-permissions.ts:92`). One small thing remains for the access area: which
   server role gets `EXEC_COMMANDS` — a viewer may not send commands and may not clear the
   buffer.
4. ~~**The audit log**~~ **Done except for one confirmation.** The catalog in `docs/api/auth.md`
   has `console_command_executed` with the mandatory field `{ command: string }` (renderer
   `ConsoleEvent`, `parser.ts:255-259`) and `console_cleared` without metadata — exactly the two
   events this area produces. Deleting a log file gets **no** name of its own, but
   `file_deleted` with `{ path }` (2.6): names outside the catalog are shown by the display as
   "Unknown event". Reading and sharing are not logged.
