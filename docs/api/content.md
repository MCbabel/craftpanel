# Content: mods, plugins, modpacks

Interface contract for the "Content" tab. It drives
`vendor/modrinth/ui/src/layouts/shared/content-tab/layout.vue` **unchanged**, filled through
`provideContentManager` from
`vendor/modrinth/ui/src/layouts/shared/content-tab/providers/content-manager.ts:115`.

All paths without a leading `/` are relative to `vendor/modrinth/` unless stated otherwise.
`ref/` stands for the reference clone `/root/ref-modrinth/`; `ref/…/content.vue` is shorthand for
`ref/packages/ui/src/layouts/wrapped/hosting/manage/content.vue`. **`wrapped/` does not exist
under `vendor/modrinth/`** — there is only `ui/src/layouts/shared/` there; every statement about
Modrinth's own hosting page is therefore evidenced against the reference clone.

This area hangs off three areas that are already settled and does not reinvent their things:
`docs/api/creation-and-progress.md` (operations, progress, `busy_reasons`,
`409 server_busy`), `docs/api/auth.md` (permission bits) and `docs/api/backups.md`
(`InlineBackupCreator`).

---

## 1. The provider contract

### 1.1 What else the layout expects

The layout injects exactly one mandatory context, but the modals the page mounts itself reach for
further providers:

| Provider | Source | Required? | Our answer |
|---|---|---|---|
| `ContentManagerContext` | `ui/src/layouts/shared/content-tab/layout.vue:158` | **yes** | Section 1.2 |
| `AppBackupContext` (`createBackup()`) | `ui/src/providers/app-backup.ts:3`, used in `ui/src/layouts/shared/content-tab/composables/use-inline-backup.ts:14,31-43` | **no** | **Unreachable, see 1.4.** `use-inline-backup.ts:16` enters this branch only when `injectModrinthServerContext(null)` returns `null` |
| `ModrinthServerContext` | `use-inline-backup.ts:13`, `InlineBackupCreator.vue:81` | **yes — but it comes from the server frame** | Not our contract. Provided by the "Create and progress" area (`docs/api/creation-and-progress.md`, 1.1), and `worldId` is the constant `"default"` there (ibid. 5.1) |
| `TagsContext` | `content-updater-modal/index.vue:313` (`injectTags(null)`) | no | Optional; read only in mode `incompatibility-warning` (`index.vue:611`) |
| `PageContext` | `ModpackContentModal.vue:37` (`injectPageContext(null)`) | no | Optional (external links) |
| `FilePicker` | `ContentInstallModal.vue:441` | no | We do not use `ContentInstallModal` — `layout.vue` does not import it (import list `layout.vue:30-37`); it serves the desktop case "which instance to install into" |

`InlineBackupCreator` sits in four modals (`ConfirmDeletionModal.vue:17`,
`ConfirmBulkUpdateModal.vue:13`, `ConfirmUnlinkModal.vue:13`,
`ContentDependencyWarningModal.vue:160`). Without `AppBackupContext` **and** without
`ModrinthServerContext`, `useInlineBackup` returns `available: false`
(`use-inline-backup.ts:48-59`) and the "Create backup" button disappears from every delete modal.
We need to do nothing for that: the server frame provides `ModrinthServerContext` anyway, which
makes it `available: true` through the Archon branch. On top of that,
`InlineBackupCreator.vue:82-83` checks the bit `BACKUPS` (`docs/api/auth.md`, 1.2); without that
bit the button stays disabled.

### 1.2 Field by field

Order as in `content-manager.ts`. "Line" = line in
`ui/src/layouts/shared/content-tab/providers/content-manager.ts`.

| Field | Line | Type per the contract | Actually required? | Where the value comes from |
|---|---|---|---|---|
| `items` | 40 | `Ref<ContentItem[]>` | yes | `GET /content` → `items`, put into a `ref` |
| `loading` | 41 | `Ref<boolean>` | yes (`layout.vue:757`) | Loading state of the query |
| `error` | 42 | `Ref<Error \| null>` | yes (`layout.vue:759,764` reads `.message`) | HTTP error → `new Error(body.message)` |
| `modpack` | 45 | `Ref<ContentModpackData \| null>` | yes, may be `null` (`layout.vue:773`) | `GET /content` → `modpack`, reshaped in the provider. Careful: `categories` is `ContentModpackCardCategory[]` there, so **objects** `{ icon, name, project_type, header }` (`types.ts:113`, `Labrinth.Tags.v2.Category`), not strings. Only `name` is read (`ContentModpackCard.vue:295-300`), so our JSON delivers `string[]` and the provider builds the objects |
| `isPackLocked` | 46 | `Ref<boolean>` | **yes, but with no effect** | Always `ref(false)`; see 1.3 |
| `isBusy` | 49 | `Ref<boolean>` | yes (`layout.vue:283,285,419,483`) | `busyReasons.value.length > 0` from `ModrinthServerContext` (`providers/server-context.ts:57`), fed from the WS message `operations` — **not** a field of this area |
| `busyMessage` | 50 | optional | in practice yes, otherwise empty tooltips (`layout.vue:284,822`) | First entry from `busyReasons`, translated; `ref/…/content.vue:175-198` does the same |
| `skipNonEssentialWarnings` | 51 | optional | no (`layout.vue:159` with `?? false`) | Local setting in the browser (`localStorage`), no endpoint |
| `disableAddContent` | 52 | optional | no (`layout.vue:827` with `?.`) | `!can_write` from `GET /content` → `permissions` |
| `disableAddContentTooltip` | 53 | optional `string` | no | Fixed text "No permission" |
| `contentTypeLabel` | 56 | `Ref<string>` | yes (`layout.vue:812,1008,1108`) | Derived from `GET /content` → `content_type` (see 1.5) |
| `toggleEnabled` | 59 | `(item) => Promise<void>` | yes (`layout.vue:475,582,606`) | `POST /content/enable` or `/disable` with one item |
| `deleteItem` | 60 | `(item) => Promise<void>` | yes (`layout.vue:514,527`) | `POST /content/delete` with one item |
| `refresh` | 61 | `() => Promise<void>` | yes (`layout.vue:264`) | `GET /content?refresh_updates=true` |
| `browse` | 62 | `() => void` | yes (`layout.vue:842,996`) | Router jump to our search page, **no** endpoint |
| `uploadFiles` | 63 | `() => void` | yes (`layout.vue:828,982`) | Opens `<input type=file>`, then `POST /content/upload` |
| `bulkDeleteItems` | 66 | optional | we supply it | `POST /content/delete` with n items |
| `bulkEnableItems` | 67 | optional | we supply it | `POST /content/enable` |
| `bulkDisableItems` | 68 | optional | we supply it | `POST /content/disable` |
| `canDeleteItem` | 69 | optional | we supply it | `item.locked === false && permissions.can_write` |
| `canToggleItem` | 70 | optional | we supply it | same |
| `getDeleteWarning` | 71 | optional | no | `null` — we have no managed instances |
| `getDisableWarning` | 72 | optional | no | `null` — same |
| `getDeleteDependencyWarning` | 73 | optional | we supply it | `POST /content/dependents` |
| `hasUpdateSupport` | 78 | `boolean` (not a Ref!) | yes (`layout.vue:232`) | Constant `true` |
| `updateItem` | 79 | optional `(id) => void` | we supply it (`layout.vue:645`) | Opens `ContentUpdaterModal`, see 1.6 |
| `bulkUpdateAll` | 80 | optional, with `onProgress` | we supply it | `POST /content/update {"all": true}` + WS progress |
| `bulkUpdateItem` | 81 | optional | **no**, we do not supply it | see 1.7 |
| `bulkUpdateItems` | 82 | optional | we supply it | `POST /content/update` with n items |
| `updateModpack` | 85 | optional | we supply it when a modpack is linked | Opens `ContentUpdaterModal` in modpack mode |
| `viewModpackContent` | 86 | optional | we supply it | `GET /content/modpack/contents` → `ModpackContentModal.show()` |
| `unlinkModpack` | 87 | optional | we supply it | `POST /content/modpack/unlink` |
| `openSettings` | 88 | optional | we supply it | Opens the settings modal ("Settings" area) |
| `switchVersion` | 91 | optional | we supply it (`layout.vue:651`) | Opens `ContentUpdaterModal` with `switchMode: true` |
| `getOverflowOptions` | 94 | optional | we supply it (`layout.vue:296`) | Purely client-side: "Copy link", "Show file in the file manager" |
| `shareItems` | 97 | optional | we supply it | Purely client-side out of `items` (model: `ref/apps/app-frontend/src/pages/instance/content/index.vue:1272-1304`) |
| `getItemId` | 100 | optional | **yes**, we override it (`layout.vue:162`) | `item => item.id` (ULID), reasoning in 1.8 |
| `isBulkOperating` | 103 | optional `Ref<boolean>` | we supply it (`layout.vue:247-250`) | Our own `ref`; suppresses reloads during bulk runs |
| `deletionContext` | 106 | optional | we supply `'server'` (`layout.vue:1110,1130,1140,1148`) | Constant |
| `mapToTableItem` | 109 | `(item) => ContentCardTableItem` | **yes** (`layout.vue:277`) | Pure transformation in the provider, see 1.9 |
| `filterPersistKey` | 112 | optional | we supply `server:<id>` | Constant from the route |

### 1.3 `isPackLocked` — a required field with no effect

The type demands it (line 46), and it is read in exactly two places: `layout.vue:234` passes it
on to `useContentFilters`, where `config.isPackLocked` is **never** read
(`composables/content-filtering.ts:55-165` uses only `showTypeFilters`, `showUpdateFilter`,
`showWarningsFilter`, `persistKey`); `layout.vue:305` writes it into a debug output.
We deliver `ref(false)` and write it down here so nobody goes looking for it later.

### 1.4 `world_id` — does not occur here

