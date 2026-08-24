# Files — interface contract

As of 2026-08-12. Area "Files" (phase P2 per `docs/PLAN.md:435`).

What is served is `FilePageLayout` from `vendor/modrinth/ui/src/layouts/shared/files-tab/layout.vue`,
unchanged, through the contract
`vendor/modrinth/ui/src/layouts/shared/files-tab/providers/file-manager.ts:13`.

Source references are relative to `vendor/modrinth/ui/src/` (Modrinth's library) and
`/root/ref-modrinth/` (reference clone of the desktop app). Every statement about the behavior of
the interface has a line reference.

One exception you have to know: **`layouts/wrapped/…` is not vendored here**
(`docs/PLAN.md:76`). Line references into it — they only show how Modrinth itself fills the
contract — apply against `/root/ref-modrinth/packages/ui/src/`. Everything under `layouts/shared/`,
`components/`, `composables/`, `providers/` and `utils/` is line for line congruent
with the reference clone (checked with `diff -r`).

---

## 1. The provider contract

### 1.1 How the layout builds paths — before the field table makes sense

The layout assembles paths itself, from `currentPath` and `item.name`, and hands them back into the
contract methods. The forms that come out are **not uniform**; our API has to accept
all of them.

| Place | Result | Source |
|---|---|---|
| Start | `currentPath` is whatever the provider sets | `layout.vue:310` only reads it |
| Breadcrumb click | `'/'` at index −1, otherwise `'plugins/config'` — **without** a leading slash | `layout.vue:391` |
| Open a folder | `currentPath.endsWith('/') ? currentPath+name : currentPath+'/'+name` | `layout.vue:405-411` |
| Rename | `` `${currentPath}/${item.name}`.replace('//','/') `` | `layout.vue:434` |
| Move (dialog) | source as above, target `` `${destination}/${item.name}` ``, `destination` with a `/` forced in front | `layout.vue:446-455`, `components/modals/FileMoveItemModal.vue:93-95` |
| Move (drag) | source is **`item.path` unchanged**, target is `` `${targetFolder.path}/${name}` `` | `layout.vue:474-484`, `composables/file-drag-state.ts:32-37` |
| Delete (single) | `` `${currentPath}/${item.name}`.replace('//','/') ``, `recursive = type === 'directory'` | `layout.vue:470-471` |
| Delete (batch) | **`item.path` unchanged** | `layout.vue:579-583` |
| Open the editor | `item.path`, normalized to a leading `/` in the editor | `layout.vue:415`, `components/editor/FileEditor.vue:174` |
| Save | `props.file.path`, likewise normalized to a leading `/` | `components/editor/FileEditor.vue:246` |
| Extract | `item.path` unchanged | `layout.vue:506`, `layout.vue:533` |
| Prefetch a file | `item.path` unchanged | `layout.vue:651` |
| Prefetch a folder | as in "open a folder" | `layout.vue:643-647` |

Two binding decisions follow from this:

**(a) The API accepts `path` with and without a leading slash.** `""`, `"/"`, `"plugins"`
and `"/plugins"` name the same thing. Without that tolerance breadcrumb navigation breaks
(`layout.vue:391` delivers without, `FileMoveItemModal.vue:94` delivers with).

**(b) The API delivers `FileItem.path` **with** a leading slash** (`/plugins/config.yml`).
Reason: `prefetchFile` puts the cache under `item.path` (`files.vue:164`, the
query key), `readFile` reads it back out under the path normalized to `/`
(`files.vue:343-344`). Without a leading slash in `path` the prefetch never hits. On top of that
the navigation path built from `currentPath` and `name` (`layout.vue:405-411`) then matches
`item.path` of the subfolder exactly, so the folder cache hits too.

**(c) Exactly one place does not tolerate the leading slash — `editingFile.path`.**
`FileNavbar.vue:404-410` decides on the "Share to mclo.gs" button with
`editingFilePath?.startsWith('logs')`, `startsWith('crash-reports')` and `endsWith('.log')` —
all three meant **without** a leading slash. With `/logs/latest.log` the first two
conditions are dead; a crash report `crash-reports/crash-…-server.txt` does not end in `.log` and
loses the button with nothing in its place. So the provider stores the path in `startEditing`
**without** a leading slash:

```ts
startEditing: (f) => (editingFile.value = { name: f.name, path: f.path.replace(/^\//, '') })
```

That costs nothing: the editor normalizes back to `/` on reading and writing anyway
(`FileEditor.vue:174`, `:246`), and the prefetch cache hangs off `item.path`, not off
`editingFile.path`. The mirror in `?editing=` gets shorter that way too.

### 1.2 `FileManagerContext`, field by field

Order as in `providers/file-manager.ts:13-67`. The column "used" means: does the layout actually
read the field, not just the declaration (working rule 3).

| Field | Declaration | used at | Where the value comes from |
|---|---|---|---|
| `items: Ref<FileItem[]>` | `:14` | `layout.vue:305`, filtered/sorted in `composables/file-search.ts:9`, `composables/file-sorting.ts:26` | **E2** `GET /files/list`, assign the field `items` **unchanged**. No rebuilding needed, see 1.3. |
| `loading: Ref<boolean>` | `:15` | `layout.vue:39` (only covers the table while `items` is empty) | Loading state of the E2 request. |
| `error: Ref<Error \| null>` | `:16` | `layout.vue:122`, `layout.vue:136` | Error of the E2 request. Watch out: the interface shows a **fixed** text ("Unable to load files / The folder may not exist.", `layout.vue:258-265`) — our error code is lost there. |
| `currentPath: Ref<string>` | `:18` | `layout.vue:310`, `:393`, `:407`, `:434`, `:451`, `:470`, `:643` | State of the provider itself; we mirror it into the URL (`?path=`). Start value `"/"`. |
| `navigateTo(path)` | `:19` | `layout.vue:402`, `:410` | Sets `currentPath`, triggers E2. No endpoint. |
| `editingFile: Ref<EditingFile \| null>` | `:21` | `layout.vue:44-46`, `:148`, `:306` | Provider state, mirrored into `?editing=`. Path **without** a leading `/`, see 1.1 (c). No endpoint. |
| `startEditing(file)` | `:22` | `layout.vue:415` | Provider state. |
| `stopEditing()` | `:23` | `layout.vue:399`, `:420` | Provider state. |
| `createItem(name, type)` | `:25` | `layout.vue:426` | **E3** `POST /files/create` with `path = currentPath + '/' + name`. |
| `renameItem(path, newName)` | `:26` | `layout.vue:435`, also from undo/redo `composables/file-undo-redo.ts:32,64` | **E4** `POST /files/move`, target = parent directory of `path` + `newName`. An endpoint of its own is unnecessary; Kyros does it the same way (`api-client/src/modules/kyros/files/v0.ts:191-194`). |
| `moveItem(source, destination)` | `:27` | `layout.vue:455`, `:484`, `file-undo-redo.ts:26,58` | **E4** `POST /files/move`. `destination` is the **complete target path including the file name**, not the target folder (`layout.vue:453`). |
| `deleteItem(path, recursive)` | `:28` | `layout.vue:471`, `:582` | **E5** `DELETE /files`. |
| `readFile(path): Promise<string>` | `:30` | `FileEditor.vue:182` | **E6** `GET /files/content` with `max_bytes`, read the answer as text. |
| `readFileAsBlob(path): Promise<Blob>` | `:31` | `FileEditor.vue:177` (images only, `utils/file-extensions.ts:74`) | **E6** without `download`, answer as a blob. |
| `writeFile(path, content)` | `:32` | `FileEditor.vue:247` | **E7** `PUT /files/content?on_conflict=overwrite`. |
| `downloadFile(path, fileName)` | `:33` | `layout.vue:498` | **E6** with `download=1`. Recommendation: point an anchor at the URL instead of building a blob in memory — a 2 GiB world file must not go through the browser's heap. The price: the anchor does not know the status, so a 404 lands on disk as a file with JSON in it. That is why Modrinth builds a blob (`files.vue:361-380`) and pays for it with memory. (The session cookie is **no** argument for the anchor: a same-origin `fetch` sends it just as well.) |
| `uploadFiles(files: File[])` | `:35` | `layout.vue:591` (drag and drop), `:608`/`:625` (file picker) | **E7**, one request per file, see 2.9. |
| `cancelUpload?()` | `:36` | **nowhere in the files tab.** The consumer is `components/servers/admonitions/ServerPanelAdmonitions.vue:351,416` through the *Servers* context (`providers/server-context.ts:67`) | Aborts the running `XMLHttpRequest` and empties the queue. No endpoint. |
| `uploadState?` | `:37` | likewise only through the server context: `components/servers/admonitions/UploadAdmonition.vue:60-66`, `ServerPanelAdmonitions.vue:223` | Filled purely on the client from the XHR progress, see 2.9. |
| `refresh()` | `:39` | `layout.vue:63`, `:140`, `file-undo-redo.ts:40,72` | Request E2 again. |
| `isBusy?` | `:41` | `layout.vue:307` → locks **every** write action (`:382`, `:425`, `:430`, `:447`, `:466`, `:480`, `:504`, `:530`, `:546`, `:552`, `:557`, `:563`, `:569`, `:575`, `:590`, `:603`) and switches the editor to read-only (`FileEditor.vue:149`, `:216`, `:243`) | `!can_write_files` OR a running file operation (E9/WS) OR a running installation/backup from other areas. See 2.4. |
| `busyTooltip?` | `:42` | `layout.vue:308`, passed on to buttons and rows (`:54`, `:106`, `:164`) | Finished display text; comes from the reason for `isBusy`, not from the API. |
| `busyWarning?` | `:43` | **no consumer.** Set in `layouts/wrapped/hosting/manage/files.vue:464`, read nowhere | **Gap, on purpose:** we do not deliver it. |
| `extractFile?(path, override, dry)` | `:45` | `layout.vue:506` (dry run), `:533` (for real); the entry in the context menu only for `.zip` **and** a function that is set (`layout.vue:675`), in the row menu only for `.zip` — there **without** a check on the function (`FileTableRow.vue:251`, `shown: isZip.value`). No consequence for us, because we do set it. | **E8** `POST /files/extract`. |
| `activeOperations?` | `:50` | **nowhere in the files tab.** Consumer: `ServerPanelAdmonitions.vue:234`, filled from the server context (`providers/server-context.ts:70`) | **WS** `file_ops` (section 3), first fill through **E9**. |
| `dismissOperation?(id, action)` | `:51` | likewise: `ServerPanelAdmonitions.vue:373,384`, `FileOperationAdmonition.vue:111` | **E10/E11**. |
| `prefetchDirectory?(path)` | `:53` | `layout.vue:647`, `:662` (150 ms hover delay) | E2 into the cache. |
| `prefetchFile?(path)` | `:54` | `layout.vue:651`, only for files the editor can open (`utils/file-extensions.ts:95`) | E6 into the cache. |
| `showInstallFromUrl?` | `:56` | `layout.vue:7`, `:51`, `components/FileNavbar.vue:151-169` | **`false` (leave it out).** Reason in 1.4. |
| `basePath?` | `:57` | `FileTableRow.vue:211`, `FileContextMenu.vue:127` → "Copy full path" | **E1** `GET /files/meta`, field `root_path`. |
| `openInFolder?(path)` | `:58` | `FileTableRow.vue:244-245`, `FileContextMenu.vue:32` — the entry appears only if it is set | **Leave it out.** Only meaningful in the desktop program (`/root/ref-modrinth/apps/app-frontend/src/pages/instance/files/index.vue:356`). |
| `downloadButtonLabel?` | `:60` | `layout.vue:33`, `FileTableRow.vue:88,273` | Leave it out → default text "Download". |
| `uploadingLabel?` | `:61` | **no consumer** (in the whole tree only the declaration and the value set by the desktop app) | Leave it out. |
| `canRestart?` / `restartServer?` | `:63-64` | **no consumer** in the files tab | Leave them out; restarting belongs in the "Servers" area anyway. |
| `canShareToMclogs?` | `:65` | **no consumer**; the button hangs on `FileNavbar.vue:404-410` alone (path starts with `logs`/`crash-reports` or ends in `.log`) — and therefore on `editingFile.path`, see 1.1 (c) | Leave it out. |
| `shareToMclogs?(content)` | `:66` | `FileEditor.vue:271-272` | **Leave it out** — then the built-in route `client.mclogs.logs_v1.create` takes over (`FileEditor.vue:277`), so a direct call from the browser to mclo.gs. A consequence somebody has to know: `FileEditor.vue:70` calls `injectModrinthClient()` **unconditionally**, and `providers/create-context.ts:66` throws without a provider. **Without a Modrinth client provided, the editor crashes when it opens.** |

### 1.3 The bound data types

`types.ts:1-10` — `FileItem`. Our answer from E2 has exactly these fields, so that
`items.value = response.items` is enough without any conversion.

| Field | Type | Source in the interface | How we fill it |
|---|---|---|---|
| `name` | `string` | display `FileTableRow.vue:31`, search `file-search.ts:13`, sorting `file-sorting.ts:69`, icon chosen by extension `FileTableRow.vue:288-300` | Last path segment, not the full path. |
| `type` | `'file' \| 'directory' \| 'symlink'` | navigation vs. editor `FileTableRow.vue:344-348`, folders first `file-sorting.ts:40-41`, the `recursive` decision `layout.vue:471` | `lstat`, without following the link. A link to a directory is `symlink`, not `directory` — otherwise deleting would set `recursive=true`. **A price you have to know:** see below. |
| `path` | `string` | see 1.1 | Relative to the root, **with** a leading `/`. |
| `modified` | `number` | `new Date(props.modified * 1000)` — `FileTableRow.vue:303` | **Unix seconds**, not RFC 3339. See 5.1. |
| `created` | `number` | `new Date(props.created * 1000)` — `FileTableRow.vue:308` | Unix seconds from `statx(STATX_BTIME)`; if the file system keeps no birth time: `0`, the way the desktop app does it (`/root/ref-modrinth/apps/app-frontend/src/pages/instance/files/index.vue:97`). |
| `size?` | `number` | `formatBytes(size)` `FileTableRow.vue:325`, sorting `file-sorting.ts:55` | Bytes, **only for `type === 'file'`**. |
| `count?` | `number` | "{count} items" `FileTableRow.vue:321`, sorting `file-sorting.ts:52` | Number of entries, **only for `type === 'directory'`**. Missing on a read error (the interface then shows "0 items"). |
| `target?` | `string` | **no consumer** (checked: only the type declaration) | We deliver the raw link content anyway, it costs nothing and it is in the bound type. |

**What `symlink` costs in the interface.** The row is shown but is dead: `selectItem`
knows only `directory` (navigate) and `file` (edit), so a click on a link does nothing at
all (`FileTableRow.vue:344-348`); the size column stays empty, because `size` is missing and `count`
is only read for `directory` (`FileTableRow.vue:319-326`); the menu entry "Download"
appears, because it only checks `type !== 'directory'` (`FileTableRow.vue:275`), but leads nowhere,
because `handleDownload` insists on `type === 'file'` (`layout.vue:497`). A symlinked
`logs` directory — not rare on servers — can therefore no longer be entered in the file manager.
We accept that: the alternative would be to report links as their target, and then a
`recursive=true` deletes the linked directory along with its contents. Whoever needs the links has
shell access (`docs/PLAN.md:97`).

`types.ts:12-15` — `EditingFile { name, path }`. Pure display state, comes from the clicked
`FileItem` (`layout.vue:415`), stored by the provider without a leading `/` (1.1 (c)). No API.

`types.ts:32-41` — `FileOperation`. Every field except `op` and `src` is optional; the interface
relies on more all the same:

| Field | expected by | How we fill it |
|---|---|---|
| `id?` | `ServerPanelAdmonitions.vue:236,238` (without `id` **not** dismissible), `FileOperationAdmonition.vue:26,110` | Always set, a ULID. |
| `op` | only as part of a key `ServerPanelAdmonitions.vue:236` | Fixed `"extract"`. The text reads "Extracting {src}" regardless (`FileOperationAdmonition.vue:99-107`) — so **no** other kinds of operation may show up here. |
| `src` | heading and the URL special case `FileOperationAdmonition.vue:95-96` | Path of the archive, with a leading `/`. |
| `state` | color/shape `FileOperationAdmonition.vue:3,7,94`, sorting and dismissibility `ServerPanelAdmonitions.vue:182-193,238` | `queued`, `ongoing`, `done`, `failed-*`. **Only the prefix `fail` is binding** — each of the six check sites is a `state?.startsWith('fail')`. Modrinth itself writes `failure-corrupted`/`failure-invalid-path` (`api-client/src/modules/archon/types.ts:1116-1121`); our `failed-*` satisfy the same check. A failure state without that prefix would stay up forever as a blue "Extracting …". See 3.2 on `cancelled`. |
| `progress?` | progress bar, clamped to 0…1 (`components/base/Admonition.vue:148`) | **0…1**, not percent. |
| `bytes_processed?` | "{size} extracted" `FileOperationAdmonition.vue:18` | Bytes extracted. |
| `files_processed?` | no consumer | Delivered anyway. |
| `current_file?` | "Current file: …" `FileOperationAdmonition.vue:22-24` | Path inside the archive. |

`types.ts:64-67` — `ExtractDryRunResult { modpack_name, conflicting_files }`.
`layout.vue:508` checks **only** `conflicting_files.length`; if the list is empty, the extraction
runs for real at once, otherwise `FileUploadConflictModal` appears. `modpack_name` has no consumer
here (the only one, `FileUploadZipUrlModal.vue:268`, is not mounted), we fill it anyway.

`api-client/src/types/upload.ts:93-100` — `UploadState`. All seven fields mandatory, all are
read (`UploadAdmonition.vue:8-16,62-66`). Purely on the client, see 2.9.

### 1.4 What we deliberately do not serve — and what that costs

1. **`showInstallFromUrl` stays off.** If it were `true`, `layout.vue:6-11` renders
   `FileUploadZipUrlModal`, and that component calls `injectModrinthClient()` in `setup`
   (`FileUploadZipUrlModal.vue:110,117`) and after that `client.kyros.files_v0.extractFile`
   (`:266,269`) — so Modrinth's hosting API, which we do not rebuild. On top of that it pulls in
   `InlineBackupCreator` (`:114`), so the backup area. With `false` the two menu entries
   "Upload from .zip URL" and "Install CurseForge pack" disappear without consequence
   (`FileNavbar.vue:151-169`, each `shown: showInstallFromUrl ?? false`).
2. **`openInFolder`, `downloadButtonLabel`, `uploadingLabel`, `canRestart`, `restartServer`,
   `canShareToMclogs`, `busyWarning`** are dropped (see table 1.2).
3. **No file watching.** There is no event "a file has changed". A running
   Minecraft server writes continuously; watching the whole tree would be expensive and the
   interface has no place to show it. Refreshing goes through the button
   (`FileNavbar.vue:112-126`), after every write and on state changes of a
   file operation (model: `layouts/wrapped/hosting/manage/files.vue:381-386`).
   **Watch out, this is a condition on the page, not on the contract:** the button hangs on the
   layout property `showRefreshButton` (`layout.vue:288-291` → `FileNavbar.vue:113`), not on the
   `FileManagerContext`. Mount `<FilePageLayout />` without `:show-refresh-button="true"` and you
   ship a file manager with no way at all to reload by hand.
4. **No directory download.** The entry appears only for non-directories
   (`FileTableRow.vue:275`, `layout.vue:696`).
5. **No SFTP** (`docs/PLAN.md:97`).
6. **`world_id` does not occur in the files area.** Checked: the contract and all components under
   `files-tab/` do not know it; it turns up only in Modrinth's wrapper layer
   (`layouts/wrapped/hosting/manage/files.vue:33`). For us it drops out with nothing in its place.

### 1.5 Who shows the errors — otherwise nobody does

At **none** of these seven places does the layout catch a rejected promise:

| Call | Place | Handling |
|---|---|---|
| `createItem` | `layout.vue:426` | `await` without `try` |
| `renameItem` | `layout.vue:435` | `await` without `try` |
| `moveItem` (dialog) | `layout.vue:455` | `await` without `try` |
| `moveItem` (drag) | `layout.vue:484` | `.then()` without `.catch()` |
| `deleteItem` (single) | `layout.vue:471` | not awaited at all |
| `deleteItem` (batch) | `layout.vue:582` | not awaited at all |
| `downloadFile` | `layout.vue:498` | `await` without `try` |

The only things caught are `extractFile` (`layout.vue:520-526`, `:535-541`) and everything in the
editor (`FileEditor.vue:186-193`, `:260-267`). `error: Ref<Error|null>` belongs to the listing
alone and shows fixed text anyway (1.2).

**Consequence:** of the 21 error codes from 2.2, **none** reaches the user without further work — a
409 `already_exists` when creating becomes a silent `unhandled rejection` in the console.
Binding rule for the provider: **every contract method catches for itself, reports through
`addNotification` and returns a resolved promise.** Never pass one through. Modrinth does it
the same way, only through the `onError` callbacks of its mutations
(`layouts/wrapped/hosting/manage/files.vue:311-318`).

A second requirement on the application follows from that, and it stands next to 5.8: **the
notification provider is mandatory.** `injectNotificationManager()` is called without a fallback
value — `layout.vue:293`, `FileTableRow.vue:142`, `FileContextMenu.vue:85`, `FileEditor.vue:68`
—, and `providers/create-context.ts:66` then throws. Unlike with the Modrinth client (5.8, only the
editor), here the **whole** files tab crashes on mounting.

---

## 2. The endpoints

Base: `/api/v1/servers/{server_id}/files`. `{server_id}` is a ULID.

### 2.1 Path model and confinement — applies to **every** endpoint

This is here and not in the implementation, because both sides need the same idea of
what a valid path is.

**Root.** Every server has exactly one directory:
`/var/lib/<panel>/users/<user-id>/servers/<server-id>` (`docs/PLAN.md:150-158`). All `path` values
are relative to it. There is no way to name anything outside it.

**Wire format.** POSIX, `/` as the only separator. A backslash `\` is an ordinary character in a
file name on Linux and is **not** treated as a separator. A leading `/` is
allowed and means nothing different from its absence. `""` and `"/"` are the root.

**Normalization, step by step.** The server carries it out, the provider may rely on it;
rebuild it on the client and you get the same results.

| No. | Rule | Violation → |
|---|---|---|
| N1 | Percent encoding is resolved **exactly once** (by the HTTP layer). `%252e%252e%252f` therefore becomes the file name `%2e%2e%2f`, not `../`. | — |
| N2 | No null byte (`0x00`) anywhere in the path. | 400 `invalid_path` |
| N3 | Valid UTF-8. | 400 `invalid_path` |
| N4 | Split at `/`; drop empty segments and `.`. `//a///b` → `a/b`. | — |
| N5 | A segment `..` is **not** resolved but **rejected**. `a/../b` too. | 400 `invalid_path` |
| N6 | Segment ≤ 255 bytes (`NAME_MAX`), whole relative path ≤ 4096 bytes (`PATH_MAX`), depth ≤ 64. | 400 `path_too_long` |
| N7 | The result is a list of segments. An empty list is the root. | — |

On N5: resolving `..` lexically is the classic trap: `a/../b` is *not* `b` exactly when
`a` is a symbolic link. We do not resolve, we reject. The interface
never produces `..` by itself; only the free text field in the move dialog
(`FileMoveItemModal.vue:22-27`) could deliver it, and there an error message is the right
answer.

**Enforcement in the file system.** Normalization alone is not enough; it says nothing about links.

1. When the server is created, a directory descriptor of the root is opened
   (`O_PATH|O_DIRECTORY`) and held. **No code ever assembles path strings.**
2. Every access goes through `openat2(root_fd, relpath, RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS)`.
   The kernel enforces the border without a race between checking and using. Linux ≥ 5.6; the
   binding to Linux is already recorded in the plan (`docs/PLAN.md:479-480`).
3. **`RESOLVE_BENEATH`, not `RESOLVE_IN_ROOT`.** `IN_ROOT` would silently bend a link to
   `/etc/passwd` onto `<root>/etc/passwd`; we want to see an error instead. A link
   that stays inside the root keeps working; one that leads out yields
   403 `forbidden_path`.
4. **`RESOLVE_NO_MAGICLINKS`** rules out `/proc/self/fd/…` and its relatives.
5. For **creating, writing, deleting and move targets** `O_NOFOLLOW` goes on the last
   segment. Otherwise a slipped-in link writes the file somewhere else. Inside the root
   that would be harmless, but the rule is cheaper than the case distinction.
6. **Deleting a link removes the link**, never its target (`unlinkat` without `AT_REMOVEDIR` on the
   parent descriptor).
7. **Only regular files** may be read, written and downloaded. FIFOs block
   on opening, device files deliver endlessly. Everything else: 400 `not_a_regular_file`.
8. Without `openat2` (kernel < 5.6) the walk goes segment by segment with `openat(O_NOFOLLOW|O_DIRECTORY)`.
   A behavioral difference that stays on the record: then **inner** links are not traversable either.
9. We do **not** set `RESOLVE_NO_XDEV`: a mount point of its own per server should stay possible.

**Why this is the most important paragraph here.** Through the group `craftpanel` the panel service
reaches **all** user directories and `panel.db` with the password hashes (`docs/PLAN.md:151-167`).
A plugin on user A's server runs as their system user and may write in their directory
— so it may also place a link. Without kernel-side confinement,

```
ln -s /var/lib/<panel>/panel.db  /var/lib/<panel>/users/<A>/servers/<S>/logs/latest.log
```

is enough, and one click on "Download" hands out the panel database. The same trick with
`/var/lib/<panel>/users/<B>/` hands out somebody else's server files. `RESOLVE_BENEATH` ends both.

**What an attacker tries.** Every line is a test case.

| Input | Result |
|---|---|
| `../../etc/passwd` | 400 `invalid_path` (N5) |
| `/etc/passwd` | becomes `<root>/etc/passwd` → 404 `not_found` |
| `%2e%2e%2f%2e%2e%2fetc/passwd` | after decoding once `../../etc/passwd` → N5 |
| `%252e%252e%252f` | file name `%2e%2e%2f` → 404, **no** second decoding |
| `....//....//etc` | segments `....`, `....`, `etc` — no `..`, so a valid name, only one that does not exist |
| `foo/..%00/bar` | 400 `invalid_path` (N2) |
| `..\..\windows` | one single segment with backslashes → 404 |
| `logs/latest.log` → link to `/etc/shadow` | 403 `forbidden_path` on opening |
| A link that is re-pointed between the check and the open | no gap: one single kernel resolution, no `stat`-then-`open` |
| `/proc/self/environ` through a magic link | 403 `forbidden_path` (`RESOLVE_NO_MAGICLINKS`) |
| Uploading under the name `../evil.jar` | 400 `invalid_path` (N5) |
| Moving to `/../../x` | 400 `invalid_path` |
| Renaming to `a/b` or `..` | 400 `invalid_name` (a name may not contain `/` and may not be `.`/`..`) |
| Moving a folder into itself (`/a` → `/a/b/a`) | 400 `invalid_move` |
| Archive entry `../../.ssh/authorized_keys` (Zip Slip) | the entry is skipped, the operation ends as `failed-path`, `invalid_path` in the log |
| Archive entry with an absolute path `/etc/cron.d/x` | as above |
| Archive entry that is a symbolic link | **not** created, skipped |
| Archive with 10 GiB of payload out of 2 MiB (zip bomb) | 400/abort `archive_too_large`, see E8 |
| Downloading a device file | 400 `not_a_regular_file` |
| A name of 300 bytes | 400 `path_too_long` |

**CSRF.** All modifying endpoints are `POST`, `PUT` or `DELETE`; with
`SameSite=Lax` the browser does not send the session cookie across sites. On top of that
the server checks the `Origin` header against its own origin on modifying requests. `GET`
never changes anything.

**Serving foreign content.** E6 delivers bytes uploaded by the user from the panel's own
origin. So **always** `Content-Disposition: attachment`,
`Content-Type: application/octet-stream` and `X-Content-Type-Options: nosniff`. Otherwise an
uploaded `.html` or `.svg` is a stored XSS against your own panel. The image preview
does not suffer: it fetches a blob and creates an object URL, and `<img>` recognizes the
format from the content (`components/editor/FileImageViewer.vue:11-21`).

### 2.2 Error format

Always `{ "error": "<code>", "message": "<text>" }` with a matching HTTP status. Codes of this
area:

| Code | Status | Meaning |
|---|---|---|
| `invalid_path` | 400 | N2, N3, N5 violated |
| `path_too_long` | 400 | N6 violated |
| `invalid_name` | 400 | name contains `/`, is empty, `.` or `..` |
| `invalid_move` | 400 | the target is inside the source directory, or the source is a prefix of the target |
| `not_a_regular_file` | 400 | FIFO, socket, device file |
| `not_a_directory` | 400 | listing on a file |
| `non_utf8_name` | 400 | an existing entry with a name that cannot be represented, see 5.4 |
| `forbidden_path` | 403 | the resolution leaves the server root |
| `permission_denied` | 403 | panel permission missing |
| `not_found` | 404 | the path does not exist |
| `parent_not_found` | 404 | the parent directory does not exist |
| `operation_not_found` | 404 | unknown operation ID |
| `already_exists` | 409 | the target exists and `on_conflict=fail` |
| `not_empty` | 409 | deleting a directory without `recursive` |
| `file_not_accessible` | 409 | `EACCES`/`EPERM`: the game process has closed this folder to the panel — not `io_error`, because nothing here is broken |
| `server_busy` | 409 | a locking operation is running (see 2.4) |
| `operation_not_cancelable` | 409 | the operation has already ended |
| `file_too_large` | 413 | `max_bytes` exceeded on reading or `max_upload_bytes` on writing |
| `unsupported_archive` | 415 | not a readable ZIP |
| `archive_too_large` | 413 | extraction limits exceeded |
| `no_space` | 507 | `ENOSPC`/`EDQUOT` |
| `io_error` | 500 | everything else |

### 2.3 Permissions

Two levels, named like Modrinth's bits (`composables/server-permissions.ts:17` and `:20`); the
final naming belongs to the "Access" area:

- **`files:read`** (corresponds to `BASE_READ`) — viewer and up: E1, E2, E6, E9, **E11**.
- **`files:write`** (corresponds to `FILES_WRITE`) — editor and up: E3, E4, E5, E7, E8, E10.

**Why E11 (dismissing) only demands `files:read`.** The interface shows the cross to everybody who
sees the notice: `ServerPanelAdmonitions.vue:238` checks only `op.id` and the end state, without a
permission; only the **cancel** button checks `canWriteFiles`
(`FileOperationAdmonition.vue:28-33`, `:110`). If E11 demanded write permission, a viewer would get
a 403 on every click and the notice would stay up — and because dismissing counts for everybody
on the server side, that goes for "dismiss all" too (`ServerPanelAdmonitions.vue:373`). Dismissing is
an acknowledgment, not an intervention.

The panel admin has both on all servers (`docs/PLAN.md:331-333`). If the permission is missing: 403
`permission_denied`. The interface also hides write actions in advance by having the
provider set `isBusy` (`layout.vue:307`), but the server checks independently of that.

### 2.4 `isBusy` — what it comes from

`isBusy = !has(files:write) || file_operation_running || installation_running || backup_running`

- `file_operation_running`: at least one operation from section 3 in `queued` or `ongoing`.
- `installation_running` (P3/P4) and `backup_running` (P5) come from other areas through the
  server context; as long as those do not exist, they are `false`.

Modrinth does **not** count extraction among the busy reasons (`server-manage-core-runtime.ts:108-126`
knows only installation and content sync). We do count it: while something is being extracted into
the same directory, renaming and deleting are a race. That is a decision of our own, see 5.2.

The server checks as well: while a file operation runs, E3, E4, E5, E7, E8 answer
409 `server_busy`. Relying on the interface alone is not enough: it has a second
browser tab.

---

### E1 — `GET /api/v1/servers/{server_id}/files/meta`

Root path and limits. The provider asks for it once per server; it fills `basePath` and gives
the upload and the editor their ceilings instead of hard-wiring them into the frontend.

Permission: `files:read`. No parameters.

**200**

```json
{
  "root_path": "/var/lib/mcsm/users/01J8Z0K7QF5N3T2V9RXG4H6MBD/servers/01J8Z0M2C4WQ7YB1PDE5S3FKAN",
  "max_upload_bytes": 4294967296,
  "max_text_bytes": 8388608,
  "max_page_size": 5000,
  "default_page_size": 1000,
  "max_extract_uncompressed_bytes": 21474836480,
  "max_extract_entries": 200000
}
```

`root_path` is the real path on disk; the plan presupposes shell access anyway
(`docs/PLAN.md:97`). It is used only for "Copy full path"
(`FileTableRow.vue:211,236`).

Errors: 403 `permission_denied`, 404 if the server does not exist.

---

### E2 — `GET /api/v1/servers/{server_id}/files/list`

Permission: `files:read`.

| Parameter | Type | Default | Note |
|---|---|---|---|
| `path` | string | `"/"` | see 2.1 |
| `after` | string | — | Name of the last entry of the previous page, exclusive. If it is missing, the listing starts at the front. |
| `page_size` | integer 1…`max_page_size` | `default_page_size` | |

There is **no** sorting on the server: the interface sorts and searches over the whole
array itself (`composables/file-sorting.ts:26-75`, `composables/file-search.ts:9-14`). A page is
therefore only a unit of transfer, not a unit of display. The transfer order is the byte order
of the name.

**Why a key and not a page number.** A running Minecraft server keeps writing while you page
through — `logs/`, `world/region/`, `crash-reports/`. With `page`/`page_size` every entry inserted
or deleted in page 1 shifts the border of all following pages: an entry
appears twice or not at all. A fixed sort order alone does **not** prevent that, it
only prevents the disorder. `after=<last name>` prevents it, because the border hangs on the
content and not on a position. The price is a restriction that does not hurt us: you
can only page forward, and that is exactly what the provider does.

**How many entries does the interface take?** The list is virtualized
(`layout.vue:345-354` → `composables/virtual-scroll.ts:134`, row height 61 px, buffer 5), so
only the visible rows are rendered. The bottleneck is not the DOM but the three
`computed` that run over the **whole** array on every keystroke (search, sorting,
selection). Modrinth fetches one single page with `page_size = 2000` and never pages on
(`layouts/wrapped/hosting/manage/files.vue:132`). We are more honest: the provider fetches pages
as long as `has_more` holds, up to at most **20,000** entries. Beyond that the contract has no
field to show "truncated"; that is a named gap (5.5).

**That does not apply to prefetching.** `prefetchDirectory` fires after only 150 ms of hovering
over a row (`layout.vue:642-648`). If it were allowed to page on to 20,000 entries too, a
mouse pointer brushing over `world/` would set off twenty requests in a row that nobody
asked for. So prefetching fetches **exactly one page**; whoever really enters the folder
fetches the rest.

**200**

```json
{
  "path": "/plugins",
  "page_size": 1000,
  "total": 3,
  "has_more": false,
  "next_after": null,
  "items": [
    {
      "name": "config",
      "type": "directory",
      "path": "/plugins/config",
      "modified": 1754990400,
      "created": 1754904000,
      "count": 12
    },
    {
      "name": "EssentialsX.jar",
      "type": "file",
      "path": "/plugins/EssentialsX.jar",
      "modified": 1754986800,
      "created": 1754986800,
      "size": 2381742
    },
    {
      "name": "latest-config.yml",
      "type": "symlink",
      "path": "/plugins/latest-config.yml",
      "modified": 1754986000,
      "created": 1754986000,
      "target": "config/2026-08-01.yml"
    }
  ]
}
```

`next_after` is the name the next request passes as `after`; with `has_more: false` it is
`null`. `total` is the number of entries at the time of **this** request and therefore a
snapshot — no consumer in the interface, only to put the 20,000 limit in perspective.

`count` costs one extra `getdents` per subdirectory, but only for the entries of the
page being served. That is the same route the desktop app takes
(`/root/ref-modrinth/apps/app-frontend/src/pages/instance/files/index.vue:102-109`), and the price
for the interface being able to show "12 items" and sort by it.

Errors: 400 `invalid_path`, 400 `not_a_directory`, 403 `forbidden_path`,
403 `permission_denied`, 404 `not_found`.

---

### E3 — `POST /api/v1/servers/{server_id}/files/create`

Permission: `files:write`.

```json
{ "path": "/plugins/config/new-file.yml", "type": "file" }
```

`type` is `"file"` or `"directory"`. Only the **last** segment is created; if the
parent directory is missing: 404 `parent_not_found`. New files are empty, directories get `0770`
and inherit the group `craftpanel` through the setgid bit (`docs/PLAN.md:169-173`).

The interface checks the name itself against `^[a-zA-Z0-9-_.\s]+$` (file) and
`^[a-zA-Z0-9-_\s]+$` (folder) — `components/modals/FileCreateItemModal.vue:99-108`. The server
does **not** check the same: it forbids only `/`, null bytes, `.` and `..`. Otherwise
existing files with umlauts or brackets could no longer be touched.

**201**

```json
{
  "item": {
    "name": "new-file.yml",
    "type": "file",
    "path": "/plugins/config/new-file.yml",
    "modified": 1754991000,
    "created": 1754991000,
    "size": 0
  }
}
```

The answer contains the finished `FileItem`, so that the provider can extend the list without a
second request; Modrinth makes one up on the client instead
(`layouts/wrapped/hosting/manage/files.vue:295-304`).

Errors: 400 `invalid_path`/`invalid_name`, 403 `forbidden_path`/`permission_denied`,
404 `parent_not_found`, 409 `already_exists`, 409 `server_busy`, 507 `no_space`.

---

### E4 — `POST /api/v1/servers/{server_id}/files/move`

Serves `moveItem` **and** `renameItem`. Permission: `files:write`.

```json
{ "source": "/plugins/EssentialsX.jar", "destination": "/plugins/disabled/EssentialsX.jar", "overwrite": false }
```

- `destination` is the **complete** new path including the name (`layout.vue:453`).
- Renaming: the provider replaces the last segment of `source` with `newName` and checks
  beforehand that `newName` contains no `/`. The dialog only lets
  `^[a-zA-Z0-9-_.\s]+$` (file) and `^[a-zA-Z0-9-_\s]+$` (folder) through
  (`FileRenameItemModal.vue:73`, `:78`) — the same narrow check as when creating, with the same
  consequence: an existing file with an umlaut cannot be renamed to another one with an umlaut.
  The server stays generous all the same, otherwise undo would be broken: `undo` plays the old
  name back unchanged, past the dialog (`file-undo-redo.ts:32-34`).
- `overwrite` is optional, default `false`. The interface never sets it; the field exists for
  later flows and makes the behavior explicit instead of silent.
- `source == destination`: 200, nothing happens (undo/redo can produce this).
- If `source` is a directory and `destination` lies inside it: 400 `invalid_move`. The interface
  catches that when dragging (`composables/file-drag-state.ts:98`), in the dialog it does not.
- Implemented as `renameat2` on the parent descriptors, so atomic within the same
  file system; across file system borders it copies and deletes afterwards — and before that hands
  the tree back to the game account (`chown-tree`), because every file is copied and every
  directory is deleted into.

**200** `{ "moved": true }`

Errors: 400 `invalid_path`/`invalid_move`, 403 `forbidden_path`/`permission_denied`,
404 `not_found`/`parent_not_found`, 409 `already_exists`, 409 `file_not_accessible`,
409 `server_busy`, 507 `no_space`.

---

### E5 — `DELETE /api/v1/servers/{server_id}/files`

Permission: `files:write`. Parameters: `path` (mandatory), `recursive` (`true`/`false`, default
`false`).

The interface sets `recursive` exactly when the entry is a directory
(`layout.vue:471`, `layout.vue:582`). A directory without `recursive` and with content:
409 `not_empty`. A symbolic link is always removed as a link, never walked.

The batch delete button sends **one request per entry**, without waiting for each other
(`layout.vue:579-584`). So the server has to take several deletions in the same directory in
parallel; "already gone" is not an error: on `not_found` we still answer **204**, so that a
double click produces no red notice.

Before a `recursive=true` on a directory the panel hands the tree back to the game
account (`chown-tree`): otherwise the deletion stops at the first folder the game process has
closed to the group `craftpanel`. A single file does not need that.

**204** without a body.

Errors: 400 `invalid_path`, 403 `forbidden_path`/`permission_denied`, 409 `not_empty`,
409 `file_not_accessible`, 409 `server_busy`, 500 `io_error`.

---

### E6 — `GET /api/v1/servers/{server_id}/files/content`

One endpoint for three contract methods: `readFile`, `readFileAsBlob`, `downloadFile`.
Permission: `files:read`.

| Parameter | Type | Default | Note |
|---|---|---|---|
| `path` | string | — | mandatory |
| `max_bytes` | integer | no limit | If the file is bigger: 413 `file_too_large`, **without** sending a body |
| `download` | `0`/`1` | `0` | Affects only the file name in `Content-Disposition` |

The provider sets `max_bytes = max_text_bytes` for `readFile` and `readFileAsBlob` (the editor
must not pull a 2 GiB `latest.log` into memory) and leaves it out for `downloadFile`.

Response headers, always:

```
Content-Type: application/octet-stream
Content-Disposition: attachment; filename="latest.log"; filename*=UTF-8''latest.log
X-Content-Type-Options: nosniff
Content-Length: 918273
Accept-Ranges: bytes
ETag: "1754986800-918273"
Cache-Control: private, no-cache
```

`Range` is supported (206 with `Content-Range`), so that an interrupted download of a large
world file can be resumed. The interface does not use it; browsers do it by themselves.
`ETag` is `mtime-size` and serves only the cache: we do **not** demand an `If-Match` when
writing, see 5.6.

Errors: 400 `invalid_path`/`not_a_regular_file`, 403 `forbidden_path`/`permission_denied`,
404 `not_found`, 413 `file_too_large`.

---

### E7 — `PUT /api/v1/servers/{server_id}/files/content`

Serves `writeFile` (editor) **and** `uploadFiles` (upload). Permission: `files:write`.

| Parameter | Type | Default | Note |
|---|---|---|---|
| `path` | string | — | mandatory, complete target path including the file name |
| `on_conflict` | `overwrite` \| `fail` | `fail` | The editor sends `overwrite`, the upload sends `fail` |

Body: the raw bytes, `Content-Type: application/octet-stream`. **No multipart.** Reason: an
`XMLHttpRequest` can send a `File` object unchanged as the body and reports
upload progress while doing so; multipart brings nothing here and costs a parser. Kyros does it the
same way (`api-client/src/modules/kyros/files/v0.ts:108-116`, parameters in the query string, file in
the body).

How it writes: `.<name>.part.<ulid>` is written into the same directory, `fsync`, then
`renameat2` onto the target name. That way there is no half-written `server.properties`. If
the connection breaks, the part file is removed; part files left over and older than 24 hours
are cleared away by the service at start. Part files are **not** hidden from the listing —
a hidden file that takes up space would be a lie to the user.

**204** without a body.

Errors: 400 `invalid_path`/`invalid_name`, 403 `forbidden_path`/`permission_denied`,
404 `parent_not_found`, 409 `already_exists` (with `on_conflict=fail`), 409 `server_busy`,
413 `file_too_large` (> `max_upload_bytes`), 507 `no_space`.

---

### E8 — `POST /api/v1/servers/{server_id}/files/extract`

Permission: `files:write`.

```json
{ "path": "/plugins/pack.zip", "target": null, "override": true, "dry": true }
```

- `dry: true` → **synchronous** answer with `ExtractDryRunResult`. No operation is created.
- `dry: false` → **202** with the operation ID; the course of it runs over the WebSocket (section 3).
- The interface always passes `override` through as `true` (`layout.vue:506`, `layout.vue:533`); it
  gets the actual consent beforehand through the conflict dialog (`layout.vue:508-512`).
  With `override: false` the operation aborts at the first conflict.
- `target` is optional. **Default: the directory the archive lies in.** Modrinth extracts
  to `/`, so into the server root (`api-client/src/modules/kyros/files/v0.ts:253`) — that is
  modpack installation, not file management, and that is exactly the surface we switched off
  (1.4). A user extracting `plugins/pack.zip` expects the content in `plugins/`. The
  parameter stays in the contract all the same, because modpack installation in P3 will use the
  same endpoint with `target: "/"`.

ZIP only. `.jar`, `.tar`, `.gz`, `.rar`, `.7z` give 415 `unsupported_archive`. Reason: the
interface offers "Extract" for `.zip` only (`layout.vue:670`,
`FileTableRow.vue:208`), and extracting a `.jar` is never what anybody wants.

**Checks per archive entry** — they belong in the contract because they change results:

1. The entry name is normalized like a `path` (2.1). `..`, absolute paths, null bytes,
   invalid UTF-8 → the entry is skipped, the operation ends as `failed-path`.
2. Entries that are symbolic links or device files are skipped.
3. Sum of the uncompressed sizes > `max_extract_uncompressed_bytes` or entry count >
   `max_extract_entries` (both from E1) → 413 `archive_too_large`, already in the dry run, because
   the values are in the ZIP's central directory.
4. A ratio of uncompressed to compressed above 200:1 with more than 64 MiB of output → abort with
   `failed-corrupt`.
5. Every entry is written like E7 (part file, then rename), directories created
   beforehand.

**200 with `dry: true`**

```json
{
  "modpack_name": "Cobblemon Official",
  "conflicting_files": [
    "/mods/fabric-api-0.100.0.jar",
    "/config/cobblemon/main.json"
  ]
}
```

`conflicting_files` are the paths that already exist and would be overwritten — at most
**200** entries. Above 100 entries the dialog switches to the text "Over 100
files will be overwritten … here are some of them" anyway (`FileUploadConflictModal.vue:94-98,127`),
so the truncation is visibly represented correctly there.

`modpack_name` comes from `modrinth.index.json` (`name`) or `manifest.json` (`name`) in the archive,
otherwise `null`. Nobody here reads it (1.3), it is in the bound type.

**202 with `dry: false`**

```json
{
  "operation": {
    "id": "01J8Z1N4XK2R7C5V8T3QF6HYBW",
    "op": "extract",
    "src": "/plugins/pack.zip",
    "state": "queued",
    "progress": 0,
    "bytes_processed": 0,
    "files_processed": 0,
    "started_at": "2026-08-12T14:03:11Z"
  }
}
```

Errors: 400 `invalid_path`, 403 `forbidden_path`/`permission_denied`, 404 `not_found`,
409 `server_busy`, 413 `archive_too_large`, 415 `unsupported_archive`, 507 `no_space`.

---

### E9 — `GET /api/v1/servers/{server_id}/files/operations`

Permission: `files:read`. The same snapshot the WebSocket sends — for the moment before
the connection is up, and so that the area can be checked without a socket.

**200**

```json
{
  "revision": 41,
  "operations": [
    {
      "id": "01J8Z1N4XK2R7C5V8T3QF6HYBW",
      "op": "extract",
      "src": "/plugins/pack.zip",
      "state": "ongoing",
      "progress": 0.42,
      "bytes_processed": 88129536,
      "files_processed": 210,
      "current_file": "mods/sodium-fabric-0.5.8.jar",
      "started_at": "2026-08-12T14:03:11Z"
    }
  ]
}
```

**The race between E9 and the socket — and how it comes out.** Both deliver a complete
snapshot, and both are under way as soon as the tab opens: the socket sends its
right after connecting (3.1). If the E9 answer arrives **later** than the first
socket message, it would overwrite newer data with older — an operation already finished would be
back on `ongoing` and would stay there until something changes next time. With an operation
that has just finished, nothing changes any more: the notice would be stuck.

That is why every snapshot carries a **`revision`**: a counter per server that rises by one on
every state change and uses the same value over both routes. The provider discards
every snapshot with a `revision` less than or equal to the one last applied, no matter where it
comes from. That makes the order of the answers irrelevant.

---

### E10 — `POST /api/v1/servers/{server_id}/files/operations/{operation_id}/cancel`

Permission: `files:write`. Aborts an operation in `queued` or `ongoing`. Files already
written stay where they are: undoing them would be more dangerous with a half-extracted modpack
than the state itself. The operation then disappears from the snapshot
(reason in 3.2).

**204** without a body. Errors: 403 `permission_denied`, 404 `operation_not_found`,
409 `operation_not_cancelable`.

### E11 — `POST /api/v1/servers/{server_id}/files/operations/{operation_id}/dismiss`

Permission: `files:read` — reason in 2.3. Removes an **ended** operation (`done`,
`failed-*`) from the snapshot. On the server side and therefore for everybody watching this server.

**204** without a body. Errors: 403 `permission_denied`, 404 `operation_not_found`,
409 `operation_not_cancelable` (the operation is still running → E10 first).

Ended operations also disappear on their own after 10 minutes.

---

### 2.9 Upload: progress, cancelling, conflicts

**Progress over HTTP in the browser, not over the WebSocket.** Four reasons:

1. `UploadState` describes what the **browser has sent**: `uploadedBytes`, `totalBytes`,
   `currentFileProgress` (`api-client/src/types/upload.ts:93-100`). Only the browser knows that
   number. A server number lags behind by the buffers of the layers in between and differs
   visibly.
2. No correlation is needed. Over the socket every request would have to carry a session ID, only
   so that the event lands in the right tab again. Modrinth's own implementation builds exactly
   that: an upload session with `create`/`finalize`/`cancel`
   (`composables/hosting/kyros-session-upload.ts:156-181`) — effort we would inherit for nothing.
3. `cancelUpload` has to take effect at once. `XMLHttpRequest.abort()` closes the connection; over
   the socket it would be a request with a travel time.
4. `fetch()` knows no upload progress (streaming bodies are not available everywhere), so
   `XMLHttpRequest` is the means — `@modrinth/api-client` makes the same choice with `client.upload`
   (`api-client/src/modules/kyros/files/v0.ts:108`).

Over the WebSocket runs only what the server alone knows: the extraction operations (section 3).

**Course of events in the provider.** `uploadFiles(files)` is `void`, not `Promise`
(`file-manager.ts:35`) — so the interface does not wait.

1. `uploadState = { isUploading: true, currentFileName: files[0].name, currentFileProgress: 0,
   uploadedBytes: 0, totalBytes: Σ size, completedFiles: 0, totalFiles: files.length }`.
2. Files **one after another**, one `PUT` (E7) each with `path = targetFolder + '/' + file.name` and
   `on_conflict=fail`. One after another, because `currentFileName` and `currentFileProgress` map
   exactly one file in flight (`UploadAdmonition.vue:8-11`). **`targetFolder` is captured once at
   the call**, not read anew from `currentPath` for every file: `uploadFiles` returns no
   promise (`file-manager.ts:35`), uploading does not count among the busy reasons from 2.4,
   and the interface stops nobody from navigating on. Without that capture, the
   remaining files of a batch land in whatever folder the user happens to be standing in.
3. `xhr.upload.onprogress` → advance `uploadedBytes` and `currentFileProgress`, the way
   `kyros-session-upload.ts:55-83` works it out.
4. After every file `completedFiles++`; at the end `isUploading = false` and `refresh()`.
5. `cancelUpload()` aborts the running `XHR` and discards the queue. Files already
   finished stay where they are — Modrinth's session model could take them back, ours
   cannot, and that is the price for there being no staging area.

**Conflicts.** Default `on_conflict=fail` → 409 `already_exists`, the provider shows an
error message with the file name and carries on with the next file. The files tab has
**no** conflict dialog for uploading (the `FileUploadConflictModal` that exists belongs to
extraction, `layout.vue:5,511`), and silently overwriting a `server.properties` with a
file dropped by accident is the more expensive mistake. `on_conflict=overwrite` stands ready in case
a dialog is built later.

**Dropping folders** is not supported: `getDroppedFiles` reads only `DataTransfer.files` and
`items[].getAsFile()` (`composables/file-drop.ts:23-31`), so there is no
`webkitGetAsEntry` recursion and no directory structure. A dropped folder shows up as
nothing. That is behavior of the library, not ours.

---

## 3. WebSocket messages

One socket per server: `/api/v1/servers/{server_id}/ws`. This area contributes **one** kind of
message.

### 3.1 `file_ops` — server → client

A complete snapshot, not a delta. Sent right after connecting and after that
on every state change, but at most every 250 ms (progress changes continuously).

```json
{
  "type": "file_ops",
  "revision": 41,
  "ops": [
    {
      "id": "01J8Z1N4XK2R7C5V8T3QF6HYBW",
      "op": "extract",
      "src": "/plugins/pack.zip",
      "state": "ongoing",
      "progress": 0.42,
      "bytes_processed": 88129536,
      "files_processed": 210,
      "current_file": "mods/sodium-fabric-0.5.8.jar",
      "started_at": "2026-08-12T14:03:11Z"
    },
    {
      "id": "01J8Z1P8YB6D2M4K9S1WQ3RTZC",
      "op": "extract",
      "src": "/backups/world.zip",
      "state": "done",
      "progress": 1,
      "bytes_processed": 4183920640,
      "files_processed": 8123,
      "started_at": "2026-08-12T13:41:02Z"
    }
  ]
}
```

A snapshot instead of a delta, because the receiver simply replaces the list and a
lost packet therefore has no consequence. Modrinth does the same
(`api-client/src/modules/archon/types.ts:1143-1146`, processed in
`layouts/wrapped/hosting/manage/root.vue:1106-1110`).

`ops: []` is a valid and frequent message — it clears the display. `revision` rises then
too, and the provider discards old snapshots by it (E9).

### 3.2 States

| `state` | Display | Note |
|---|---|---|
| `queued` | "Extracting …", the bar pulses (`FileOperationAdmonition.vue:7`) | operation queued |
| `ongoing` | "Extracting …" with progress | |
| `done` | green, dismissible (`FileOperationAdmonition.vue:3,94`) | |
| `failed-path` | red, dismissible | an entry violated 2.1 |
| `failed-corrupt` | red | not a readable ZIP, or a suspected bomb |
| `failed-io` | red | disk full, permissions, other |

We do **not** send `cancelled`. `FileOperationAdmonition.vue:94` counts only `done` and `fail*` as
ended — an operation with `state: "cancelled"` would stay up forever as a non-dismissible
"Extracting …". Modrinth gets around that by having the client dismiss every `cancelled` operation
itself right away (`layouts/wrapped/hosting/manage/root.vue:1116-1123`). We save ourselves the
detour: after E10 the operation disappears from the snapshot.

`progress` is **0…1**, not percent (`components/base/Admonition.vue:148` clamps to [0,1],
`:83` and `:93` multiply by 100 themselves).

When an operation moves to `done` or `failed-*`, the provider reloads the listing; that
is the substitute for the missing file watching (1.4, point 3).

---

## 4. Data types

To be adopted literally. `FileItem`, `ExtractDryRunResult` and `UploadState` are Modrinth's
types — repeated here only for checking; do **not** declare them again, import them from
`@modrinth/ui`.

```ts
/** Relative to the root, POSIX, leading '/' optional when sending, always present when receiving. */
export type FilePath = string

/** Congruent with FileItem from files-tab/types.ts:1 — directly assignable. */
export interface ApiFileItem {
	name: string
	type: 'file' | 'directory' | 'symlink'
	path: FilePath
	/** Unix seconds. Binding through FileTableRow.vue:303. */
	modified: number
	/** Unix seconds, 0 if the file system keeps no birth time. */
	created: number
	/** Only for type === 'file'. */
	size?: number
	/** Only for type === 'directory'. */
	count?: number
	/** Only for type === 'symlink'; raw link content. */
	target?: string
}

export interface FilesMetaResponse {
	root_path: string
	max_upload_bytes: number
	max_text_bytes: number
	max_page_size: number
	default_page_size: number
	max_extract_uncompressed_bytes: number
	max_extract_entries: number
}

export interface ListDirectoryQuery {
	path?: FilePath
	/** Name of the last entry of the previous page, exclusive. */
	after?: string
	page_size?: number
}

export interface ListDirectoryResponse {
	path: FilePath
	page_size: number
	/** A snapshot, no consumer in the interface. */
	total: number
	has_more: boolean
	/** `after` for the next request; `null` when `has_more` is false. */
	next_after: string | null
	items: ApiFileItem[]
}

export interface CreateItemRequest {
	path: FilePath
	type: 'file' | 'directory'
}

export interface CreateItemResponse {
	item: ApiFileItem
}

export interface MoveItemRequest {
	source: FilePath
	/** Complete target path including the file name. */
	destination: FilePath
	overwrite?: boolean
}

export interface MoveItemResponse {
	moved: boolean
}

export interface DeleteItemQuery {
	path: FilePath
	recursive?: boolean
}

export interface ReadContentQuery {
	path: FilePath
	max_bytes?: number
	download?: 0 | 1
}

export type WriteConflictMode = 'overwrite' | 'fail'

export interface WriteContentQuery {
	path: FilePath
	on_conflict?: WriteConflictMode
}

export interface ExtractRequest {
	path: FilePath
	/** null or left out = the directory of the archive. */
	target?: FilePath | null
	override: boolean
	dry: boolean
}

/** Congruent with ExtractDryRunResult from files-tab/types.ts:64. */
export interface ExtractDryRunResponse {
	modpack_name: string | null
	/** At most 200 entries. */
	conflicting_files: FilePath[]
}

export type FileOperationKind = 'extract'

export type FileOperationState =
	| 'queued'
	| 'ongoing'
	| 'done'
	| 'failed-path'
	| 'failed-corrupt'
	| 'failed-io'

/** Superset of FileOperation from files-tab/types.ts:32 — directly assignable. */
export interface ApiFileOperation {
	id: string
	op: FileOperationKind
	src: FilePath
	state: FileOperationState
	/** 0…1, not percent. */
	progress: number
	bytes_processed: number
	files_processed: number
	current_file?: string
	/** RFC 3339, UTC. No consumer in the interface. */
	started_at: string
}

export interface ExtractStartedResponse {
	operation: ApiFileOperation
}

export interface FileOperationsResponse {
	/** Monotonic per server; older snapshots are discarded. */
	revision: number
	operations: ApiFileOperation[]
}

/** Server → client, a snapshot. */
export interface WsFileOpsMessage {
	type: 'file_ops'
	/** The same counter as in FileOperationsResponse. */
	revision: number
	ops: ApiFileOperation[]
}

export type FileErrorCode =
	| 'invalid_path'
	| 'path_too_long'
	| 'invalid_name'
	| 'invalid_move'
	| 'not_a_regular_file'
	| 'not_a_directory'
	| 'non_utf8_name'
	| 'forbidden_path'
	| 'permission_denied'
	| 'not_found'
	| 'parent_not_found'
	| 'operation_not_found'
	| 'already_exists'
	| 'not_empty'
	| 'server_busy'
	| 'operation_not_cancelable'
	| 'file_too_large'
	| 'unsupported_archive'
	| 'archive_too_large'
	| 'no_space'
	| 'io_error'

export interface ApiError {
	error: FileErrorCode
	message: string
}
```

---

## 5. Open questions and assumptions

### 5.1 Decided against the rule: `modified` and `created` as Unix seconds

The general rule says RFC 3339. But `FileItem.modified` and `FileItem.created` are not
timestamps *of ours*, they are fields of a bound foreign type, and the interface computes
`new Date(props.modified * 1000)` hard (`FileTableRow.vue:303,308`). Sending strings would mean
converting them back in every row of every listing — with 20,000 entries, 40,000
`Date.parse` calls for nothing.

**Proposal: numbers, and only at this one place.** Everything else in this area
(`started_at`) is RFC 3339. **Whoever decides otherwise** changes one line in the provider:
`modified: Math.floor(Date.parse(x.modified) / 1000)`. The decision belongs to the steering, because
it touches a cross-area rule.

### 5.2 Extraction locks the write actions — at Modrinth it does not

Modrinth's busy reasons know only installation and content sync
(`composables/server-manage-core-runtime.ts:108-126`); a running extraction locks nothing there.
We do lock (2.4), because renaming while something is being extracted into the same directory is a
race whose outcome nobody can explain. The cost: whoever extracts a large modpack cannot edit a
file for five minutes.

The text for it is in the library ("File operations are disabled while the operation is
in progress.", `ServerPanelAdmonitions.vue:52-55`) — **but it does not appear by itself.** The
notice hangs on `filesBusyHeader`, and that is fed from `busyReasons` of the **server** context
(`ServerPanelAdmonitions.vue:79-85`, `:91-93`, `:269-278`), not from the `isBusy` of the
files contract. Set only `isBusy` and you get gray buttons with a tooltip and no explanation
above them. So for the duration of a file operation the "Servers" area has to put a busy reason
into `busyReasons` — see 5.9.

### 5.3 The extraction target is the directory of the archive, not the root

Reasoned in E8. Whoever wants to extract a modpack into the root puts the archive into the root — or
P3 uses `target: "/"`.

### 5.4 File names that are not UTF-8

Linux allows any byte sequence except `/` and `0x00`. A mod that creates a file with a cp1252 name
produces an entry JSON cannot represent.

**Assumption:** such entries are **listed** with a lossily converted name (U+FFFD),
so that the user sees them. Reading and writing accesses to them answer with
400 `non_utf8_name`; **deleting** is allowed if the lossy form is unique within the directory
— otherwise you could no longer get rid of the junk. Our own extractor never produces such names
(2.1, check 1). What remains to be decided is whether it is worth the effort or whether leaving
them out is enough.

### 5.5 Very large directories

The interface virtualizes (`layout.vue:345-354`), but searches and sorts over the whole array
(`file-search.ts:9`, `file-sorting.ts:26`). The provider fetches up to 20,000 entries in pages. For
more there is **no** field in the contract that could show "truncated" — the list would be
silently incomplete. Modrinth has the same problem and caps at 2,000
(`layouts/wrapped/hosting/manage/files.vue:132`). Whoever wants a visible truncation needs a
row of their own above the table; that would be a change to `packages/ui` and therefore against the
plan's basic decision.

### 5.6 No `If-Match` on saving

The Minecraft server writes into its own files while somebody has them open in the editor. An
`If-Match` with 412 would prevent such overwrites — but the editor shows the same text for every
error, "Save failed / Could not save the file." (`FileEditor.vue:260-267`), so the user
would not learn why. Therefore: `ETag` is delivered, `If-Match` accepted optionally, never
demanded. **To be decided** as soon as there is a place for the message.

### 5.7 The editor can damage non-UTF-8 **content**

`readFile` delivers a `string` (`file-manager.ts:30`), and the editor saves it back unchanged
(`FileEditor.vue:247`). A latin-1 encoded `server.properties` is decoded with replacement
characters on reading and written with replacement characters on saving — data loss without a
warning. That sits in the library, not in our API. Mitigation would be a server-side
`require_utf8=1` on E6 that answers 415 instead of delivering mojibake. **Not decided.**

### 5.8 Two providers have to be in place before the tab opens

`FileEditor.vue:70` calls `injectModrinthClient()` without a fallback value; `providers/create-context.ts:66`
then throws. So the file editor crashes if the application provides no client. The plan
foresees one anyway (`docs/PLAN.md:78-80`, `packages/api-client` against the real
`api.modrinth.com`). **Belongs to another area**, but has to be in place before P2.

The second is the notification provider, and it weighs more, because it concerns the whole tab
and not just the editor — reasoned in 1.5. The third provider, `injectFilePicker`, is
harmless by comparison: `layout.vue:295` passes `null` as a fallback value and falls back to a
generated `<input type="file">` (`layout.vue:620-628`).

A consequence of this: "Share to mclo.gs" in the editor sends the file content **straight from the
browser** to mclo.gs (`FileEditor.vue:277`). No endpoint on our side, but a hand-over of data that
somebody has to approve. In a sealed-off network the button fails silently.

### 5.9 What the Servers area has to take over from us

`activeOperations`, `dismissOperation`, `uploadState` and `cancelUpload` are **also** in
`providers/server-context.ts:66-71`, and the notice bar reads them from there only
(`ServerPanelAdmonitions.vue:223,234,351,373`), not from the files contract. So the "Servers" area
has to fill these four fields from the same sources: WS `file_ops` (3.1), E9, E10/E11 and the
upload state from 2.9. Miss that and you get a files tab that works but does not show a
single progress notice.

A **fifth** field comes on top: `busyReasons` (`server-context.ts:57`). While a file operation
runs, a reason belongs in there, otherwise the explanation for the locked buttons is missing (5.2).
Modrinth fills it from installation and content sync (`server-manage-core-runtime.ts:108-126`)
and has foreseen nothing for extraction operations — that is our addition, not theirs.

Not a field of its own, but the same building site: the notice bar appears only if the
server frame mounts `ServerPanelAdmonitions` at all. Without it, `activeOperations`,
`dismissOperation`, `uploadState` and `cancelUpload` in the files contract are a pure formality — in
`files-tab/` itself **nobody** reads them (checked, 1.2).

### 5.10 Smaller assumptions

- **Concurrent writes** to the same path are not locked; the last request
  wins. A locking mechanism between browser tabs is not worth it for a tool meant for one team.
- **Deleting a file that does not exist** is a success (204), not a 404 — because of
  batch deletion without a queue (`layout.vue:579-584`).
- **`max_upload_bytes` = 4 GiB, `max_text_bytes` = 8 MiB** are starting values, not findings;
  they belong in the configuration file.
- **Per-user quotas** (disk space) are not foreseen in the plan; `no_space` maps
  `ENOSPC` and `EDQUOT` alike, in case somebody switches quotas on later.

### 5.11 Measured afterwards: how a refusal gets into a test at all

`cargo test` runs as **root** on this machine, and root is never turned away by a `drwx--S---`.
The refusal that 1.7 (`file_not_accessible`) and the disk measurement are about therefore
**cannot** be staged against root at all — every test for it would be green without having
measured anything.

The way out is `setfsuid`: it swaps the identity the kernel checks file accesses against, and does
that for **exactly this thread**, so the tests next to it keep theirs. What a run under
this guard hits is the refusal the panel hits in operation, and no substitute for it
(`files/mod.rs:534-539,564-567`). Two things about it that you would otherwise work out again next
time: `setfsuid` answers with the **previous** value, so a second call is the
only way to ask whether the first took hold; and the `errno` branch itself
(`EACCES`/`EPERM` → `409 file_not_accessible` instead of `500 internal`) is secured, where it cannot
be staged, with the table as a yardstick of its own (`files/mod.rs:671-682`).

The same situation holds for the `chown-tree` hand-back before a copy, a deletion or an
extraction: that the panel **asks** at all is checkable, the refusal behind it is not. So the
tests measure two things — that it asks where a tree has to be walked, and that it does
**not** ask where there is nothing to walk (a single file is unlinked from its
parent directory) — and on top of that the modes afterwards, which are visible to root:
if `below_server` points one segment off, they fall over (`api/files.rs:1461-1466,1484-1485`,
`files/mod.rs:718-722`).