The shared content tab knows no `world_id`; a `grep` over
`ui/src/layouts/shared/content-tab/` finds no hit. The identifier appears only
- in `ref/packages/ui/src/layouts/wrapped/hosting/manage/content.vue` (which we do not adopt;
  under `vendor/modrinth/` there is no `wrapped/` at all — only `shared/`) and
- in `use-inline-backup.ts:65,130,148,154,163` — but only in the branch that is entered when
  `injectModrinthServerContext` returns something (`use-inline-backup.ts:13`).

The way out through `provideAppBackup` (`use-inline-backup.ts:16-46`) **is not open to us**:
this branch is entered only when `injectModrinthServerContext(null)` returns `null`
(`use-inline-backup.ts:13,16`). `provide` acts on the component tree, and the server frame
provides `ModrinthServerContext` for console, files and overview
(`docs/api/creation-and-progress.md`, 1.1). The content tab hangs inside it. There is no place
where you could take it away again for this one tab.

**So the Archon branch applies.** `worldId` is the constant `"default"` settled across areas
(`creation-and-progress.md`, 5.1), and `archon.backups_queue_v1`
(`use-inline-backup.ts:130,154,163`) is served by the adapter from `docs/api/backups.md`,
section 1 — the same finding is recorded there as the "central finding". For this area it follows
that there is **nothing to do** — no `world_id` in our endpoints, no backup endpoint here, not a
line of code. The earlier version of this section proposed `provideAppBackup`; that was wrong.

### 1.5 `contentTypeLabel`

Modrinth derives the label from the loader
(`ref/…/content.vue:140-145`): Paper/Purpur → `plugin`, Vanilla → `datapack`,
otherwise → `mod`. We ship the result as the field `content_type`, because we know more loaders
than Modrinth does (plan, section "Loaders and their sources"): Paper, Folia, Purpur, Leaf and
Velocity are plugin platforms, Vanilla is `datapack`, Fabric/Quilt/Forge/NeoForge are
`mod`.

### 1.6 The update modal needs real Modrinth objects

`ContentUpdaterModal` (`content-tab/components/modals/content-updater-modal/index.vue:419`)
demands `versions: Labrinth.Versions.v2.Version[]` — **complete** v2 version objects, including
`changelog` (`index.vue:170-176`), `game_versions`/`loaders` for the compatibility check
(`index.vue:497-516` via `ui/src/utils/version-compatibility.ts:38`) and `date_published`
(`index.vue:521-526`). The changelog is fetched on demand when the user picks a version or hovers
over one for 500 ms (`index.vue:473-475,618-631`).

Plus four mandatory values that come from 2.1 and not from Modrinth: `currentVersionId`,
`currentGameVersion`, `currentLoader` and `projectType` (`index.vue:419-425`). `currentVersionId`
must **appear in the fetched list**. Otherwise the "Current" badge stays away
(`index.vue:543,551`), `isDowngrade` is always `false` (`index.vue:521-526`), and the update
button can be pressed on the version that is already installed
(`index.vue:228`). Our `item.version.id` is exactly this value.

From that it follows necessarily: **the browser has to be able to call `/v2/project/{id}/version`
and `/v2/version/{id}`.** That is the real reason for the proxy in section 2.17, not just search.

### 1.7 Why no `bulkUpdateItem`

`layout.vue:743` picks `bulkUpdateItem` only when neither `bulkUpdateAll` nor `bulkUpdateItems`
is set. We set both, so `bulkUpdateItem` would be dead code.

### 1.8 A stable row id

The layout's default: `getItemId` falls back to `file_path ?? file_name ?? id`
(`layout.vue:162`, identically `ContentSelectionBar.vue:134`). Neither fallback works for us:
enabling and disabling renames the file — Modrinth's own desktop app reads the new path back and
decides from the suffix `.disabled` whether something is active
(`ref/apps/app-frontend/src/pages/instance/content/index.vue:637-639`). If that changed the row
id, `useContentSelection` would throw the selection away on the next pass
(`composables/content-selection.ts:16-26`).

Hence: `id` is a ULID of the database row, stable across renaming, updating and reinstalling the
same file. The provider sets `getItemId: item => item.id`.
All bulk endpoints take this `id`, never file names.

### 1.9 `mapToTableItem` — pure transformation, no extra data

`layout.vue:277-297` adds `disabled`, `disabledTooltip`, `toggleDisabled`, `installing`,
`hasUpdate`, `isClientOnly`, `clientWarning`, `hideSwitchVersion` and `overflowOptions` to the
result itself. So the provider only has to deliver `id`, `project`, `projectLink`, `version`,
`versionLink`, `owner`, `source`, `enabled`, `hideDelete`. Two exceptions:

1. **`project` is mandatory and must never become `undefined`.** `ContentCardTableItem.project`
   is not optional (`types.ts:47`), and `ContentCardItem.vue:148,161-166,187,364` reads
   `project.title` and `project.icon_url` unchecked. Our JSON may deliver `project: null`
   (a file with no Modrinth origin), **and then the provider has to substitute**:
   `{ id: item.id, slug: null, title: item.file_name, icon_url: null }`. That is exactly what
   both models do — `ref/…/content.vue:1004-1011` and, for the modpack modal,
   `ModpackContentModal.vue:248-253`. `layout.vue:191` only catches the sort key, not the
   display.
2. **`projectLink` and `versionLink`** we build client-side from `project.slug` as absolute
   Modrinth web links (`https://modrinth.com/<project_type>/<slug>`), because we have no project
   page. In this place Modrinth puts **relative** paths into its own website
   (`ref/…/content.vue:1417-1422`: `/${projectType}/${slug}` and `…/version/${id}`); the absolute
   form only appears there in the "Copy link" entry (`ref/…/content.vue:1368`).

`ContentCardItem.vue:336` shows the "Switch version" button only when `version` is set;
`layout.vue:295` additionally sets `hideSwitchVersion` to `!base.versionLink`. For files with no
Modrinth origin the button therefore disappears by itself — exactly right.

### 1.10 Gaps — what we deliberately do not serve

1. **`getDeleteWarning` / `getDisableWarning`** return `null`. They exist for Modrinth's
   "managed" instances (`ref/apps/app-frontend/src/pages/instance/content/index.vue:1452-1453`,
   `use-managed-content-policy`). We have none.
2. **`source` in `ContentItem`** (`types.ts:52`) stays empty. In Modrinth's desktop app it shows
   which shared modpack a file comes from. One server, one origin.
3. **`source_kind`** (`types.ts:94`) we set to `'local'`, `'modrinth_modpack'` or
   `'server_project'`. `'modrinth_hosting'`, `'imported_modpack'` and `'shared_instance'` we
   never hand out. The field is not read anywhere in the shared layout anyway (`grep` over
   `content-tab/`: only the type declaration).
4. **`external` / `external_url`** (`types.ts:95-96`) we set only for modpack files that do not
   come from Modrinth (CurseForge downloads in an `.mrpack`). `ModpackContentModal` shows a badge
   and a slicer link for those (`ModpackContentModal.vue:304-316,605-627`).
   If we leave out `external_url`, only the slicer button falls away.
5. **`pack_client_retained`** (`types.ts:91`) is **always `false`** for us. Reasoning in 4.5.
6. **`size`** (`types.ts:85`) is never read anywhere in the content tab (`grep`). We deliver it
   anyway, because it helps when debugging and costs nothing.
7. **`ContentInstallModal`** we do not serve (see 1.1).
8. **The progress numbers "Deleting 3/7…"** we do not get for deleting, enabling and disabling.
   See section 3.2 — that is a deliberate decision.
9. **`ContentDiffItem.disabled` and `.fileCount`** (`installation-settings/types.ts:53-54`)
   stay empty. Our preview from 2.14 does not know them — Modrinth's own transformation does not
   set them either (`server-settings/pages/installation.vue:900-906`).
10. **The links in the modpack modal are not ours.** `ModpackContentModal` builds its table rows
    itself and puts **app-internal** paths there: `projectLink` to `/project/<id>`
    (`ModpackContentModal.vue:256`) and `owner.link` to `/user/<id>` (`:262-270`). Those routes
    do not exist for us; `mapToTableItem` is not called there, so we cannot bend them either.
    Consequence: either we create the two routes and redirect to modrinth.com, or the rows in the
    modpack modal link into nothing. **To be decided when building the routes**, not by the
    interface.

---

## 2. The endpoints

Common rules: JSON, `snake_case`, everything under `/api/v1/`, sign-in through the session
cookie, timestamps RFC 3339 in UTC, ids are ULIDs.

Errors always as `{"error": "<code>", "message": "<text>"}`. The codes of this area:

| HTTP | `error` | Meaning |
|---|---|---|
| 400 | `invalid_request` | Body or query parameter unusable |
| 401 | `unauthenticated` | No session cookie, or an expired one |
| 403 | `forbidden` | Role is not enough |
| 404 | `server_not_found` | Server id unknown, or invisible to this user |
| 404 | `content_not_found` | At least one `id` does not belong to this server |
| 409 | `modpack_not_linked` | Modpack action without a linked modpack |
| 409 | `modpack_already_linked` | Installing a modpack although one is already linked |
| 409 | `server_busy` | A blocking operation is running on this server. Code, text and envelope are settled across areas (`docs/api/creation-and-progress.md`, 2.10) — **no** `content_busy` of our own |
| 409 | `server_running` | Action demands a stopped server (game version change only) |
| 413 | `file_too_large` | Uploaded file over the limit |
| 415 | `unsupported_file_type` | Neither `.jar` nor `.zip` nor `.mrpack` |
| 422 | `no_compatible_version` | No build for the loader and game version |
| 422 | `unresolvable_dependency` | Required dependency not resolvable |
| 422 | `invalid_mrpack` | `modrinth.index.json` is missing or unreadable |
| 429 | `modrinth_rate_limited` | Our own guard against Modrinth's rate limit kicked in |
| 502 | `modrinth_unavailable` | api.modrinth.com not reachable, or 5xx |
| 500 | `internal` | Everything else |

Permissions: `read` = bit **`BASE_READ`**, `write` = bit **`SETUP`**. The bits are no longer open
— the "Access" area has settled them (`docs/api/auth.md`, 1.2: `BASE_READ` = `1<<63`, `SETUP` =
`1<<59`, contained in the role `editor`, not in `viewer`). Modrinth checks the same bit for
exactly these actions (`ref/…/content.vue:134,174` via `useServerPermissions().canSetup`), and
"Settings" writes it too (`docs/api/settings.md:963`).

**Long-running operations do not belong to this area.** Every endpoint here that answers with
`202` uses the cross-area envelope `{ "operation": { … } }` from
`docs/api/creation-and-progress.md`, 2.10: the same `Operation` shape, the same states
(`queued`/`ongoing`/`done`/`failed`/`cancelled`), the same progress (`progress` as **0…1**, not
0…100 and not `current`/`total`), the same WS message `operations`, the same lookup resource
`GET /api/v1/servers/:id/operations/:op_id`. This area invents neither an operation resource of
its own nor a progress message of its own. The 202 blocks below show only the fields this area
settles; the rest are over there.

Two `OperationKind` values are still missing there and **have to be added**: `update_content` and
`change_game_version`. `install_content`, `install_modpack` and `repair_content` already exist
(`creation-and-progress.md`, 4.1); `install_content` blocks with the reason `syncing_content`
and lets the server keep running, `install_modpack` and `repair_content` block with `installing`
and demand a stopped server (ibid. 4.6).

---

### 2.1 `GET /api/v1/servers/:id/content`

The one query the whole page grows out of.

Query parameters:

| Name | Type | Default | Effect |
|---|---|---|---|
| `refresh_updates` | `bool` | `false` | `true` kicks off an update check in the background (section 4.4). The response does **not** wait for it. |

Permission: `read`.

Response `200`:

```json
{
  "content_type": "mod",
  "loader": "fabric",
  "loader_version": "0.16.10",
  "game_version": "1.21.4",
  "update_channel": "release",
  "updates_checked_at": "2026-08-12T09:14:03Z",
  "permissions": { "can_read": true, "can_write": true },
  "modpack": {
    "source_kind": "modrinth_modpack",
    "project_id": "1KVo5zza",
    "slug": "adrenaline",
    "title": "Adrenaline",
    "description": "A performance-focused modpack",
    "icon_url": "https://cdn.modrinth.com/data/1KVo5zza/icon.png",
    "filename": null,
    "downloads": 184203,
    "followers": 2210,
    "owner": {
      "id": "JZA4dW8o",
      "name": "modrinth",
      "type": "organization",
      "avatar_url": "https://cdn.modrinth.com/user/JZA4dW8o/avatar.png"
    },
    "categories": ["optimization", "utility"],
    "version_id": "Yc2Ph5nD",
    "version_number": "1.4.2",
    "date_published": "2026-07-30T18:02:11Z",
    "has_update": true,
    "update_version_id": "Kk3Vv1Qa"
  },
  "items": [
    {
      "id": "01J9T4V6Q3ZC5D8G2N7B0XKMER",
      "file_name": "sodium-fabric-0.6.9+mc1.21.4.jar",
      "file_path": "mods/sodium-fabric-0.6.9+mc1.21.4.jar",
      "size": 1187423,
      "enabled": true,
      "locked": false,
      "project_type": "mod",
      "date_added": "2026-08-01T11:20:44Z",
      "source_kind": "modrinth_modpack",
      "environment": "client_and_server",
      "pack_client_retained": false,
      "pack_client_depends": false,
      "installing": false,
      "external": false,
      "external_url": null,
      "has_update": true,
      "update_version_id": "AN2Ph2rT",
      "project": {
        "id": "AANobbMI",
        "slug": "sodium",
        "title": "Sodium",
        "icon_url": "https://cdn.modrinth.com/data/AANobbMI/icon.png"
      },
      "version": {
        "id": "Cs3Ph9nW",
        "version_number": "mc1.21.4-0.6.9-fabric",
        "file_name": "sodium-fabric-0.6.9+mc1.21.4.jar",
        "date_published": "2026-06-11T14:00:00Z"
      },
      "owner": {
        "id": "Ha3Rm1kL",
        "name": "jellysquid3",
        "type": "user",
        "avatar_url": "https://cdn.modrinth.com/user/Ha3Rm1kL/avatar.png"
      }
    },
    {
      "id": "01J9T4V6Q3ZC5D8G2N7B0XKMES",
      "file_name": "my-plugin.jar.disabled",
      "file_path": "mods/my-plugin.jar.disabled",
      "size": 40311,
      "enabled": false,
      "locked": false,
      "project_type": "mod",
      "date_added": "2026-08-05T20:03:00Z",
      "source_kind": "local",
      "environment": null,
      "pack_client_retained": false,
      "pack_client_depends": false,
      "installing": false,
      "external": false,
      "external_url": null,
      "has_update": false,
      "update_version_id": null,
      "project": null,
      "version": null,
      "owner": null
    }
  ]
}
```

Where a row's fields come from ("which fields does a row need?"):

| Field | Displayed in | Source in our backend |
|---|---|---|
| `project.title` | Row title (`ContentCardItem.vue:187`) | Modrinth project; with no origin `null` in the JSON, **the provider builds the substitute** (1.9, point 1) |
| `project.icon_url` | Icon (`ContentCardItem.vue:162`) | Modrinth, cached |
| `version.version_number` | Version column (`ContentCardItem.vue:283`) | Modrinth version of the file |
| `version.file_name` | Line under the version (`ContentCardItem.vue:294`) | File name on disk |
| `enabled` | Switch (`ContentCardItem.vue:355`) | Suffix `.disabled` |
| `owner` | Line under the title (`ContentCardItem.vue:236-245`) | Modrinth team of the project |
| `has_update` | Green arrow (`ContentCardItem.vue:316`) | Section 4.4 |
| `update_version_id` | Preselection in the modal (`ref/…/content.vue:1196`) | Section 4.4 |
| `date_added` | Sorting "Newest first" (`layout.vue:198-209`) | From our database row. For files that never went through the panel (file manager, shell) there is no row — then `mtime`. `std::fs::Metadata::created()` is not available everywhere on Linux and is no good as the only source |
| `project_type` | Type filter (`content-filtering.ts:73-84`) | `mod`, `plugin`, `datapack`, `resourcepack`, `shader` |
| `environment` | Warning triangle (`content-filtering.ts:12-21`) | `Labrinth.Versions.v3.Version.environment` (`api-client/src/modules/labrinth/types.ts:1520`) |
| `locked` | `canDeleteItem`/`canToggleItem` | `true` for the loader jar and the server core |

**No `busy` block.** `isBusy` and `busyMessage` come from `busyReasons` in the
`ModrinthServerContext`, fed from the WS message `operations`
(`docs/api/creation-and-progress.md`, 1.3 and 3.1). A second path for the same lock would be a
source of contradictory states; the earlier version had one.

**No pagination, and that is on purpose.** Neither `GET /content` nor 2.2 paginates. The layout
cannot do it: `useContentFilters` counts the type filters over **all** items
(`content-filtering.ts:73-84`), `useContentSearch` searches the complete set
(`layout.vue:222-226`), and `ContentCardTable` renders everything. A partial delivery would
silently falsify filters, counters and selection. The limit is drawn instead by the split from
2.2 (the modpack is not in the main list) plus a hard cap of **2,000 items** per response; above
that we deliver the first 2,000 and set `truncated: true`, so the page does not lie in silence.

Errors: `401 unauthenticated`, `403 forbidden`, `404 server_not_found`.

---

### 2.2 `GET /api/v1/servers/:id/content/modpack/contents`

What the linked modpack brought along. Feeds `ModpackContentModal.show(items)`
(`ModpackContentModal.vue:372`), which expects `ContentItem[]`.

Permission: `read`.

Response `200`:

```json
{
  "items": [
    {
      "id": "01J9T4V6Q3ZC5D8G2N7B0XKMER",
      "file_name": "sodium-fabric-0.6.9+mc1.21.4.jar",
      "file_path": "mods/sodium-fabric-0.6.9+mc1.21.4.jar",
      "size": 1187423,
      "enabled": true,
      "locked": false,
      "project_type": "mod",
      "date_added": "2026-08-01T11:20:44Z",
      "source_kind": "modrinth_modpack",
      "environment": "client_and_server",
      "pack_client_retained": false,
      "pack_client_depends": false,
      "installing": false,
      "external": false,
      "external_url": null,
      "has_update": false,
      "update_version_id": null,
      "project": {
        "id": "AANobbMI",
        "slug": "sodium",
        "title": "Sodium",
        "icon_url": "https://cdn.modrinth.com/data/AANobbMI/icon.png"
      },
      "version": {
        "id": "Cs3Ph9nW",
        "version_number": "mc1.21.4-0.6.9-fabric",
        "file_name": "sodium-fabric-0.6.9+mc1.21.4.jar",
        "date_published": "2026-06-11T14:00:00Z"
      },
      "owner": null
    },
    {
      "id": "01J9T4V6Q3ZC5D8G2N7B0XKMET",
      "file_name": "something-from-curseforge.jar",
      "file_path": "mods/something-from-curseforge.jar",
      "size": 233110,
      "enabled": true,
      "locked": false,
      "project_type": "mod",
      "date_added": "2026-08-01T11:20:45Z",
      "source_kind": "modrinth_modpack",
      "environment": null,
      "pack_client_retained": false,
      "pack_client_depends": false,
      "installing": false,
      "external": true,
      "external_url": "https://edge.forgecdn.net/files/1234/56/something.jar",
      "has_update": false,
      "update_version_id": null,
      "project": null,
      "version": null,
      "owner": null
    }
  ]
}
```

Why this is separate from 2.1: Modrinth splits the same way (`from_modpack=true|false`,
`api-client/src/modules/archon/content/v1.ts:10-36`, used in
`ref/…/content.vue:154-171`). The main list only carries the files the user installed **on top**.
Otherwise the list would be unusable with a 200-mod modpack, and the heading "Additional
content" (`layout.vue:794`) would be a lie.

Errors: `409 modpack_not_linked`, otherwise as 2.1.

---

### 2.3 `POST /api/v1/servers/:id/content/enable`
### 2.4 `POST /api/v1/servers/:id/content/disable`
### 2.5 `POST /api/v1/servers/:id/content/delete`

Three endpoints, one pattern. Always a list, even for one item — see 3.2.

Permission: `write`.

Request:

```json
{ "ids": ["01J9T4V6Q3ZC5D8G2N7B0XKMER", "01J9T4V6Q3ZC5D8G2N7B0XKMES"] }
```

Response `200` (partial success is possible and is reported):

```json
{
  "results": [
    {
      "id": "01J9T4V6Q3ZC5D8G2N7B0XKMER",
      "ok": true,
      "file_name": "sodium-fabric-0.6.9+mc1.21.4.jar.disabled",
      "file_path": "mods/sodium-fabric-0.6.9+mc1.21.4.jar.disabled",
      "enabled": false,
      "error": null,
      "message": null
    },
    {
      "id": "01J9T4V6Q3ZC5D8G2N7B0XKMES",
      "ok": false,
      "file_name": null,
      "file_path": null,
      "enabled": null,
      "error": "content_not_found",
      "message": "File was removed outside the panel"
    }
  ]
}
```

For `/delete`, `file_name`, `file_path` and `enabled` are absent (always `null`).

The provider throws an error only when **all** entries fail; otherwise it reports the individual
errors as a notification and reloads the list. Reason: `layout.vue:496,514` expects a resolved
`Promise` and reloads afterwards anyway; throwing on a single failure would swallow the remaining
successes.

Rename instead of move: enabling and disabling appends or removes the suffix `.disabled` — the
same convention Modrinth's desktop app reads
(`ref/apps/app-frontend/src/pages/instance/content/index.vue:639`). The `id` stays.

Errors: `400 invalid_request` (empty list), `403 forbidden`, `404 server_not_found`,
`409 server_busy`.

---

### 2.6 `POST /api/v1/servers/:id/content/update`

Updates existing content. Three call shapes in one endpoint.

Permission: `write`.

Request — single, with a chosen target version (from the modal):

```json
{
  "items": [
    { "id": "01J9T4V6Q3ZC5D8G2N7B0XKMER", "version_id": "AN2Ph2rT" }
  ],
  "all": false
}
```

Request — update a selection, each to the target version the server works out:

```json
{
  "items": [
    { "id": "01J9T4V6Q3ZC5D8G2N7B0XKMER", "version_id": null },
    { "id": "01J9T4V6Q3ZC5D8G2N7B0XKMEU", "version_id": null }
  ],
  "all": false
}
```

Request — "Update all":

```json
{ "items": [], "all": true }
```

Response `202` — envelope from `creation-and-progress.md`, 2.10, plus one field of this area:

```json
{
  "operation": {
    "id": "01J9T5A0K7YB3Q4M8P2R6WZTNC",
    "kind": "update_content",
    "state": "queued",
    "phase": "addons",
    "progress": 0,
    "message": "Updating 7 items"
  },
  "total": 7
}
```

`total` is the number of items the operation will touch: the provider needs it, because
`Operation.progress` is 0…1 and `BulkOperationStatus.progress` is a count
(`ContentSelectionBar.vue:55`: "Updating {progress}/{total}"). Working out which target version
applies to which item runs **synchronously before** the response; only that way can `total` be
right, and only that way can `404 content_not_found` and `422 no_compatible_version` show up as
HTTP errors at all. Only the downloading and laying out is the operation.

The operation runs in the background; progress arrives through the WS message `operations`
(section 3). For `bulkUpdateAll(onProgress)` the provider translates the snapshot into
`BulkOperationStatus` (`content-tab/types.ts:70-75`); that is exactly what the callback is for
(`layout.vue:695-700,712`).

**The returned `Promise` has to stay open until the operation ends.**
`layout.vue:704-722` sets `isBulkOperating = true` before the `await` and only resets it
afterwards; for that whole time a `beforeunload` handler is attached, along with a confirmation
when leaving the page (`bulk-operations.ts:41-66`). So the provider resolves only once the
operation reaches `done`, `failed` or `cancelled` in the `operations` snapshot. If the socket
breaks, it falls back to `GET /api/v1/servers/:id/operations/:op_id` every five seconds
(the same pattern as the server list, `creation-and-progress.md`, 3.2) and gives up with an
error after ten minutes without progress. Without this rule the page stays locked for good after
a connection drop.

Why asynchronous: an "Update all" over 40 mods downloads tens of megabytes. Modrinth's desktop
app does the same — one call, progress through events
(`ref/apps/app-frontend/src/pages/instance/content/index.vue:778`).

Errors: `403 forbidden`, `404 content_not_found`, `409 server_busy`,
`422 no_compatible_version`, `502 modrinth_unavailable`.

---

### 2.7 `POST /api/v1/servers/:id/content/install`

Installation from Modrinth. The client sends **a project id and optionally a version id** — no
more.

Permission: `write`.

Request:

```json
{
  "items": [
    { "project_id": "AANobbMI", "version_id": "Cs3Ph9nW" },
    { "project_id": "gvQqBUqZ", "version_id": null }
  ],
  "resolve_dependencies": true
}
```

Response `202`:

```json
{
  "operation": {
    "id": "01J9T5B4M2XD7F1H3K5N9QWRTV",
    "kind": "install_content",
    "state": "queued",
    "phase": "addons",
    "progress": 0,
    "message": "Installing 3 items"
  },
  "planned": [
    {
      "project_id": "AANobbMI",
      "version_id": "Cs3Ph9nW",
      "file_name": "sodium-fabric-0.6.9+mc1.21.4.jar",
      "reason": "requested"
    },
    {
      "project_id": "gvQqBUqZ",
      "version_id": "Rt7Yh2mK",
      "file_name": "lithium-fabric-0.14.3.jar",
      "reason": "requested"
    },
    {
      "project_id": "P7dR8mSH",
      "version_id": "Zz9Kk1vB",
      "file_name": "fabric-api-0.115.0+1.21.4.jar",
      "reason": "dependency"
    }
  ],
  "skipped": [
    {
      "project_id": "9s6osm5g",
      "version_id": null,
      "reason": "already_installed"
    }
  ]
}
```

**Who resolves dependencies: the server.** Reasoning:

1. The browser would have to call `/v2/project/{id}/version` per dependency and then filter by
   itself — for a mod with three dependencies that is four Modrinth requests out of the browser,
   each without a settable `User-Agent` (section 2.17).
2. The backend needs the same logic anyway: for `.mrpack` (section 4.5), for "Update all"
   and for the game version change (2.14).
3. Only the backend reliably knows what is already installed: it knows the files on disk, not
   just a list in browser storage.
4. One operation, one progress report, one error path. Modrinth's own web solution has to catch
   the case "install started, browser window closed" with `localStorage` and a queue
   (`ui/src/utils/server-content-installing.ts:10-45`,
   `browse-tab/composables/install-logic.ts:80-125`). We need none of that apparatus if the
   operation lives on the server.

Resolution rules (a rebuild of `versionMatchesCompatibilityTarget`,
`ui/src/utils/version-compatibility.ts:38-73`):

- `version_id` set → exactly that version, without a compatibility check. The modal has already
  warned the user at that point, but more weakly than the earlier version of this document
  claimed: for mods, incompatible versions only become visible after a click on "Show
  incompatible" (`content-updater-modal/index.vue:721`, `use-content-updater-filtering.ts`), they
  carry an orange badge (`index.vue:546-548`), and the footer holds the general sentence
  "Updating can break your world…" (`index.vue:357-360`). The explicit confirmation
  "Update to incompatible version?" (`index.vue:255-268`) appears **only for modpacks**
  (`index.vue:652-664`). The text `incompatibilityWarning` (`index.vue:361-365`) belongs to the
  mode `incompatibility-warning`, which this tab does not use. The rule stands nonetheless:
  what the user explicitly picks, we install.
- `version_id` empty → the newest version whose `game_versions` contains our game version and
  whose `loaders` match our loader; loader alias groups as in `version-compatibility.ts:3-6`
  (`paper`/`purpur`/`spigot`/`bukkit` and `neoforge`/`neo`); channel rules per
  `content-tab/utils/update-channels.ts:37-44` (`allowsUpdateChannel`), whose core is the
  fallback chain in `:26-35`. Note: `effectiveUpdateChannel` (`:16-24`) **raises** the default
  when the installed version is itself beta or alpha — for that the backend needs the
  `version_type` of the installed version, not just its id.
- Dependencies: only `dependency_type == "required"`. `embedded` is already inside the jar,
  and `optional` and `incompatible` we do not touch (types in
  `api-client/src/modules/labrinth/types.ts`, `Versions.v2.DependencyType`).
- `skipped[].reason` deliberately uses the same values as Modrinth's own resolver
  (`api-client/src/modules/labrinth/types.ts:41-50`): `already_installed`, `duplicate_project`,
  `conflicting_dependency`, `no_compatible_version`, `missing_version`, `quilt_fabric_api`.
  The last one is the special case "Quilt can do Fabric API itself" and has to be rebuilt.

Not used: Modrinth's resolver endpoint `POST /v3/content/resolve`
(`api-client/src/modules/labrinth/content/v3.ts:9-18`, deployed in
`ref/…/content.vue:390-404`). It would be convenient, but it is undocumented, it depends on
Modrinth being reachable, and in the end it returns only pairs of project id and version id that
we download ourselves anyway. See open question O-3.

Errors: `403 forbidden`, `409 server_busy`, `422 no_compatible_version`,
`422 unresolvable_dependency`, `502 modrinth_unavailable`, `429 modrinth_rate_limited`.

---

### 2.8 `POST /api/v1/servers/:id/content/upload`

Upload your own file. `multipart/form-data`, field name `file`, may repeat.

Permission: `write`.

Allowed extensions: `.jar` and `.zip` (Modrinth restricts the same way: `.zip` for datapacks,
otherwise `.jar`, `ref/…/content.vue:932`). `.mrpack` goes to 2.10.

Response `200`:

```json
{
  "results": [
    {
      "file_name": "my-plugin.jar",
      "ok": true,
      "id": "01J9T5C8N4ZE9G2J4M6P8SXUVW",
      "error": null,
      "message": null
    },
    {
      "file_name": "broken.txt",
      "ok": false,
      "id": null,
      "error": "unsupported_file_type",
      "message": "Only .jar and .zip"
    }
  ]
}
```

The "unknown file" warning (`UnknownFileWarningModal`, used in
`ref/…/content.vue:980-989`) happens **before** the upload, in the browser: build a SHA-1 over the
file and ask `GET /v2/version_file/{hash}?algorithm=sha1`; 404 means "not on Modrinth"
(`ref/…/content.vue:965-978`, client method
`api-client/src/modules/labrinth/versions/v2.ts:76-86`). That goes through our proxy
(2.17) and needs no endpoint of its own.

Limit: `max_upload_bytes`, the same setting and the same default (4 GiB) as in the file manager
(`docs/api/files.md:294,579,1099`). See O-6.

Errors: `403 forbidden`, `409 server_busy`, `413 file_too_large`, `415 unsupported_file_type`.

---

### 2.9 `POST /api/v1/servers/:id/content/dependents`

Serves `getDeleteDependencyWarning` (`content-manager.ts:73`). The question: if I delete these
files — which of the remaining content needs them?

Permission: `read`.

Request:

```json
{ "ids": ["01J9T4V6Q3ZC5D8G2N7B0XKMEU"] }
```

Response `200`:

```json
{
  "dependents": [
    {
      "id": "01J9T4V6Q3ZC5D8G2N7B0XKMER",
      "depends_on": ["01J9T4V6Q3ZC5D8G2N7B0XKMEU"]
    }
  ]
}
```

From that the provider assembles `ContentDependencyWarning`
(`content-manager.ts:30-36`): it already has the complete `ContentItem` objects in `items` and
only has to look them up. An empty list → return `null`, and then `layout.vue:379` skips the
modal.

Why server-side: for this, Modrinth's desktop app fetches **all** version objects of all
remaining content from Modrinth (`ref/apps/app-frontend/src/pages/instance/content/index.vue:719-723`,
`get_version_many`). With 200 mods that is a large request on every delete click. We have already
seen the dependency lists once, at install time, and we store them next to the file — the answer
then comes without a single Modrinth call.

Only `dependency_type == "required"` and `"embedded"` count, as in
`ref/apps/app-frontend/src/pages/instance/content/index.vue:695-696`.

**The call blocks the delete modal.** `layout.vue:374-378` awaits it before any window appears at
all, and swallows every error into `null` (`:377`). So it has to be fast and answerable offline —
that is the reason to write the dependency list down at install time instead of fetching it here.
On an error, deleting continues without a warning; that is Modrinth's behavior and we do not
change it.

Errors: `403 forbidden`, `404 content_not_found`.

---

### 2.10 `POST /api/v1/servers/:id/content/modpack/install`

Permission: `write`. The server has to be stopped.

Request — from Modrinth:

```json
{
  "source": {
    "kind": "modrinth",
    "project_id": "1KVo5zza",
    "version_id": "Yc2Ph5nD"
  },
  "keep_extra_content": false
}
```

Request — uploaded file: `multipart/form-data` with the field `file` (`.mrpack`) and the field
`meta` holding `{"source":{"kind":"upload"},"keep_extra_content":false}`.

Response `202`:

```json
{
  "operation": {
    "id": "01J9T5D2P6AF0H3K5N7Q9TYVWX",
    "kind": "install_modpack",
    "state": "queued",
    "phase": "analyzing",
    "progress": 0,
    "message": "Installing Adrenaline 1.4.2"
  }
}
```

`keep_extra_content: false` first deletes all content that does not come from the pack. That is
Modrinth's "Reinstall" behavior (text in `ConfirmReinstallModal.vue:60-64`). `true` leaves it
lying there (the behavior when updating, `ConfirmModpackUpdateModal.vue:93-97`).

Errors: `403 forbidden`, `409 modpack_already_linked`, `409 server_running`,
`409 server_busy`, `415 unsupported_file_type`, `422 invalid_mrpack`,
`502 modrinth_unavailable`.

---

### 2.11 `POST /api/v1/servers/:id/content/modpack/update`

Permission: `write`.

Request:

```json
{ "version_id": "Kk3Vv1Qa" }
```

`version_id` may be `null` → the newest version in the allowed channel. Downgrading is allowed;
the modal already warns (`ConfirmModpackUpdateModal.vue:87-97`, the direction is picked in
`content-updater-modal/index.vue:521-526`).

Response `202`:

```json
{
  "operation": {
    "id": "01J9T5E6R8BG1J4M6P8S0UWXYZ",
    "kind": "install_modpack",
    "state": "queued",
    "phase": "analyzing",
    "progress": 0,
    "message": "Laying out Adrenaline 1.5.0"
  }
}
```

What the backend does: build the new file list from the `.mrpack`, compare it with the old one,
delete removed files, replace changed ones, lay out `server-overrides/` and `overrides/` again.
Content the user added themselves stays untouched — that is what the modal's text promises
(`ConfirmModpackUpdateModal.vue:95-96`).

Errors: `403 forbidden`, `409 modpack_not_linked`, `409 server_busy`,
`422 invalid_mrpack`, `502 modrinth_unavailable`.

---

### 2.12 `POST /api/v1/servers/:id/content/modpack/unlink`

Permission: `write`. No body.

Response `200`:

```json
{ "unlinked": true, "adopted_items": 187 }
```

The files stay where they are but lose their origin: `source_kind` changes from
`modrinth_modpack` to `local`, and from now on they show up in the main list instead of in the
modpack modal. That is exactly what the modal's text promises (`ConfirmUnlinkModal.vue:76-78`:
"Mods and content will be merged with what you added on top of the modpack").

Errors: `403 forbidden`, `409 modpack_not_linked`, `409 server_busy`.

---

### 2.13 Repair — **no endpoint of its own**

The button sits in the settings area (`installation-settings/layout.vue:1070`) and serves
`InstallationSettingsContext.repair`
(`installation-settings/providers/installation-settings.ts:38`), and the "Settings" area has
already settled the endpoint: **`POST /api/v1/servers/:id/repair`**, permission `SETUP`
(`docs/api/settings.md:84,961-963`). A second endpoint `…/content/modpack/repair` next to it
would be exactly the duplication O-5 warned about; the earlier version of this section had it.

What this area contributes is only the behavior, not the path: according to the modal, "repair"
means reinstalling the loader and the Minecraft dependencies **without** deleting content
(`ConfirmRepairModal.vue:58-61`). With a linked modpack it also means checking every file from
the `.mrpack` (SHA-512 from the index against the file on disk) and re-fetching anything missing
or changed. Without a linked modpack only the loader is reinstalled.

Two settlements from outside apply here: the operation kind is called `repair_content`, it blocks
with `installing` and **demands a stopped server**
(`creation-and-progress.md`, 4.6), so the endpoint also answers with
`409 server_running`.

---

### 2.14 `GET /api/v1/servers/:id/content/game-version/preview`

Preview: what becomes incompatible if I change the game version?

Permission: `read`.

Query parameters: `game_version` (mandatory), `loader` (optional, default: the current one),
`loader_version` (optional).

Response `200`:

```json
{
  "new_game_version": "1.21.5",
  "new_loader": "fabric",
  "new_loader_version": "0.17.0",
  "has_unknown_content": true,
  "changes": [
    {
      "type": "updated",
      "id": "01J9T4V6Q3ZC5D8G2N7B0XKMER",
      "file_name": "sodium-fabric-0.6.9+mc1.21.4.jar",
      "project_id": "AANobbMI",
      "project_title": "Sodium",
      "project_icon_url": "https://cdn.modrinth.com/data/AANobbMI/icon.png",
      "current_version": { "id": "Cs3Ph9nW", "version_number": "mc1.21.4-0.6.9-fabric" },
      "new_version": { "id": "Bv4Nn8xQ", "version_number": "mc1.21.5-0.6.10-fabric" }
    },
    {
      "type": "removed",
      "id": "01J9T4V6Q3ZC5D8G2N7B0XKMEU",
      "file_name": "old-mod-1.21.4.jar",
      "project_id": "Fq2Ll5tR",
      "project_title": "Old Mod",
      "project_icon_url": null,
      "current_version": { "id": "Ww1Rr3yU", "version_number": "3.1.0" },
      "new_version": null
    }
  ]
}
```

That maps onto `ContentDiffPreview` without contortions
(`installation-settings/types.ts:38-63`); Modrinth does the same transformation in
`server-settings/pages/installation.vue:890-910` out of a response that has exactly these fields
(`api-client/src/modules/archon/types.ts:460-502`). It is not one to one, though, and that goes
both ways:

- **Unused** are `id`, `project_id`, `project_icon_url`, `new_loader` and the version ids.
  `ContentDiffItem` knows only `type`, `projectName`, `fileName`, `currentVersionName`,
  `newVersionName` (`types.ts:38-56`). We deliver them anyway, because our own page can show more
  later than Modrinth's does.
- **Missing** are `disabled` and `fileCount` (1.10, point 9).
- `ContentDiffPreview.newLoaderVersion` is **not** nullable (`types.ts:61`); our
  `new_loader_version: string | null` is mapped to `""` in the provider when the loader has no
  build number (Vanilla).
- `previewSave` returns `null` when there is nothing to report
  (`installation.vue:898`: an empty list **and** `has_unknown_content == false`); our response is
  allowed to be both at once, and the provider turns that into `null`.

`type` knows the values `added`, `removed`, `updated`;
`installation-settings/types.ts:39-48` additionally allows `modpack_linked`,
`modpack_updated`, `modpack_unlinked`, `game_version_updated`, `loader_updated`,
`config_files_updated` — we add those when the change also unlinks the modpack.

`has_unknown_content: true` means: there are files in the folder with no Modrinth origin that we
can say nothing about. The modal then warns in general terms.

The preview costs one Modrinth call per affected project. It can be cancelled
(`installation.vue:890,896`, `signal`), so our endpoint has to react cleanly to a client that
hangs up.

Errors: `400 invalid_request` (unknown `game_version`), `403 forbidden`,
`502 modrinth_unavailable`.

---

### 2.15 `POST /api/v1/servers/:id/content/game-version`

Permission: `write`. The server has to be stopped.

Request:

```json
{
  "game_version": "1.21.5",
  "loader": "fabric",
  "loader_version": "0.17.0",
  "incompatible_content": "update_then_disable"
}
```

`incompatible_content` decides what happens to content for which there is no matching build:

| Value | Effect | Counterpart at Modrinth |
|---|---|---|
| `update_then_disable` | Update where possible, otherwise disable | `applyGameVersionUpdate` (`installation.vue:595`) |
| `disable` | Update nothing, disable what is incompatible | `disableIncompatibleContent` (`installation-settings.ts:91`, implementation `installation.vue:826-856`) |
| `keep` | Touch nothing | `saveWithoutAutoFix` (`installation-settings.ts:97`, implementation `installation.vue:858-888`) |

Response `202`:

```json
{
  "operation": {
    "id": "01J9T5G4V2DJ3M6P8S0U2WYABC",
    "kind": "change_game_version",
    "state": "queued",
    "phase": "analyzing",
    "progress": 0,
    "message": "Switching to 1.21.5"
  }
}
```

Errors: `400 invalid_request`, `403 forbidden`, `409 server_running`, `409 server_busy`,
`502 modrinth_unavailable`.

---

### 2.16 Looking up an operation — **no endpoint of its own**

Looking up an operation after a page change or a connection drop is done by
`GET /api/v1/servers/:id/operations/:op_id` (`docs/api/creation-and-progress.md`, 2.3), and
`GET /api/v1/servers/:id/operations` returns all of them. The earlier version defined a second,
content-specific `…/content/tasks/:task_id` here, with a state name of its own (`running` instead
of `ongoing`) and a progress format of its own (`current`/`total` instead of `progress` 0…1).
That duplicated the cross-cutting area and is deleted; O-8 is thereby settled.

What remains here is the requirement on that area: the `OperationKind` list needs
`update_content` and `change_game_version` in addition to `install_content`, `install_modpack`
and `repair_content`.

---

### 2.17 `GET /api/v1/modrinth/*path` — the proxy

Passing read requests through to `https://api.modrinth.com`. The browser gets
`labrinthBaseUrl: '/api/v1/modrinth'` set; the configuration provides for it
(`api-client/src/core/abstract-client.ts:52,125,298-300`), and we change nothing about
`@modrinth/api-client` itself.

Permission: signed in (`read` on some server is not required — a valid session is enough; the
data is public anyway).

Only these `GET` patterns are allowed through:

| Path | What for |
|---|---|
| `/v2/search`, `/v3/search` | Search page (`browse-tab/composables/use-browse-search.ts:30`) |
| `/v2/project/{id}`, `/v3/project/{id}` | Project header, modpack card (`ref/…/content.vue:216-220`) |
| `/v2/projects?ids=[…]` | Several projects at once |
| `/v2/project/{id}/version` | Version list in the update modal (`content-updater-modal/index.vue:419`) |
| `/v2/version/{id}` | Changelog when selecting and hovering (`index.vue:473-475`) |
| `/v2/versions?ids=[…]` | Several versions |
| `/v2/version_file/{hash}` | Recognizing uploaded files (`ref/…/content.vue:973`) |
| `/v2/tag/game_version`, `/v2/tag/loader`, `/v2/tag/category` | Filter lists (`browse-manager.ts:16-20`) |
| `/v2/user/{id}`, `/v2/team/{id}/members` | Author display |

Everything else: `403 forbidden`. No `POST`, `PATCH`, `DELETE` — we are not a Modrinth client
with a sign-in.

The response is passed through unchanged (status code, body), with `Cache-Control` from our
cache. On a `429` from Modrinth we answer with `429 modrinth_rate_limited`, on network errors
with `502 modrinth_unavailable` — in our error format, so the client has to understand one thing
and not two.

**Why not straight from the browser?** Four reasons, in this order:

1. **The `User-Agent` can only be set reliably on the server.** Modrinth demands a meaningful
   `User-Agent`; the desktop app sets
   `modrinth/theseus/<version> (support@modrinth.com)`
   (`ref/apps/app-frontend/src/App.vue:222`). From the server we keep to that, from the browser
   it depends on the browser — the header is on `fetch`'s forbidden list, and whether it gets
   through is not our decision.
   *Correction to the earlier version:* it said the client sets the header "unconditionally"
   (`api-client/src/core/abstract-client.ts:373-376`). That is not true: line 374 is
   `if (userAgent)`, and `resolveUserAgent()` (`:381-384`) simply returns
   `config.userAgent`, which without configuration is `undefined`. So without a `userAgent` set,
   the client sends none at all. This point alone therefore does not carry the decision; the
   three that follow do.
2. **The rate limit can be steered centrally.** Modrinth's own web solution injects a rate limit
   key **only on the server** (`api-client/src/platform/nuxt.ts:54-57,227-228`,
   `import.meta.server`). With a proxy we have exactly one counter for all browser windows of all
   users and one shared cache; without it every tab counts for itself, and behind NAT the lock
   hits everybody at once.
3. **The browser does not necessarily have internet.** A panel typically runs on a machine with a
   network connection, and it is often operated from a walled-off network or over a VPN.
   The server has to download the jars anyway: it is the one that has internet.
4. **One cache, two beneficiaries.** The version list the update modal needs has already been
   fetched by the backend for the update check anyway (4.4). Without a proxy the browser fetches
   it a second time.

The price: we deviate from PLAN.md, which says `packages/api-client` will be used "only with
`labrinthBaseUrl` pointing at the original". The client stays unchanged — only the base URL
points at us instead of straight at Modrinth. That is a deliberate deviation and belongs in the
next version of the plan.

Errors: `401 unauthenticated`, `403 forbidden` (path not allowed through),
`429 modrinth_rate_limited`, `502 modrinth_unavailable`.

---

## 3. WebSocket messages

One socket per server under `/api/v1/servers/:id/ws`. This area adds **one** kind of message —
`content_changed` (3.3). Progress comes from the message that already exists.

### 3.1 Progress comes from `operations`, not from a message of our own

`operations` is the complete snapshot of all operations of a server, including `busy_reasons`
(`docs/api/creation-and-progress.md`, 3.1). Everything the content tab needs is in there:
`state`, `phase`, `progress` (0…1), `message`, `error`. The earlier version defined a second
message `content_task` here, with state names of its own (`running` instead of `ongoing`) and a
progress format of its own — two paths for the same state, which can drift apart. Deleted.

Mapping onto `BulkOperationStatus` (`content-tab/types.ts:70-75`) in the provider:

| Field over there | Value |
|---|---|
| `message` | `operation.message` |
| `total` | the number from the 202 response (`total`), or the length of the list passed in |
| `progress` | `Math.round(operation.progress * total)` — `Operation.progress` is 0…1, `BulkOperationStatus.progress` is a count (`ContentSelectionBar.vue:55`) |
| `waiting` | `operation.state === "queued" \|\| operation.phase === "analyzing"` |

`waiting: true` switches the bar to indeterminate (`ContentSelectionBar.vue:288`).

The phase names are not ours either: `analyzing`, `installing_loader`,
`installing_pack`, `addons` (`creation-and-progress.md`, 4.1). For content, `analyzing`
(resolving versions) and `addons` (downloading and laying out) are the two that occur. The
earlier proposal `resolving`/`downloading`/`applying`/`finishing` falls away; it leaned on
Modrinth's desktop app (`ref/apps/app-frontend/src/pages/instance/content/index.vue:753`), whose
text we do not use at all, because we send `message` ourselves.

### 3.2 What the progress bar really shows — and why

`bulk-operations.ts:12-39` (`runBulk`) counts `bulkProgress` up **only** when the layout itself
loops item by item — with a 250 ms pause in between (`bulk-operations.ts:18,28`).
And the layout only loops when we do **not** supply the bulk functions
(`layout.vue:489,523` for deleting, `555,590` for disabling, `616,634` for enabling).
If we supply them, the layout sets `bulkWaiting = true` (`layout.vue:494,560,621`) and the text
reads "Deleting content…" instead of "Deleting 3/7…" (`ContentSelectionBar.vue:53-60,168-182`).

Decision: **we supply the bulk functions.** Deleting 7 items is then one request instead of seven
plus six artificial pauses. The price is the indeterminate bar.

For updating the price falls away: `bulkUpdateAll` gets an `onProgress` callback
(`content-manager.ts:80`, called in `layout.vue:712`), which we feed from `operations`, and then
it reads "Updating 3/7…" again.

Two details that are otherwise missed when building. First: `bulkDeleteItems` is used only for
**more than one** item (`layout.vue:489`); for exactly one the layout calls `deleteItem`
(`layout.vue:509-514`). The endpoint still always gets a list. Second: `bulkEnableItems` is used
**without** a count check (`layout.vue:616`), and `bulkDisableItems` is used in the single case
too (`layout.vue:579-580`) — the three paths are not symmetrical in the layout, and our endpoint
makes the difference invisible.

### 3.3 `content_changed`

```json
{
  "type": "content_changed",
  "reason": "updates_checked"
}
```

`reason`: `updates_checked` (the update check from 4.4 is through) or
`external_change` (somebody changed something in the `mods/` folder through the file manager or
the shell). **No `task_finished`** — that an operation has finished is in the `operations`
snapshot, and that arrives anyway; a second trigger for it would mean reloading twice.

The provider then reloads `GET /content` — on `content_changed` and on every content operation
that reaches `done` or `failed` in the snapshot. **Except** when `isBulkOperating` is set
(`content-manager.ts:103`, maintained in `layout.vue:247-250`): a reload in the middle of a bulk
run would pull the rows out from under the user. That field, however, covers **only** bulk runs;
a single delete sets only `markChanging` (`layout.vue:512`). If a `content_changed` arrives at
that moment, a reload happens while the call is still running — harmless, because `getItemId` is
the ULID and the selection therefore stays stable (1.8), but that is the reason it has to be.

---

## 4. Data types

These declarations are meant to be taken over verbatim into
`web/src/api/content.ts` later.

### 4.1 Response types

```ts
export type ContentProjectType = 'mod' | 'plugin' | 'datapack' | 'resourcepack' | 'shader'

export type ContentSourceKindOwn = 'local' | 'modrinth_modpack' | 'server_project'

export type UpdateChannel = 'release' | 'beta' | 'alpha'

export interface ContentOwnerDto {
	id: string
	name: string
	type: 'user' | 'organization'
	avatar_url: string | null
}

export interface ContentProjectDto {
	id: string
	slug: string | null
	title: string
	icon_url: string | null
}

export interface ContentVersionDto {
	id: string
	version_number: string
	file_name: string
	date_published: string | null
}

export interface ContentItemDto {
	id: string
	file_name: string
	file_path: string
	size: number
	enabled: boolean
	locked: boolean
	project_type: ContentProjectType
	date_added: string
	source_kind: ContentSourceKindOwn
	environment: string | null
	pack_client_retained: boolean
	pack_client_depends: boolean
	installing: boolean
	external: boolean
	external_url: string | null
	has_update: boolean
	update_version_id: string | null
	project: ContentProjectDto | null
	version: ContentVersionDto | null
	owner: ContentOwnerDto | null
}

export interface ContentModpackDto {
	source_kind: 'modrinth_modpack' | 'local'
	project_id: string | null
	slug: string | null
	title: string
	description: string | null
	icon_url: string | null
	filename: string | null
	downloads: number | null
	followers: number | null
	owner: ContentOwnerDto | null
	categories: string[]
	version_id: string | null
	version_number: string | null
	date_published: string | null
	has_update: boolean
	update_version_id: string | null
}

export interface ContentPermissionsDto {
	can_read: boolean
	can_write: boolean
}

export interface ContentListResponse {
	content_type: ContentProjectType
	loader: string
	loader_version: string | null
	game_version: string
	update_channel: UpdateChannel
	updates_checked_at: string | null
	permissions: ContentPermissionsDto
	modpack: ContentModpackDto | null
	items: ContentItemDto[]
	/** true when the cap of 2,000 items kicked in (2.1). */
	truncated: boolean
}

export interface ModpackContentsResponse {
	items: ContentItemDto[]
}
```

### 4.2 Request and operation types

```ts
export interface ContentIdsRequest {
	ids: string[]
}

export interface ContentMutationResult {
	id: string
	ok: boolean
	file_name: string | null
	file_path: string | null
	enabled: boolean | null
	error: string | null
	message: string | null
}

export interface ContentMutationResponse {
	results: ContentMutationResult[]
}

export interface ContentUpdateTarget {
	id: string
	version_id: string | null
}

export interface ContentUpdateRequest {
	items: ContentUpdateTarget[]
	all: boolean
}

export interface ContentInstallTarget {
	project_id: string
	version_id: string | null
}

export interface ContentInstallRequest {
	items: ContentInstallTarget[]
	resolve_dependencies: boolean
}

export type ContentSkipReason =
	| 'already_installed'
	| 'duplicate_project'
	| 'conflicting_dependency'
	| 'no_compatible_version'
	| 'missing_version'
	| 'quilt_fabric_api'

export interface ContentPlanEntry {
	project_id: string
	version_id: string
	file_name: string
	reason: 'requested' | 'dependency'
}

export interface ContentSkippedEntry {
	project_id: string
	version_id: string | null
	reason: ContentSkipReason
}

export interface ContentInstallResponse {
	/** Shape and states: docs/api/creation-and-progress.md, 4.1. */
	operation: Operation
	planned: ContentPlanEntry[]
	skipped: ContentSkippedEntry[]
}

export interface ContentUpdateResponse {
	operation: Operation
	/** Number of items the operation touches — the denominator of the progress bar. */
	total: number
}

export interface OperationResponse {
	operation: Operation
}

export interface ContentUploadResult {
	file_name: string
	ok: boolean
	id: string | null
	error: string | null
	message: string | null
}

export interface ContentUploadResponse {
	results: ContentUploadResult[]
}

export interface ContentDependentEntry {
	id: string
	depends_on: string[]
}

export interface ContentDependentsResponse {
	dependents: ContentDependentEntry[]
}

export type ModpackSource =
	| { kind: 'modrinth'; project_id: string; version_id: string | null }
	| { kind: 'upload' }

export interface ModpackInstallRequest {
	source: ModpackSource
	keep_extra_content: boolean
}

export interface ModpackUpdateRequest {
	version_id: string | null
}

export interface ModpackUnlinkResponse {
	unlinked: boolean
	adopted_items: number
}

export type GameVersionChangeDiffType =
	| 'added'
	| 'removed'
	| 'updated'
	| 'modpack_unlinked'
	| 'game_version_updated'
	| 'loader_updated'
	| 'config_files_updated'

export interface GameVersionChangeVersion {
	id: string
	version_number: string
}

export interface GameVersionChangeEntry {
	type: GameVersionChangeDiffType
	id: string | null
	file_name: string | null
	project_id: string | null
	project_title: string | null
	project_icon_url: string | null
	current_version: GameVersionChangeVersion | null
	new_version: GameVersionChangeVersion | null
}

export interface GameVersionPreviewResponse {
	new_game_version: string
	new_loader: string
	new_loader_version: string | null
	has_unknown_content: boolean
	changes: GameVersionChangeEntry[]
}

export interface GameVersionChangeRequest {
	game_version: string
	loader: string | null
	loader_version: string | null
	incompatible_content: 'update_then_disable' | 'disable' | 'keep'
}

/**
 * No operation type of our own. `Operation`, `OperationKind`, `OperationState` and
 * `OperationPhase` come from docs/api/creation-and-progress.md, 4.1 and are imported.
 * This area needs two additional values in `OperationKind` over there:
 * `update_content` and `change_game_version`.
 */
```

### 4.3 WebSocket types

```ts
export interface ContentChangedMessage {
	type: 'content_changed'
	reason: 'updates_checked' | 'external_change'
}
```

### 4.4 The update check — who, how often, cached where

**Who:** the backend, and nobody else. The browser never asks Modrinth about updates; it reads
`has_update` and `update_version_id` from 2.1.

**How often:**

| Trigger | Behavior |
|---|---|
| `GET /content` without `refresh_updates` | Cache only. If it is older than 6 h, a check runs in the background and `content_changed` is sent afterwards. |
| `GET /content?refresh_updates=true` (the "Refresh" button, `layout.vue:928`) | The check is kicked off at once, the response does not wait. |
| After every installation or update | Only for the affected projects. |
| Background run | Every 6 h per server, staggered, only for running and recently used servers. |

**How the check works:** for every project with a known project id, `GET /v2/project/{id}/version`
with `include_changelog=false` (saves data; the client fetches the changelog itself later,
`api-client/src/modules/labrinth/versions/v2.ts:38-40`). From the list, pick the newest version
that
- has the game version in `game_versions` and whose `loaders` match the loader
  (`ui/src/utils/version-compatibility.ts:38-73`),
- satisfies the channel rules (`content-tab/utils/update-channels.ts:46-77`, the function
  `newestEligibleUpdate` — it is exported but is called neither in `ui/` nor in the desktop app;
  it exists precisely for whoever fills in the contract, and it is rebuilt in Rust),
- is newer than the installed one (`date_published`).

The channel is held per server in `update_channel` (Modrinth's desktop app has the same field per
instance, `ref/apps/app-frontend/src/helpers/types.d.ts:22`, set in
`ref/apps/app-frontend/src/pages/instance/components/settings-modal/general-settings.vue:102`).
It is written in the "Settings" area; here it is read-only.

**Cached where:** SQLite, four tables. The first two cover the update check, the last two cover
the fields that **every** row in 2.1 needs and that do not follow from a version list at all.

| Table | Key | Contents | Lifetime |
|---|---|---|---|
| `modrinth_project_versions` | `project_id` | JSON list of the versions, `etag`, `fetched_at` | 6 h, then refreshed with `If-None-Match` |
| `modrinth_version` | `version_id` | A single version including the changelog, `fetched_at` | 24 h |
| `modrinth_project` | `project_id` | `slug`, `title`, `icon_url`, `description`, `project_type`, `downloads`, `followers`, `team`, `fetched_at` | 24 h |
| `modrinth_project_owner` | `project_id` | `id`, `name`, `type`, `avatar_url` of the team owner, `fetched_at` | 7 days |

The last two are no accessory: `project.title`, `project.icon_url`, `owner` and the modpack
figures `downloads`/`followers` are in **no** version response. `project` comes from
`GET /v2/projects?ids=[…]` (one call for many), `owner` from
`GET /v2/team/{team}/members` — the entry with the owner role in there. Without these two tables,
opening the page with 40 mods costs 41 Modrinth calls.

**`environment` has a source of its own.** The field is v3
(`Labrinth.Versions.v3.Version.environment`, `api-client/src/modules/labrinth/types.ts:1520`);
the v2 version we fetch for the update check does **not** have it. Of it, only `client_only` and
`singleplayer_only` are read (`content-filtering.ts:10-14`). We fetch it with
`GET /v3/project/{id}` on first contact and store it in `modrinth_project`; as long as it is
missing, `environment: null` stays and the warning triangle stays away. The v2 substitute
`project.client_side`/`server_side` (`types.ts:1084-1085`) is **not** equivalent — it does not
know `singleplayer_only`.

The same cache serves the proxy from 2.17 — one call, two beneficiaries.

**Rate limit and `User-Agent`:**

- One shared token bucket for all outgoing Modrinth requests, set conservatively to
  300 requests a minute (Modrinth's documented limit; not provable from the code at hand, see
  O-4). `X-Ratelimit-Remaining` is read and the bucket is adjusted to it.
- On `429` and on `5xx` we retry with a growing wait — the same status codes Modrinth's own
  client retries (`api-client/src/features/retry.ts:85`:
  `[408, 429, 500, 502, 503, 504]`).
- `User-Agent` fixed to `<panelname>/<version> (+<repo-url>)`, following the pattern Modrinth's
  desktop app uses (`ref/apps/app-frontend/src/App.vue:222`:
  `modrinth/theseus/${version} (support@modrinth.com)`; `api-client/src/platform/tauri.ts:33` is
  only the same example in a doc comment).
- A background run over 200 projects is throttled to at most 60 requests a minute, so that
  interactive requests (search, version list) keep priority.

### 4.5 The `.mrpack` layout

**What is actually evidenced in the reference clone:** an `.mrpack` is a ZIP with a file
`modrinth.index.json`. What is evaluated there are `name`, `versionId` and `dependencies`
with the keys `minecraft`, `forge`, `neoforge`, `fabric-loader`, `quilt-loader`
(`ref/apps/frontend/src/helpers/infer/loader-parsers.ts:285-311` and
`ref/apps/frontend/src/providers/manage-server-compatibility-modal.ts:70-104`, which reads the
file out of the archive with JSZip).

**What is not in the reference clone:** the file list and the override folders. The reference
clone contains only `apps/frontend` and `apps/app-frontend`; the Rust side (`theseus`), which
actually unpacks a pack, is missing (`ls /root/ref-modrinth/apps`). I have refrained from
asserting the rest, and record it here as an **assumption from the published format description**
that has to be checked against a real `.mrpack` before implementation (O-1):

```
pack.mrpack (ZIP)
├── modrinth.index.json
├── overrides/            → always lay out
├── server-overrides/     → lay out on a server, overrides overrides/
└── client-overrides/     → ignore on a server
```

and, in the index, next to `formatVersion`, `game`, `name`, `versionId`, `summary` and
`dependencies`, a field `files` whose entries carry `path`, `hashes` (`sha1`, `sha512`),
`downloads` (a list of URLs), `fileSize` and optionally `env` with `client` and `server` (each
`required`, `optional` or `unsupported`).

Our rules follow from that:

1. Files with `env.server == "unsupported"` are **not** laid out. A server has no use for a
   client-only mod, and the third line in
   `commonMessages` warns about exactly that ("may cause issues when starting your server",
   `ui/src/utils/common-messages.ts:474-476`).
2. **`pack_client_retained` therefore always stays `false`.** According to the tooltip the field
   means: "a client mod that was installed as a dependency"
   (`ui/src/utils/common-messages.ts:478-482`). If we never lay out client mods in the first
   place, that case does not exist.
3. **`pack_client_depends`** we set to `true` when a laid-out file, according to its Modrinth
   version, has a required dependency on a project we left out because of rule 1. The tooltip
   fits ("This mod depends on a client-side mod",
   `common-messages.ts:483-487`) and the user finds out why the server may complain.
4. For non-Modrinth sources, `downloads` points at foreign servers. Those files get
   `external: true` and `external_url` (display: `ModpackContentModal.vue:304-316`). We download
   them, check the `sha512` from the index and refuse if it does not match.
5. The `sha512` per file is at the same time the basis for "repair" (2.13).

---

## 5. Open questions and assumptions

**O-1 — `.mrpack` format details unverified.** What is in the reference clone is evidenced in
4.5; the file list, `env` and the three override folders are an assumption. *To do:* at the start
of P3, download a real `.mrpack` (Adrenaline, for example), unpack it, hold `modrinth.index.json`
against 4.5 and correct this section. Nobody else decides this: it is pure verification.

**O-2 — the proxy instead of direct access deviates from the plan.** PLAN.md says the
`api-client` will be used "only with `labrinthBaseUrl` pointing at the original"; I point it at
our proxy (reasoning in 2.17). The client stays unchanged, only the base URL changes.
*A decision is needed* from whoever maintains the plan.

**O-3 — our own dependency resolution instead of `/v3/content/resolve`.** I decided for our own
(2.7). The counter-argument: Modrinth's resolver knows special cases we have to rebuild — the
list is in
`api-client/src/modules/labrinth/types.ts:41-50` and is our checklist. If it turns out while
building that `quilt_fabric_api` and `conflicting_dependency` are too much work, switching to
`POST /v3/content/resolve` is one line in the backend — the outward interface does not change.

**O-4 — the numeric value of the rate limit.** 300 requests a minute is Modrinth's documented
value, but it is **not** provable from the code at hand; all that is provable is that a limit
exists (`api-client/src/platform/nuxt.ts:54-57,227-228` with `x-ratelimit-key`,
`features/retry.ts:85` with 429).
*To do:* on the first real run, record `X-Ratelimit-Limit` and set the bucket accordingly.

**O-5 — overlap with the "Settings" area, partly settled.** As it stands:

- `repair` — **settled, against us.** `docs/api/settings.md:84,961-963` defines
  `POST /api/v1/servers/:id/repair`. Our duplicate endpoint is deleted (2.13).
- `reinstallModpack` — **settled, for us.** `settings.md:85` points explicitly at the
  content area; that is 2.10 with `keep_extra_content: false`.
- `updaterModalProps` — **settled, for us** (`settings.md:92`).
- The game version change (2.14/2.15) — **open.** The endpoints are here, the page is over there.
  As long as "Settings" does not define anything to the contrary, this version stands.
- `update_channel` — **open and homeless.** The field does not appear in `settings.md`.
  We read it in 2.1; there is no write path anywhere so far. *To decide:* either "Settings" takes
  it on, or we add a `PATCH …/content/settings` here.

**O-6 — the upload cap, settled.** The "Files" area has decided:
`max_upload_bytes` = 4 GiB, configurable, error code `413 file_too_large`
(`docs/api/files.md:294,579,1099`). We take the same value and the same code instead of our own
256 MiB — a panel with two upload limits would be a trap. The earlier number was made up and is
deleted.

**O-7 — a backup before deleting, settled.** Not through `AppBackupContext`: that branch is
unreachable as long as the server frame provides `ModrinthServerContext` (1.4). The Archon branch
of `useInlineBackup` applies (`use-inline-backup.ts:62-176`), and it is served by the adapter
from `docs/api/backups.md`, section 1. For this area there is nothing to do; the earlier
proposal `provideAppBackup` was wrong.

**O-8 — counting operations, settled.** The shared resource already exists:
`GET /api/v1/servers/:id/operations/:op_id` including the model, the states, progress and the
WS message (`docs/api/creation-and-progress.md`, 2.1-2.6, 3.1, 4.1). This area has deleted its
duplication (`content_task`, `…/content/tasks/:id`, `content_busy`). **Exactly one small thing
remains open:** `OperationKind` over there needs `update_content` and `change_game_version`.

**O-9 — where `environment` comes from.** The field is v3, the update check fetches v2
(4.4). The proposal is a `GET /v3/project/{id}` per project on first contact. *To check:* whether
v3 carries the field at project level too or only per version — in the second case it costs one
call per installed version instead of per project, and then it is worth asking whether the
warning triangle is worth it.

**Assumption A-1 — the server keeps running during content changes.** Installing, enabling,
disabling, deleting and updating are allowed with the server running as well; they take effect
only after a restart. Only a modpack installation and a game version change demand a stopped
server (`409 server_running`). Modrinth handles it the same way — the content tab locks only on
`installing` and `syncing` (`ref/…/content.vue:174-198`), not on "running".

**Assumption A-2 — one content change at a time per server.** `409 server_busy` when an operation
is already running. Reason: two simultaneous runs in the same `mods/` folder are a source of
half-written files. The interface copes with that, because it evaluates `isBusy` anyway
(`layout.vue:283`). The lock itself is not ours: it comes out of `busy_reasons` and is enforced
across areas with `409 server_busy` (`creation-and-progress.md`, 4.6).

**Assumption A-3 — the update check does not lock.** The background run from 4.4 is **not** an
`Operation`, sets **no** `busy_reason` and triggers **no** `409`. Otherwise a delete click would
fail at random, only because a check happened to be running. It is pure reading at Modrinth plus
one write into the cache; it becomes visible solely through `content_changed` with
`reason: "updates_checked"`.

**Assumption A-4 — resolution runs synchronously, downloading asynchronously.** `planned`,
`skipped` and `total` in the 202 responses can only be filled if the version resolution happens
**before** the response; likewise `422 no_compatible_version` and `404 content_not_found` are
otherwise not HTTP errors but operation errors. An "Update all" over 40 mods thereby costs up to
40 Modrinth queries before the response — out of the cache (4.4), usually not a single one. If
that gets too slow, the alternative is to drop `planned` and `total` and push everything into the
operation; then the progress bar loses its denominator and `bulkUpdateAll` shows "Updating
content…" instead of "Updating 3/7…".
