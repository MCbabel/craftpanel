# Settings and loaders — the interface contract

As of 2026-08-12. Area: the five settings pages (`general`, `installation`, `network`,
`properties`, `advanced`), the loader/version selection, the loader sources and the danger zone.

All source references are relative to `vendor/modrinth/` or to the project root.
Checked against the vendored copy, not against the documentation.

---

## 0. A finding up front that shapes the whole section

The plan assumes the `shared/` layouts are addressed through contracts (`docs/PLAN.md:25-36`).
**For the settings that is only half true.**

| File | talks to |
|---|---|
| `ui/src/layouts/shared/installation-settings/layout.vue` | **only** `injectInstallationSettings()` (`layout.vue:45`) — a real contract |
| `ui/src/layouts/shared/server-settings/pages/properties.vue` | `injectModrinthClient()` (`properties.vue:312`), calls `client.archon.properties_v1.*` (`properties.vue:395`, `:524`) |
| `.../pages/advanced.vue` | `injectModrinthClient()` (`advanced.vue:233`), `client.archon.options_v1.*` (`advanced.vue:281`, `:388`) |
| `.../pages/network.vue` | `injectModrinthClient()` (`network.vue:242`), `client.archon.servers_v0.*Allocation*` (`network.vue:259`, `:312`, `:351`, `:369`) |
| `.../pages/general.vue` | `injectModrinthClient()` (`general.vue:165`), `servers_v0.updateName` (`general.vue:363`), subdomain (`general.vue:367`, `:381`), billing (`general.vue:254`, `:259`) |
| `.../pages/installation.vue` | `injectModrinthClient()` + `provideInstallationSettings(...)` (`installation.vue:107`, `:412`) — is itself the one that fills the contract |
| `ui/src/components/servers/SaveBanner.vue` | `client.archon.servers_v0.power` (`SaveBanner.vue:73`) |
| `ui/src/components/servers/ServerSettingsModal.vue` | the **host** of the five pages: provides `ServerSettingsContext` (`:81-87`), builds the tabs from `serverSettingsTabDefinitions` (`:93-94`) — and itself calls `servers_v0.get` (`:177`), `servers_v1.get` (`:186`), `properties_v1.getProperties` (`:196`), `options_v1.getStartup` (`:200`) |

Two decisions follow from this, and they carry everything else:

**Decision 0.1 — `installation.vue` gets rebuilt, `installation-settings/layout.vue` does
not.** At Modrinth, `installation.vue` is the one that fills the contract
(`installation.vue:412-912`), and it hard-wires `availablePlatforms: ['vanilla','fabric',
'neoforge','forge','quilt','paper','purpur']` (`installation.vue:480`) as well as direct calls to
`fill.papermc.io`, `api.purpurmc.org` and `launcher-meta.modrinth.com` (`installation.vue:294-333`).
Leaf, Folia and Velocity do not appear there. We write those ~500 lines ourselves; the layout
underneath, with its form, warning modals and diff view (1,119 lines), stays unchanged.

**Decision 0.2 — the client-bound pages get a client adapter instead of a rebuilt API.**
A subclass of our own of `AbstractModrinthClient` maps the eleven Archon paths that are actually
used onto ten targets of our own. This does **not** rebuild Archon's API: our HTTP surface is
designed freely, and the adapter is a table in the frontend. It is in section 1.5.

**Where the subclass hooks in — corrected.** `buildUrl(path, baseUrl, version)` is `protected`
and overridable (`api-client/src/core/abstract-client.ts:278`), but it sees **neither the method
nor the body nor `params`**; the query parameters are appended later, by the platform client
(`platform/generic.ts:53`, `platform/tauri.ts:69`, `:136`). Two of the transformations we need —
name out of the query into a JSON body, `PUT` → `PATCH` — are therefore **not** possible there.
So we hook into `request()` (`:122`, public and not abstract): that is where `path`,
`options.method`, `options.params` and `options.body` sit together. `buildUrl` stays untouched;
the only abstract method is `executeRequest` (`:411`) anyway.

Also corrected: Modrinth's desktop app is **no** model for rewriting paths.
`TauriModrinthClient` (`/root/ref-modrinth/apps/app-frontend/src/App.vue:221`, `:242`) is the
platform transport from the package itself (`api-client/src/platform/tauri.ts:42`) and talks to
the real Archon. It only shows that a subclass of your own is the intended route. Nobody there has
rewritten paths with one. That part we invent.

With that, `properties.vue` runs unchanged. `network.vue` runs unchanged apart from one addition
(1.4). `general.vue` and `advanced.vue` do get touched, because they contain surfaces we
explicitly do not want (subdomain, billing specifications, SFTP — `docs/PLAN.md:93-98`); the
calls that survive there go through the same adapter. Details in 1.4.

**One world per server.** `properties.vue:396` and `advanced.vue:282` set `enabled:
worldId !== null`; if `worldId` is null, nothing is ever loaded. The `worldId` ref
(`ui/src/providers/server-context.ts:39`) is therefore constantly **`"default"`** here.
Our endpoints have no world segment; the adapter cuts `/worlds/default` out. If another value
arrives, the backend answers `404 world_not_found`.

---

## 1. The provider contract

### 1.1 `InstallationSettingsContext` — field by field

Source: `ui/src/layouts/shared/installation-settings/providers/installation-settings.ts:14-117`,
types in `.../types.ts:3-63`. "Used in" is the evidence that the layout really reads the field —
rule 3.

| Field | Required by type | Used in | Where the value comes from |
|---|---|---|---|
| `loading` | yes | `layout.vue:579` | `isLoading` of the queries `GET /servers/:id` (overview area) and `GET /loaders` |
| `installationInfo` | yes | `layout.vue:591`, `:959` | assembled from `server.loader`, `server.mc_version`, `server.loader_version` (overview area) — three lines, the third drops out for Vanilla, as in `installation.vue:438-448`. A line with `value: null` is rendered by the layout as a loading bar (`layout.vue:596-600`), not as an empty value — so for Vanilla leave the line out, do not empty it |
| `isLinked` | yes | `layout.vue:585`, `:606` | `true` when a modpack is linked → `modpack !== null` from `GET /servers/:id/content` (**content area**) |
| `isBusy` | yes | `layout.vue:672` and 11 more places | `status === "installing"` (overview area) **or** `busyReasons.length > 0` — exactly like `installation.vue:203-216` |
| `busyMessage` | optional, needed in practice | `layout.vue:673`, `:826` | text of the first busy reason. If it is missing, `v-tooltip` gets `undefined` and **no** tooltip appears at all (`layout.vue:672`) — the user then sees a dead button with no reason |
| `skipNonEssentialWarnings` | optional | `layout.vue:47`, `:387`, `:406` | local user setting (localStorage), **no endpoint** |
| `modpack` | yes (`null` allowed) | `layout.vue:608-668` | content area |
| `currentPlatform` | yes | `layout.vue:184`, `:993`, `use-installation-form.ts:70` | `server.loader` in lowercase; our IDs are lowercase already (1.6) |
| `currentGameVersion` | yes | `use-installation-form.ts:71` | `server.mc_version` — that is what the field is called in the contract type (`api-client/src/modules/archon/types.ts:616`), **not** `game_version` |
| `currentLoaderVersion` | yes | `use-installation-form.ts:74`, `:379` | `server.loader_version` (build number as a string) |
| `availablePlatforms` | yes | `layout.vue:48-50`, `:811` | `GET /api/v1/loaders` → `loaders[].id` |
| `resolveGameVersions(loader, showSnapshots)` | yes | `use-installation-form.ts:36-38` | `GET /api/v1/loaders/:loader/game-versions`, filtered by `version_type` in the client — **synchronous**, see below |
| `resolveLoaderVersions(loader, gameVersion)` | yes | `use-installation-form.ts:40-42` | `GET /api/v1/loaders/:loader/game-versions/:game_version/builds` — **synchronous**, see below |
| `resolveHasSnapshots(loader)` | yes | `use-installation-form.ts:57` | the same response: `game_versions.some(v => v.version_type !== "release")` — **synchronous**, see below |
| `onGameVersionHover` | optional | `layout.vue:838` | nothing but prefetching the same build query; no endpoint of its own |
| `save(platform, gameVersion, loaderVersionId)` | yes | `use-installation-form.ts:217` | `POST /api/v1/servers/:id/install` |
| `repair()` | yes | `layout.vue:317` | `POST /api/v1/servers/:id/repair` |
| `reinstallModpack()` | yes | `layout.vue:328` | content area |
| `swapModpack()` | optional | `layout.vue:268` | content area (local modpack file only) |
| `unlinkModpack()` | yes | `layout.vue:339` | content area |
| `getCachedModpackVersions()` | yes | `use-installation-form.ts:427` | content area / the real `api.modrinth.com` |
| `fetchModpackVersions()` | yes | `use-installation-form.ts:450` | the real `api.modrinth.com` through `packages/api-client` |
| `getVersionChangelog(id)` | yes | `use-installation-form.ts:485`, `:499` | the real `api.modrinth.com` |
| `onModpackVersionConfirm(v)` | yes | `use-installation-form.ts:527` | content area |
| `updaterModalProps` | yes | `layout.vue:230`, `:1039-1045` | content area + `server.mc_version`/`server.loader` |
| `isServer` | yes | `layout.vue:719`, `:766`, `:1110` | the constant `true` |
| `isApp` | yes | `layout.vue:1042` | the constant `false` |
| `showModpackVersionActions` | optional | `layout.vue:196-200` | content area: `true` when the modpack comes from Modrinth |
| `isLocalFile` | optional | `layout.vue:202-206` | content area |
| `isManagedModpack` / `managedModpackWarning` | optional | `layout.vue:208-212`, `:683` | **drops out** — that is Modrinth's "Shared Instance"; we deliver `false`/nothing |
| `repairing` / `reinstalling` | optional | `layout.vue:741`, `:776` | local `ref`s on our own page, set around the call |
| `afterSave` | optional | `use-installation-form.ts:218` | not set: our `save` does not wait for the installation, the progress runs over WebSocket |
| `closeSettings` | optional | `layout.vue:295` | `ServerSettingsContext.closeModal` |
| `lockPlatform` | optional | `layout.vue:184` | `false` — changing the loader is allowed here |
| `hideLoaderVersion` | optional | `layout.vue:861` | `false`, **except** for `vanilla` (the layout checks that itself already) |
| `disableAllContent()` | optional | `use-installation-form.ts:132`, `:235` | content area. **If it is missing, the warning before a loader change drops out with nothing in its place** (`use-installation-form.ts:132`) |
| `disableIncompatibleContent(target)` | optional | `use-installation-form.ts:147`, `:306` | content area |
| `saveWithoutAutoFix(...)` | optional | `use-installation-form.ts:315` | `POST /api/v1/servers/:id/install` with `content_policy: "keep"` |
| `previewSave(...)` | optional | `use-installation-form.ts:173`, `:262` | content area (`ContentDiffPreview`) |
| `editingPlatformRef` / `editingGameVersionRef` | optional | `use-installation-form.ts:26-27` | the page's own `ref`s, so that the build queries follow reactively |

**What the layout silently assumes on top of that (rule 3):**

* **The three `resolve*` are synchronous.** Their signatures return arrays, not promises
  (`providers/installation-settings.ts:30-32`); they are called in `computed`
  (`use-installation-form.ts:36-42`, `:57`), in `cancelEditing` (`:380`) and in the middle of
  rendering (`layout.vue:132-146`). So they must **fetch nothing** and only read an already
  filled, reactive cache. Our page therefore has to hang the three queries on
  `editingPlatformRef`/`editingGameVersionRef` and return nothing but `query.data.value` from
  `resolve*` — exactly the reason those two refs are in the contract at all.
* `IncompatibleContentModal` and `ContentDiffModal` embed `InlineBackupCreator`
  (`IncompatibleContentModal.vue:29`, `ContentDiffModal.vue:128`, switched on through
  `layout.vue:1110`). It calls
  `client.archon.backups_queue_v1.create/list/cancelCreate`
  (`content-tab/composables/use-inline-backup.ts:130`, `:154`, `:163`). **The loader change
  therefore hangs on the backups area.** Without its endpoints the "Create backup" button in the
  warning dialog stays dead — the change itself keeps working.
* `loaderVersionEntries[selectedLoaderVersion]` is addressed by **index**
  (`use-installation-form.ts:214-216`). The order of our build list is therefore part of the
  contract: **newest build first**, because `selectedLoaderVersion` is set to `0` on every change
  (`use-installation-form.ts:87`, `:95`).
* **A consequence you have to know about:** `handleStartEditing` (`layout.vue:359-365`) does
  **not** look for the installed build. Open the editor and you stand on index 0 = the newest
  build, and `hasChanges` (`use-installation-form.ts:69-79`) reports a change immediately as soon
  as the server is not on the newest build. A click on "Save" therefore raises the build without
  the user having touched the selection. Only `cancelEditing` (`:384-387`) looks for the current
  entry, and falls back to 0 when it is missing. Our build list must therefore **contain** the
  installed build, even when the upstream source cleared it away long ago.
* `channelTag` is rendered for Paper only (`layout.vue:894`, `:910`). We deliver the field for
  Leaf anyway; showing it is one line in our own page (see gap L4).
  **Nobody** in the whole layout reads `stable` (checked: no occurrence outside the type file).
  We deliver it as raw material for our own display, not for the contract.
* Besides the contract, the layout also has an **emit**: `reset-server` (`layout.vue:344`,
  triggered from `IncompatibleContentModal` through `:347-356`, wired up in
  `installation.vue:17`). Our page has to handle it: it leads down the same road as the "Reset
  server" button (2.5.3), but `cancelEditing` runs first (`:353`), so the selection is already
  reset and the setup dialog has to ask for the target values again.

### 1.2 `ServerSettingsContext`

Source: `ui/src/layouts/shared/server-settings/providers/server-settings.ts:11-17`.

| Field | Used in | Where from |
|---|---|---|
| `isApp: Ref<boolean>` | `installation.vue:794`, `:805` | the constant `false` |
| `currentUserId: Ref<string \| null>` | not read in this area | session (`GET /api/v1/me`, accounts area) |
| `currentUserRole: Ref<string \| null>` | `installation.vue:266` (`=== 'admin'` shows "Reset to onboarding"), and `ServerSettingsModal.vue:91` | panel role from the session: `"admin"` or `"user"` (`docs/PLAN.md:308`) |
| `browseModpacks(args)` | `installation.vue:942` | router jump, no endpoint. `args.worldId` is `"default"` |
| `closeModal?` | `properties.vue:328`, `installation.vue:936`, `:971` | local |

### 1.3 What the pages read from `ModrinthServerContext`

Source: `ui/src/providers/server-context.ts:37-72`. The contract belongs to the overview area;
what stands here is only what **these** pages need from it. Otherwise they do not run.

| Field | Read in | Requirement on the data model |
|---|---|---|
| `serverId` | everywhere | ULID as a string |
| `worldId` | `properties.vue:396`, `advanced.vue:282`, `installation.vue:247` | constantly `"default"`, **never `null`** |
| `server` | `general.vue:174`, `network.vue:248`, `installation.vue:268`, `advanced.vue:320` | needs `name`, `net.ip`, `net.port`, `status`, `loader`, `loader_version`, **`mc_version`** — the contract type `Archon.Servers.v0.Server` knows no `game_version` (`api-client/src/modules/archon/types.ts:604-629`), and `advanced.vue:320` literally reads `server.value?.mc_version`. If our HTTP field is called `game_version`, the provider has to rename it; otherwise the Java preselection silently shows every version and `currentGameVersion` stays `""`, which means the form never becomes valid (`use-installation-form.ts:62`) |
| `powerState` | `properties.vue:447`, `SaveBanner.vue:53` | `"running" \| "stopped" \| "starting" \| "stopping" \| "crashed"`; `properties.vue:447-449` reloads the properties on every change |
| `busyReasons` | `properties.vue:282`, `general.vue:140`, `installation.vue:205` | list of busy reasons |
| `currentUserPermissions` | through `useServerPermissions()` (`composables/server-permissions.ts:92`) | 64-bit mask; bits in `server-permissions.ts:15-32`. **Do not send it as a JSON number:** the type is `number` (`archon/types.ts:631`), but masks like `BASE_READ` are ≥ 2⁶³ and JSON numbers are doubles. `parsePermissionString` (`server-permissions.ts:39-53`) also takes the name form `"BASE_READ\|ADVANCED"`; that is what the overview area delivers (`docs/api/servers.md:40`) |
| `isSyncingContent` | `installation.vue:205` | content area |
| `status === 'installing'` | `tabs.ts:55` hides the "Properties" tab | our `status` has to know the same value |

Permission bits this area checks (`composables/server-permissions.ts:15-32`,
evaluated in `:92-99`; `SERVER_ADMIN` overrides everything, `:69-80`):

| Bit | Value | What for here |
|---|---|---|
| `BASE_READ` | `1<<63` | all read access |
| `POWER_ACTIONS` | `1<<62` | "Save & restart" in the SaveBanner (`properties.vue:283`) |
| `ADVANCED` | `1<<57` | writing properties, startup options, ports |
| `SETUP` | `1<<59` | loader/version change (`installation.vue:216`) |
| `RESET_SERVER` | `1<<56` | reset the server, reset to first-time setup (`installation.vue:221`, `:273`) |
| `FILES_WRITE` | `1<<60` | server icon (`general.vue:77`) |

### 1.4 The four client-bound pages, call by call

**`properties.vue`** — runs unchanged as soon as the adapter is in place.

| Call | Where | Our endpoint |
|---|---|---|
| `properties_v1.getProperties(serverId, worldId)` | `properties.vue:395` | `GET /api/v1/servers/:id/properties` |
| `properties_v1.patchProperties(serverId, worldId, patch)` | `properties.vue:524` | `PATCH /api/v1/servers/:id/properties` |
| `servers_v0.power(serverId, 'Start'\|'Restart')` (SaveBanner) | `SaveBanner.vue:73` | `POST /api/v1/servers/:id/power` (overview area) |

**`advanced.vue`** — has to be changed: `advanced.vue:5-89` is the SFTP block we explicitly do
not want (`docs/PLAN.md:97-98`). The rest stays line for line.

| Call | Where | Our endpoint |
|---|---|---|
| `options_v1.getStartup(serverId, worldId)` | `advanced.vue:281` | `GET /api/v1/servers/:id/startup` |
| `options_v1.patchStartup(serverId, worldId, patch)` | `advanced.vue:388` | `PATCH /api/v1/servers/:id/startup` |
| `server.sftp_host/_username/_password` | `advanced.vue:31`, `:47`, `:64` | **drops out** |

**`patchStartup` returns `void`** (`api-client/src/modules/archon/options/v1.ts:25-36`): the
client throws the body of the `PATCH` response away. Everything the interface is meant to see
therefore has to be in the `GET` as well — `advanced.vue:394-395` reloads after saving anyway.
That concerns `stripped_flags` in particular (E2).

**A new surface we add: the memory slider.** Today `advanced.vue` has exactly three controls —
startup command (`:114-122`), Java version (`:139-160`), Java runtime (`:177-186`). For
`memory_mib` there is **none**. At Modrinth the value is a matter of the plan you pay for; here
it is the main adjusting screw (`docs/PLAN.md:254-256`), so a slider goes between startup command
and Java version, capped by `memory_max_mib`, and `hasUnsavedChanges` (`:379-384`) as well as
`syncFormFromData` (`:363-367`) each get one more line. Without this addition, `memory_mib` and
`memory_max_mib` are dead in the contract.

Also to be changed: the hard-wired lists `JAVA_VERSIONS` (`advanced.vue:285-291`, values
8/11/17/21/25) and `JRE_VENDORS` (`:341-345`, `corretto`/`temurin`/`graal`) are filtered against
`GET /api/v1/java-runtimes`, otherwise the interface offers runtimes that do not exist on the
machine. The preselection by game version (`advanced.vue:317-339`) stays as it is — it agrees
with Mojang's `javaVersion.majorVersion` (checked: 1.21.8 → 21).

**`network.vue`** — runs unchanged apart from one addition. The DNS block
(`network.vue:149-210`) works with nothing but `net.ip`/`net.port` and needs no endpoint. There
is no "make primary" button: `network.vue:118` hides rename and delete for the primary row (the
copy button `:111-117` stays). That is what we add.

**Two traps in this file that the addition would not survive otherwise.**

1. `serverPrimaryPort` is a **snapshot**: `ref(data?.value?.net?.port ?? 0)`
   (`network.vue:249`), with no `watch` — unlike `general.vue:177-182`, which keeps its refs up
   to date. If `server` is not there yet while the page is being built, the primary row stays at
   `0`; after a swap it stays on the old port until the page is built again. Our version has to
   repair both, otherwise the table shows something wrong after `PUT …/primary`.
2. The same line applies to `serverIP` (`:248`).

| Call | Where | Our endpoint |
|---|---|---|
| `servers_v0.getAllocations(serverId)` | `network.vue:259` | `GET /api/v1/servers/:id/allocations` |
| `servers_v0.reserveAllocation(serverId, name)` | `network.vue:312` | `POST /api/v1/servers/:id/allocations` |
| `servers_v0.updateAllocation(serverId, port, name)` | `network.vue:369` | `PATCH /api/v1/servers/:id/allocations/:port` |
| `servers_v0.deleteAllocation(serverId, port)` | `network.vue:351` | `DELETE /api/v1/servers/:id/allocations/:port` |
| — (new) | — | `PUT /api/v1/servers/:id/allocations/:port/primary` |

The table assembles the primary port from `server.net.port` and appends the remaining
allocations (`network.vue:269-281`). Our `GET` therefore delivers **only the non-primary ones**.
Otherwise the primary port is in the list twice.

**And it delivers a bare list, not an envelope.** `getAllocations` returns
`Archon.Servers.v0.Allocation[]` (`api-client/src/modules/archon/servers/v0.ts:204-210`),
and `network.vue` treats the response as an array: `.map` in `allocationRows`
(`network.vue:275`), `.find` in `showEditAllocationModal` (`:332`). The adapter rewrites paths
and methods only, not bodies. An object `{ allocations, primary_port, pool }` would tear the page
apart with `allocations.map is not a function`. `primary_port` is in `server.net.port` already;
the pool comes from an endpoint of its own in the accounts and administration area. The element
type there is `{ port, name }` — the page adds `primary` itself (`:273`, `:278`), so it is
superfluous in the response, though it does no harm.

**`general.vue`** — gets replaced. Of 421 lines, these carry weight here: the server name
(`general.vue:174`, saved through `servers_v0.updateName`, `:363`), the server icon
(`general.vue:75-79`, writes `/server-icon.png` through the file manager,
`edit-server-icon/EditServerIcon.vue:153-184`) and the info box. These drop out: subdomain
(`general.vue:29-72`, `:367`, `:381`), billing specifications (`general.vue:252-297`,
`:324-341`), `node.instance` (`general.vue:326`), `is_medal` (`general.vue:76`). The
preferences (`general.vue:193-226`) live in `localStorage` only anyway.
The name belongs to the overview area (`PATCH /api/v1/servers/:id`); I do not claim it.

### 1.5 The client adapter's rewrite table

A subclass of `AbstractModrinthClient` overrides `request()`
(`api-client/src/core/abstract-client.ts:122`; reasoning in 0.2) and maps:

| Archon path (what the contract code produces) | Our path |
|---|---|
| `GET /modrinth/v0/servers/{id}` | `GET /api/v1/servers/{id}` (overview area) |
| `GET /v1/servers/{id}` | `GET /api/v1/servers/{id}` (overview area) — the same response; nobody in the whole area reads `ServerFull`, but the call has to **succeed**, otherwise the host bails out |
| `GET /v1/servers/{id}/worlds/default/properties` | `GET /api/v1/servers/{id}/properties` |
| `PATCH /v1/servers/{id}/worlds/default/properties` | `PATCH /api/v1/servers/{id}/properties` |
| `GET /v1/servers/{id}/worlds/default/options/startup` | `GET /api/v1/servers/{id}/startup` |
| `PATCH /v1/servers/{id}/worlds/default/options/startup` | `PATCH /api/v1/servers/{id}/startup` |
| `GET /modrinth/v0/servers/{id}/allocations` | `GET /api/v1/servers/{id}/allocations` |
| `POST /modrinth/v0/servers/{id}/allocations?name=X` | `POST /api/v1/servers/{id}/allocations`, `{"name":"X"}` |
| `PUT /modrinth/v0/servers/{id}/allocations/{port}?name=X` | `PATCH /api/v1/servers/{id}/allocations/{port}`, `{"name":"X"}` |
| `DELETE /modrinth/v0/servers/{id}/allocations/{port}` | `DELETE /api/v1/servers/{id}/allocations/{port}` |
| `POST /modrinth/v0/servers/{id}/power` | `POST /api/v1/servers/{id}/power` |
| `POST /v1/servers/{id}/worlds/default/onboard` | `POST /api/v1/servers/{id}/reset-to-setup` |
| `POST /modrinth/v0/servers/{id}/name` | `PATCH /api/v1/servers/{id}` |
| `GET /modrinth/v0/subdomains/{s}/isavailable` | **not mapped** — we replace the page that calls it |

The two top rows come from the host: `ServerSettingsModal.vue:177` and `:186` fetch both server
objects **before** the tabs render, and wait in one `Promise.all` (`:191`). A path that is not
mapped ends up in the `catch` there (`:203-208`). Then the whole window stays empty and reports
"Failed to load server". They are listed here, even though the endpoint belongs to the overview
area, because the **rewrite** belongs here.

The last three rows are **dead** as soon as 0.1 is done: `onboard` is called only by
`installation.vue:954`, `name` only by `general.vue:363`, `isavailable` only by
`general.vue:367`: we build all three pages ourselves and then call our paths directly. They
stay in the table because otherwise nobody notices that they were checked. That leaves **eleven**
live rows: server object twice, properties twice, startup twice, allocations four times, `power`
once, onto ten targets, because the two server fetches share one.

Two more transformations the adapter makes: `POST`/`PUT` with the name in the query
(`api-client/src/modules/archon/servers/v0.ts:224`, `:237`) become a JSON body, and
`PUT` becomes `PATCH`. Each is one line, but in `request()`, not in `buildUrl()`.

### 1.6 Loader identifiers

Our IDs are lowercase, because the layout expects `toLowerCase()` everywhere
(`installation.vue:268`, `:477`) and `formatLoaderLabel` (`ui/src/utils/loaders.ts:18`)
capitalizes everything it does not know — `leaf` → "Leaf", `folia` → "Folia", `velocity` →
"Velocity" with no change to the library at all.

`vanilla` · `paper` · `folia` · `purpur` · `leaf` · `fabric` · `velocity` (first wave) ·
`neoforge` · `quilt` · `forge` (second wave).

Difference from Modrinth: their `Modloader` union calls NeoForge `neo_forge`
(`api-client/src/modules/archon/types.ts:349`) and knows neither Leaf nor Folia nor Velocity. We
do not use that type — our install call is our own.

### 1.7 Gaps named explicitly

* **L1 — `isLinked`, `modpack`, all modpack methods, `previewSave`, `disableAllContent`,
  `disableIncompatibleContent`.** I do not serve these; they belong to the content area. Without
  them the modpack branch of the layout (`layout.vue:606-795`) stays unused and the loader change
  gives no warning about incompatible content (`use-installation-form.ts:132`).
* **L2 — a backup before the change.** `InlineBackupCreator` needs the backups area (1.1).
* **L3 — `installationInfo`, `currentPlatform`, `currentGameVersion`, `currentLoaderVersion`,
  `isBusy`** come from the overview area's server object. With that I fix that this object
  carries `loader`, `loader_version`, `mc_version` and `status`. Otherwise the page can show
  nothing. `mc_version`, not `game_version`: that is what the field is called in the contract type.
* **L4 — a channel badge for Leaf.** The layout renders `channelTag` for Paper only
  (`layout.vue:894`). Leaf's `experimental` builds therefore get no badge as long as we do not
  change that line. I deliver the field anyway.
* **L5 — the server icon.** `EditServerIcon` writes through the file manager
  (`EditServerIcon.vue:153-184`, `kyros.files_v0.uploadFileWithAuth`). That hangs on the files
  area, not on me.
* **L6 — `-Xmx` in the free-form startup command.** See decision E2: the field stays freely
  writable, the backend cleans up. Modrinth's page has no interface that shows this *beforehand*;
  our version has to show the cleaned command after saving —
  `advanced.vue:394-395` already reloads for that.

---

## 2. The endpoints

Common rules: JSON, `snake_case`, everything under `/api/v1/`, session cookie, ULIDs,
RFC 3339 timestamps in UTC, errors as `{ "error": "<code>", "message": "<text>" }`.

Always possible and therefore not repeated at every endpoint:
`401 unauthorized` · `403 forbidden` (bit missing) · `404 server_not_found` ·
`500 internal_error`.

### 2.1 `server.properties`

#### `GET /api/v1/servers/:id/properties`

Permission: `BASE_READ`.

Reads the file fresh from disk on every call. If it is missing (Velocity, or the server has never
started), both objects are empty and the status is `200` all the same. The page then shows its
warning (`properties.vue:429`, `:5-9`).

Response `200`:

```json
{
  "known": {
    "allow_cheats": "false",
    "allow_flight": "false",
    "difficulty": "normal",
    "enforce_whitelist": "false",
    "force_gamemode": "false",
    "gamemode": "survival",
    "generate_structures": "true",
    "generator_settings": "{}",
    "hardcore": "false",
    "level_seed": "",
    "level_type": "minecraft:normal",
    "max_players": "20",
    "max_tick_time": "60000",
    "motd": "A Minecraft Server",
    "pause_when_empty_seconds": "60",
    "player_idle_timeout": "0",
    "require_resource_pack": "false",
    "resource_pack": "",
    "resource_pack_id": "",
    "resource_pack_sha1": "",
    "simulation_distance": "10",
    "spawn_protection": "16",
    "sync_chunk_writes": "true",
    "view_distance": "10",
    "white_list": "false"
  },
  "custom": {
    "enable-command-block": "false",
    "enable-rcon": "false",
    "online-mode": "true",
    "query.port": "25565",
    "server-port": "25565",
    "spawn-monsters": "true"
  },
  "restart_required": false
}
```

The 25 keys under `known` are **exactly** the list from `properties.vue:333-359`; it is nailed
down in the vendored code, so it is binding for the backend too. In the file they carry hyphens,
in the contract underscores (`spawn-protection` → `spawn_protection`); this rewriting applies
**only** to these 25. `custom` keeps the file's raw spelling, because arbitrary foreign keys live
there (`enable-command-block`, `query.port`).

`restart_required` is `true` when the running process started with an older version of the file.
Modrinth's page does not read the field; ours may.

#### `PATCH /api/v1/servers/:id/properties`

Permission: `ADVANCED`. `properties.vue:544` checks the same thing in the client already.

Request — changed keys only, split the way `properties.vue:499-520` builds it:

```json
{
  "known": {
    "difficulty": "hard",
    "hardcore": "true",
    "gamemode": "survival",
    "white_list": "true",
    "enforce_whitelist": "true",
    "spawn_protection": "0"
  },
  "custom": {
    "enable-command-block": "true",
    "spawn-monsters": null
  }
}
```

`null` deletes the line from the file. A key may sit in both buckets; **the name decides, not the
bucket**. Otherwise the contract breaks as soon as our list and the one in the vendored code
drift apart.

Response `200`: the same body as the `GET`, with the new state and `restart_required`.

Checks in the backend (the client checks **nothing**):

| Rule | Error |
|---|---|
| numeric key (`max_players`, `max_tick_time`, `pause_when_empty_seconds`, `player_idle_timeout`, `simulation_distance`, `spawn_protection`, `view_distance`) without an integer | `400 invalid_property_value` |
| `difficulty` not in `peaceful\|easy\|normal\|hard` | `400 invalid_property_value` |
| `gamemode` not in `survival\|creative\|adventure\|spectator` | `400 invalid_property_value` |
| boolean key neither `true` nor `false` | `400 invalid_property_value` |
| line break or null byte in the value | `400 invalid_property_value` |
| key is not `[A-Za-z0-9._-]+` | `400 invalid_property_key` |
| key is `server-port` or `query.port` | `409 property_is_panel_owned`; `message` points at the allocations |
| the server has no `server.properties` and the loader is a proxy | `409 properties_unsupported` |

`message` always names the key: `"max_players must be an integer"`.

**Two keys belong to the panel.** `server-port` and `query.port` are written by
`PUT …/allocations/{port}/primary` (2.3). Without the lock above, the same file would have two
writers and no order between them. Nobody can trigger this from the interface anyway — see the
next paragraph — but the API is open to callers outside it as well.

**The `custom` bucket has no editor on this page.** Only the basic block and the ten keys from
`ADVANCED_GROUPS` (`properties.vue:365-381`, filtered in `:552-557`) are rendered; for everything
else the page explicitly points at the file manager (`:251-260`). `buildPatch` can therefore
almost never fill a `custom` field. Two consequences: first, our `PATCH` has to **leave every
line of the file it was not told about unchanged** — the page sends no full state, only a
difference; second, the `custom` branch of the contract is a reserve for surfaces of our own, not
a requirement from Modrinth's page. It never sends a `null` as a value either (`:499-520` writes
strings only); deleting is our addition.

**What a running server does with the file.** Minecraft reads `server.properties` at startup and
**writes it again on a clean shutdown** — from its in-memory image. Every change the panel writes
into a running instance is gone at the next stop. `restart_required` alone is therefore half the
truth. The ruling: if the server is running, the backend writes the file immediately all the same
(the interface expects that, `properties.vue:447-449` reloads on a change of the power state),
but also remembers the change and **replays it after the stop**, before the process starts again.
Without that replay, "Save & restart" in the SaveBanner loses exactly the change you pressed it
for.

One point you have to know when writing the backend: the basic block of the page (game mode,
difficulty, max players, MOTD, flight, cheats, whitelist, spawn protection) is **always**
rendered, even when the key does not appear in the file at all: without an active search,
`isPropertyVisible` returns `true` unseen (`properties.vue:578-581`). So a `PATCH` creates keys
that were not there before, among them `allow_cheats`, which an ordinary `server.properties` does
not know. The backend writes them all the same: an unknown line does not bother the server, and a
rejection would be inexplicable to the user.

**Decision E1 — no schema from the server.** The client knows the keys itself
(`properties.vue:333-359`) and their display types (`toggle`/`number`/`text`), and it knows them
**statically, in the vendored code**. A schema endpoint would steer nothing the page has not
hard-wired already — it would be a second truth next to `KNOWN_PROPERTIES`. On the wire all
values are therefore strings; `server.properties` is a Java `.properties` file, which holds
nothing but text anyway. Type checking sits in the backend, because there it also applies to
calls from outside the interface.

### 2.2 Startup options

#### `GET /api/v1/servers/:id/startup`

Permission: `BASE_READ`.

```json
{
  "java_version": 21,
  "jre_vendor": "temurin",
  "java_path": "/var/lib/craftpanel/runtimes/temurin-21.0.12/bin/java",
  "memory_mib": 4096,
  "memory_max_mib": 8192,
  "extra_flags": ["-XX:+UseG1GC", "-XX:MaxGCPauseMillis=200"],
  "startup_command": "java -Xmx4096M -XX:+UseG1GC -XX:MaxGCPauseMillis=200 -jar server.jar nogui",
  "original_invocation": "java -Xmx4096M -jar server.jar nogui",
  "managed_flags": ["-Xmx4096M"],
  "stripped_flags": [],
  "restart_required": false
}
```

`java_version`, `jre_vendor`, `startup_command` and `original_invocation` are named exactly as in
`Archon.Content.v1.RuntimeOptions` (`api-client/src/modules/archon/types.ts:431-436`), so that
`advanced.vue:347-352` reads unchanged. The four remaining fields are our addition:

* `memory_mib` — the `-Xmx` the panel manages (`docs/PLAN.md:254-256`). Needs a new slider in
  `advanced.vue`, see 1.4.
* `memory_max_mib` — the upper bound for that slider: the owner's remaining budget plus what this
  server already has (`docs/PLAN.md:318-322`). For admins, the size of the machine
  (`docs/PLAN.md:354`).
* `extra_flags` — the flags that belong to the user.
* `managed_flags` — what the panel put into the command. With it the interface can explain why an
  `-Xmx` disappeared from the text field.
* `stripped_flags` — what the last `PATCH` removed from the typed command, empty after an
  unchanged save. **It is in the `GET`, not only in the `PATCH`**, because
  `options_v1.patchStartup` discards the response body (1.4); that is the only way the
  explanation reaches the interface at all. The value survives until the next `PATCH`.

`startup_command` is the assembled command that actually runs.
`original_invocation` is the same command with the loader definition's defaults and without
`extra_flags`; `advanced.vue:97-111` uses it to show the "Default" button.

#### `PATCH /api/v1/servers/:id/startup`

Permission: `ADVANCED`. For `memory_mib` the budget check on top.

```json
{
  "java_version": 21,
  "jre_vendor": "temurin",
  "memory_mib": 6144,
  "startup_command": "java -Xmx16G -XX:+UseZGC -jar server.jar nogui"
}
```

All fields optional; `null` for `java_version`/`jre_vendor` means "choose automatically again"
(that is how `advanced.vue:388-392` sends it).

Response `200`: the same body as the `GET`, **after** the cleanup:

```json
{
  "java_version": 21,
  "jre_vendor": "temurin",
  "java_path": "/var/lib/craftpanel/runtimes/temurin-21.0.12/bin/java",
  "memory_mib": 6144,
  "memory_max_mib": 8192,
  "extra_flags": ["-XX:+UseZGC"],
  "startup_command": "java -Xmx6144M -XX:+UseZGC -jar server.jar nogui",
  "original_invocation": "java -Xmx6144M -jar server.jar nogui",
  "managed_flags": ["-Xmx6144M"],
  "stripped_flags": ["-Xmx16G"],
  "restart_required": true
}
```

| Error | When |
|---|---|
| `400 invalid_java_version` | version not in the list from `GET /java-runtimes` |
| `400 invalid_jre_vendor` | vendor unknown |
| `404 runtime_not_installed` | the combination of version and vendor is not there and cannot be fetched |
| `400 invalid_startup_command` | empty command, quotes that cannot be split, line break |
| `403 memory_budget_exceeded` | the sum of the `-Xmx` across all of the owner's servers exceeds their budget (`docs/PLAN.md:318-322`); `message` names the budget and the demand |
| `400 memory_too_small` | below 512 MiB |
| `409 user_over_budget` | the owner is over budget already (`docs/PLAN.md:364-367`) |

**Decision E2 — `-Xmx` is cleaned up, not rejected.** At Modrinth the startup command is a free
text field (`advanced.vue:114-122`), and we keep that field. A `400` on every `-Xmx` would
trigger the only error message the page knows ("Failed to update server arguments",
`advanced.vue:405-408`). The user would not learn why. Instead the backend removes `-Xmx`,
`-Xms`, `-XX:MaxRAM*` and `-XX:MaxHeapSize` from `extra_flags`, reports them back under
`stripped_flags` and sets its own value. `advanced.vue:394-395` reloads after saving, so the user
sees the result at once. The throttling stays tight that way (`docs/PLAN.md:342-344`), without
the interface having to fight.

**Decision E3 — the startup command is a template, not a command line.** What gets stored is not
the string but its decomposition: Java path (from version and vendor), managed flags,
`extra_flags`, jar path and loader arguments. `argv` is built from that at startup and is never
assembled from an input (`docs/PLAN.md:191-192`). What the user types lands in `extra_flags` and
nowhere else; everything the backend does not recognize as a flag in the typed command (a
different jar name, `&&`, pipes) falls away and is likewise reported under
`stripped_flags`.

#### `GET /api/v1/java-runtimes`

Permission: signed in. Panel-wide, not per server.

```json
{
  "runtimes": [
    {
      "major": 21,
      "vendor": "temurin",
      "version": "21.0.12+8",
      "path": "/var/lib/craftpanel/runtimes/temurin-21.0.12/bin/java",
      "source": "managed",
      "installed": true
    },
    {
      "major": 17,
      "vendor": "temurin",
      "version": "17.0.13+11",
      "path": null,
      "source": "managed",
      "installed": false
    },
    {
      "major": 21,
      "vendor": "corretto",
      "version": "21.0.5.11.1",
      "path": "/usr/lib/jvm/java-21-amazon-corretto/bin/java",
      "source": "system",
      "installed": true
    }
  ],
  "default_major_for_game_version": 21
}
```

`installed: false` means: known and obtainable, but not on disk yet. If the user picks such a
runtime, the backend fetches it while saving; if that takes a while, it reports the progress over
`install_state` (section 3). `default_major_for_game_version` applies to the server whose page is
open at the moment, and is only set when `?server_id=` comes along.

### 2.3 Ports

Our model: an allocation is a port from the admin's pool plus a name. Exactly one port per server
is the primary one; it lands in `server.properties` as `server-port` (and `query.port`).

#### `GET /api/v1/servers/:id/allocations`

Permission: `BASE_READ`. **A bare list**, no envelope — the contract code calls `.map` on it
(`network.vue:275`, `:332`; return type `Allocation[]` in
`api-client/src/modules/archon/servers/v0.ts:204-210`). Delivered **without** the primary port,
because `network.vue:269-281` puts that one in front itself, out of `server.net.port`. Ascending
by port.

```json
[
  { "port": 25566, "name": "Dynmap" },
  { "port": 25567, "name": "Simple Voice Chat" }
]
```

The pool (`from`/`to`/`free`) does not belong here: the page does not show it, and the endpoint
that carries it lives in the accounts and administration area.

#### `POST /api/v1/servers/:id/allocations`

Permission: `ADVANCED`. Only a panel admin may set `port` (`docs/PLAN.md:350-353`); otherwise the
panel hands out the next free port from the pool. Search and insert run in **one** transaction
with a unique constraint on the port. Otherwise two concurrent calls grab the same number and
the second one fails only when the server starts.

```json
{ "name": "Dynmap" }
```

or, as an admin:

```json
{ "name": "Dynmap", "port": 8123 }
```

Response `201`:

```json
{ "port": 25566, "name": "Dynmap" }
```

| Error | When |
|---|---|
| `409 port_in_use` | the port belongs to a server in the panel already; `message` names no other server's name |
| `409 port_unavailable` | a foreign process holds the port; checked with a short bind attempt |
| `409 port_pool_exhausted` | nothing free is left in the pool |
| `403 port_out_of_pool` | port outside the pool, the caller is not an admin |
| `400 invalid_port` | not 1024–65535 |
| `400 invalid_name` | empty or longer than 32 characters (`network.vue:84` caps at 32) |
| `409 allocation_limit` | more than 8 allocations per server |

**Collisions** come in three kinds, and they are kept apart on purpose: taken by the panel
(`port_in_use`, fixable by releasing it), taken from outside (`port_unavailable`, the panel does
not know by whom), outside the pool (`port_out_of_pool`, a question of rights). The bind attempt
happens when the allocation is created, not at startup; a port that is taken at startup makes the
server start fail and is reported there (overview area).

#### `PATCH /api/v1/servers/:id/allocations/:port`

Permission: `ADVANCED`. Only the name can be changed: the port is the key.

```json
{ "name": "Map" }
```

Response `200`: `{ "port": 25566, "name": "Map" }`.
Errors: `404 allocation_not_found`, `400 invalid_name`.

#### `DELETE /api/v1/servers/:id/allocations/:port`

Permission: `ADVANCED`. Response `204`.

| Error | When |
|---|---|
| `404 allocation_not_found` | the panel does not know it |
| `409 primary_allocation` | the primary port cannot be deleted, only swapped |

The port goes back into the pool. Modrinth's warning text ("This cannot be reserved again",
`network.vue:36`) is wrong here and needs changing in our version.

#### `PUT /api/v1/servers/:id/allocations/:port/primary`

Permission: `ADVANCED`. No body.

Response `200`:

```json
{
  "primary_port": 25566,
  "allocations": [
    { "port": 25565, "name": "Previously primary" }
  ],
  "restart_required": true
}
```

The previous primary port stays with the server as an ordinary allocation. Otherwise a swap
would quietly lose it to the pool. The backend writes `server-port` and `query.port` into
`server.properties`; if the server is running, that only takes effect after the restart, hence
`restart_required: true`. Because a running server rewrites the file when it shuts down (2.1),
the same replay applies here. Errors: `404 allocation_not_found`, `409 already_primary`.

This endpoint does not exist in the contract code; it is served by the row we add to
`network.vue` (1.4). It therefore does **not** run through the adapter, but directly.

The admin sets the pool itself; that endpoint belongs to the accounts and administration area. I
only assume that it can be read as a closed interval.

### 2.4 The loader catalog

**Decision E4 — one unified endpoint, no upstream formats passed through.**
Five reasons, in the order of their weight:

1. The backend needs the same data anyway, to fetch the file and to check it. Speaking to the
   same upstream API twice — once in the browser for the display, once in Rust for the
   installation — would be two versions of the same knowledge.
2. The formats differ irreconcilably: Paper v3 delivers a bare list of objects with
   `downloads["server:default"].checksums.sha256`, Purpur `{builds:{all:[…]}}` with `md5`, Leaf
   the old Paper v2 format with `downloads.primary`, Fabric three separate lists, Mojang a
   manifest with a second round per version. `resolveLoaderVersions` (`use-installation-form.ts:40`)
   is a single function; every further upstream format is another branch inside it — Modrinth
   already has four for two loaders (`installation.vue:351-389`).
3. The second wave has no JSON at all: NeoForge and Forge deliver `maven-metadata.xml`
   (checked on 2026-08-12). Passing through something the browser cannot read does not work.
4. PaperMC asks for a `User-Agent` that says who you are; a browser cannot set it.
   (Checked: without one, `fill.papermc.io` currently answers `200` all the same. You should not
   rely on that.)
5. A cache in the backend keeps the upstream APIs out of it. Paper allows
   `cache-control: max-age=1800`; we adopt that (see below).

The price: our backend has to maintain four adapters. We pay it for the installation anyway.

**A collision with the overview area — has to be decided before anybody builds.**
`docs/api/servers.md:509-560` claims the same path `GET /api/v1/loaders` with a **different**
body (`{id, name, kind: 'server'|'proxy', needs_installer}`) and sets out
`GET /api/v1/loaders/:loader/versions` next to it — a nested response in the shape of
`LauncherMeta.Manifest.v0.Manifest`, because the creation wizard
(`CreationFlowOptions.getLoaderManifest`) expects exactly that shape. My version is two-stage
(`…/game-versions` and `…/builds`), because the settings layout has two separate, **synchronous**
resolvers. Both shapes are right for their own page and neither can be turned into the other
without loss. Three roads are open, and the decision is not mine alone:
(a) one shared catalog in **my** shape, out of which the wizard builds its nesting in the
frontend; (b) two paths with different names, a doubled truth in the backend;
(c) `GET /api/v1/loaders` once, with the union of both field sets — then `kind` has to carry the
four values from 4., not the two from `servers.md`. I hold (a) to be right, but it costs the
wizard a transformation.

#### `GET /api/v1/loaders`

Permission: signed in.

```json
{
  "loaders": [
    {
      "id": "vanilla",
      "name": "Vanilla",
      "kind": "vanilla",
      "install_kind": "download",
      "has_loader_versions": false,
      "supports_properties": true,
      "supports_content": false,
      "source": "mojang",
      "wave": 1
    },
    {
      "id": "paper",
      "name": "Paper",
      "kind": "server",
      "install_kind": "download",
      "has_loader_versions": true,
      "supports_properties": true,
      "supports_content": true,
      "source": "papermc",
      "wave": 1
    },
    {
      "id": "folia",
      "name": "Folia",
      "kind": "server",
      "install_kind": "download",
      "has_loader_versions": true,
      "supports_properties": true,
      "supports_content": true,
      "source": "papermc",
      "wave": 1
    },
    {
      "id": "purpur",
      "name": "Purpur",
      "kind": "server",
      "install_kind": "download",
      "has_loader_versions": true,
      "supports_properties": true,
      "supports_content": true,
      "source": "purpurmc",
      "wave": 1
    },
    {
      "id": "leaf",
      "name": "Leaf",
      "kind": "server",
      "install_kind": "download",
      "has_loader_versions": true,
      "supports_properties": true,
      "supports_content": true,
      "source": "leafmc",
      "wave": 1
    },
    {
      "id": "fabric",
      "name": "Fabric",
      "kind": "modloader",
      "install_kind": "download",
      "has_loader_versions": true,
      "supports_properties": true,
      "supports_content": true,
      "source": "fabricmc",
      "wave": 1
    },
    {
      "id": "velocity",
      "name": "Velocity",
      "kind": "proxy",
      "install_kind": "download",
      "has_loader_versions": true,
      "supports_properties": false,
      "supports_content": true,
      "source": "papermc",
      "wave": 1
    },
    {
      "id": "neoforge",
      "name": "NeoForge",
      "kind": "modloader",
      "install_kind": "installer",
      "has_loader_versions": true,
      "supports_properties": true,
      "supports_content": true,
      "source": "neoforged",
      "wave": 2
    },
    {
      "id": "quilt",
      "name": "Quilt",
      "kind": "modloader",
      "install_kind": "installer",
      "has_loader_versions": true,
      "supports_properties": true,
      "supports_content": true,
      "source": "quiltmc",
      "wave": 2
    },
    {
      "id": "forge",
      "name": "Forge",
      "kind": "modloader",
      "install_kind": "installer",
      "has_loader_versions": true,
      "supports_properties": true,
      "supports_content": true,
      "source": "minecraftforge",
      "wave": 2
    }
  ]
}
```

`availablePlatforms` is `loaders.filter(l => l.wave <= currentWave).map(l => l.id)`.
`supports_properties: false` for Velocity is the reason the "Properties" tab stays empty — the
server has no `server.properties` (`docs/PLAN.md:421`).

#### `GET /api/v1/loaders/:loader/game-versions`

Permission: signed in. Serves `resolveGameVersions` (`use-installation-form.ts:36`) and
`resolveHasSnapshots` (`:57`). Order: newest first.

```json
{
  "loader": "paper",
  "game_versions": [
    { "version": "1.21.11", "version_type": "release" },
    { "version": "1.21.11-rc3", "version_type": "snapshot" },
    { "version": "1.21.10", "version_type": "release" },
    { "version": "1.21.9", "version_type": "release" },
    { "version": "1.21.8", "version_type": "release" }
  ],
  "cached_until": "2026-08-12T15:30:00Z"
}
```

`version_type` is `"release"` or `"snapshot"`; for Vanilla and Fabric the distinction comes from
the source (Mojang `type`, Fabric `stable`), for Paper/Folia/Purpur/Leaf from the spelling
(`-rc`, `-pre`, `-snapshot` → `snapshot`), and for Velocity likewise (`-SNAPSHOT`).

For the installation page this endpoint replaces Modrinth's `injectTags().gameVersions`
(`installation.vue:486-488`), which pulls the full list from `api.modrinth.com` and then
intersects it with the loader's list (`installation.vue:492-518`). We intersect in the backend,
where the loader list sits anyway; the tags stay with the content area.

**Velocity has no game versions.** There we put the Velocity versions themselves onto this axis
(`3.5.1`, `3.4.0-SNAPSHOT`, …), because without a selected "game version" the form never becomes
valid (`use-installation-form.ts:62`). The build number stays the second axis. That is the same
split as with Paper and needs no special handling in the layout.

#### `GET /api/v1/loaders/:loader/game-versions/:game_version/builds`

Permission: signed in. Serves `resolveLoaderVersions`. **Newest build first** — index 0 is the
preselection (`use-installation-form.ts:87`).

```json
{
  "loader": "paper",
  "game_version": "1.21.8",
  "builds": [
    { "id": "60", "label": "Build 60", "stable": true, "channel_tag": null, "released": "2025-09-06T21:50:11Z" },
    { "id": "59", "label": "Build 59", "stable": true, "channel_tag": null, "released": "2025-09-05T18:22:04Z" },
    { "id": "58", "label": "Build 58", "stable": false, "channel_tag": "ALPHA", "released": "2025-09-04T11:07:56Z" }
  ],
  "cached_until": "2026-08-12T15:30:00Z"
}
```

`{id, label, stable, channel_tag}` matches `LoaderVersionEntry`
(`installation-settings/types.ts:29-36`) in substance — **but not literally.** The contract
writes `channelTag` in camelCase, our rule writes `snake_case` on the wire; the renaming is
unavoidable and belongs in our page, one line. Two more small things from the same corner: for
`channelTag` the contract type knows **no `null`**, only `'ALPHA' | 'BETA' | undefined`
(`layout.vue:132-146` does normalize a `null`, but the object itself would break the type, so we
leave the field out instead of setting it to `null`), and `released` is not in the contract at
all; it is raw material for our own display. `channel_tag` knows only `"ALPHA"`, `"BETA"` or
`null`, because `PaperChannelBadge` has no other values (`components/base/PaperChannelBadge.vue:23`).

**`released` is not to be had everywhere.** Paper, Folia, Velocity (`time`) and Leaf (`time`)
deliver a timestamp per build. Purpur's build list is `builds.all: string[]` — numbers only; a
date would cost **one extra call per build** there. Fabric's `/v2/versions/loader/{game}` has no
date at all. That is why `released` is **nullable**, and we do not fetch it afterwards; whoever
needs it fetches it during the installation, where the build is queried on its own anyway.

For `vanilla`, `builds` is empty; the layout does not ask there anyway (`use-installation-form.ts:63`).

**No pagination, on purpose.** Both lists go out in full: the resolvers are synchronous (1.1) and
the combobox's search field filters in the client. Order of magnitude, measured against the real
sources: Vanilla with snapshots about 900 game versions (~30 KB), Leaf 168 builds for `1.21.8`
alone, old Paper series in the four digits. Hence a hard ceiling: **500 builds per game version,
newest first**, older ones are cut off — with the exception that the installed build is always
included (1.1). If anything is cut, `truncated: true` is in the response.

Errors: `404 loader_not_found`, `404 game_version_not_found`,
`502 upstream_unavailable` (the upstream API does not answer and the cache is empty;
`message` names the source).

#### The sources, checked against the real APIs (2026-08-12)

| Loader | Version list | Build list | File | Checksum |
|---|---|---|---|---|
| Vanilla | `GET https://launchermeta.mojang.com/mc/game/version_manifest_v2.json` → `{latest:{release,snapshot}, versions:[{id,type,url,time,releaseTime,sha1,complianceLevel}]}` | none | second call on `versions[].url` → `downloads.server.url` | `downloads.server.sha1` |
| Paper | `GET https://fill.papermc.io/v3/projects/paper` → `{project:{id,name}, versions:{"1.21":["1.21.11", …]}}` (groups → list) | `GET /v3/projects/paper/versions/{v}/builds` → **a bare list** `[{id,time,channel,commits,downloads}]` | `downloads["server:default"].url` | `downloads["server:default"].checksums.sha256` |
| Folia | like Paper, project `folia` (fewer versions, from 1.19.4 on — confirmed) | like Paper | like Paper | like Paper |
| Velocity | like Paper, project `velocity`; the keys of the `versions` map are Velocity series (`4.0.0`, `3.0.0`, `1.1.0`, `1.0.0`) | like Paper | like Paper | like Paper |
| Purpur | `GET https://api.purpurmc.org/v2/purpur` → `{project,metadata:{current},versions:[…]}` (ascending!) | `GET /v2/purpur/{v}` → `{project,version,builds:{latest,all:[…]}}` (numbers as **strings**, ascending) | `GET /v2/purpur/{v}/{build}/download` | `GET /v2/purpur/{v}/{build}` → `md5` (**no** sha256) |
| Leaf | `GET https://api.leafmc.one/v2/projects/leaf` → `{project_id,project_name,version_groups,versions}` | `GET /v2/projects/leaf/versions/{v}/builds` → `{project_id,project_name,version,builds:[{build,time,channel,promoted,changes,downloads}]}` (ascending) | `GET /v2/projects/leaf/versions/{v}/builds/{b}/downloads/{name}` | `downloads.primary.sha256` |
| Fabric | `GET https://meta.fabricmc.net/v2/versions/game` → `[{version,stable}]` | `GET /v2/versions/loader/{game}` → `[{loader:{version,stable,maven,build,separator}, intermediary, launcherMeta}]` | `GET /v2/versions/loader/{game}/{loader}/{installer}/server/jar` (168 KB, starts as it is) | **none** — see below |

Three deviations from the plan that stood out:

* **Leaf's channels are called `default` and `experimental`**, not `stable`/`experimental`
  (`docs/PLAN.md:387`). Counted in `1.21.8`: 99 × `default`, 69 × `experimental`, `promoted`
  `false` everywhere. Mapping: `default` → `stable: true, channel_tag: null`; `experimental` →
  `stable: false, channel_tag: "ALPHA"`. The default stays the newest `default` build.
* **Leaf hangs the file under `downloads.primary`, not under `downloads.application`** the way
  old Paper v2 did. Otherwise the shape is identical — the adapter really is small.
* **Fabric publishes no checksum for the server jar.** The file is generated on every fetch. We
  check nothing there but TLS and size and write that into the loader definition, instead of
  pretending we had a hash.

The installer version for Fabric (`GET /v2/versions/installer` → `[{url,maven,version,stable}]`)
is chosen by the backend (newest `stable: true`) and does **not** appear in the interface.
Otherwise the version selection would have a third axis.

Second wave, also checked on 2026-08-12, for the completeness of the adapter table:
NeoForge `GET https://maven.neoforged.net/api/maven/versions/releases/net%2Fneoforged%2Fneoforge`
→ `{isSnapshot,versions:[…]}` (JSON, more pleasant than the `maven-metadata.xml` next to it),
installer at `…/releases/net/neoforged/neoforge/{v}/neoforge-{v}-installer.jar` (checked: 200);
Quilt `GET https://meta.quiltmc.org/v3/versions/loader` and `…/installer` →
`[{maven,version,url,file_size,hashes:{sha1,sha256,…}}]`; Forge
`GET https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json` →
`{homepage, promos:{"1.20.1-latest":"47.4.0", …}}`.

Cache: version and build lists 30 minutes (matches Paper's
`s-maxage=1800`), the Mojang manifest 10 minutes, downloaded files permanently under their
hash. `cached_until` is in every response, so that the interface does not have to guess.

### 2.5 Installation, repair, the danger zone

#### `POST /api/v1/servers/:id/install`

Permission: `SETUP`. Serves `save` and `saveWithoutAutoFix`
(`use-installation-form.ts:217`, `:315`).

```json
{
  "loader": "paper",
  "game_version": "1.21.8",
  "loader_version": "60",
  "content_policy": "keep",
  "wipe": false
}
```

`loader_version` is `null` for Vanilla. `content_policy` is `"keep"` (the default; world, mods and
configuration stay where they are) or `"wipe_mods"` (`mods/` and `plugins/` are moved aside).
`wipe: true` is the dangerous case from 2.5.4 and is allowed here only for completeness.

Response `202`:

```json
{
  "job_id": "01K2C3D4E5F6G7H8J9K0M1N2P3",
  "state": "installing",
  "steps": ["resolve", "download", "verify", "write_config", "done"]
}
```

The call does **not** wait. The progress comes over `install_state` (section 3), the end
additionally as a status change on the server, which `installation.vue:914-928` listens for
already.

Because the call returns at once, the **other direction** has to be tight: while a job is
running, `POST /api/v1/servers/:id/power` refuses with `409 server_installing`. The overview area
carries this code already (`docs/api/servers.md:841`); I rely on it. Without it a user starts the
server into the middle of a half-written installation.

| Error | When |
|---|---|
| `400 unknown_loader` | loader ID not in the catalog |
| `400 unsupported_game_version` | the loader does not know this game version |
| `404 build_not_found` | `loader_version` does not exist (any more) for this game version |
| `409 server_running` | the server is running; stop it first |
| `409 install_in_progress` | an installation is running already (`job_id` in the `message`) |
| `409 loader_change_needs_wipe` | a change between incompatible families (e.g. `fabric` → `paper`) without `content_policy: "wipe_mods"` |
| `502 upstream_unavailable` | the upstream source does not answer |
| `507 no_space` | too little room for the file |

`loader_change_needs_wipe` is the server-side counterpart to Modrinth's warning dialog
(`use-installation-form.ts:132-145`). The warning in the client is optional and hangs on the
content area (gap L1); the rule in the backend does not.

**Changes and incompatibility.** Four families: `vanilla` · the Bukkit descendants
(`paper`, `folia`, `purpur`, `leaf`) · modloaders (`fabric`, `quilt`, `neoforge`, `forge`) ·
proxy (`velocity`). Within one family the content stays where it is. Across family borders the
backend demands `content_policy: "wipe_mods"`; the world survives in both cases.
When changing **to** or **from** `velocity` the response additionally warns through
`warnings: ["properties_will_be_ignored"]`, because a proxy reads no `server.properties`.

#### `POST /api/v1/servers/:id/repair`

Permission: `SETUP`. Serves `repair` (`layout.vue:317`).

No body. Downloads the same loader file again, checks the checksum, rewrites `eula.txt` and the
managed parts of the configuration. Does **not** touch the world, the mods or `server.properties`.
That matches the text the interface shows (`layout.vue:479-481`).

Response `202`: as for `install`.
Errors: `409 server_running`, `409 install_in_progress`, `502 upstream_unavailable`.

#### `POST /api/v1/servers/:id/reset`

Permission: `RESET_SERVER`. At Modrinth the "Reset server" button (`installation.vue:25-35`)
opens the setup dialog, which at the end sends `installContent` with `soft_override: false`
(`ServerSetupModal.vue:166-171`). Here it is an endpoint of its own, because the damage it does
is a different one.

```json
{
  "loader": "paper",
  "game_version": "1.21.8",
  "loader_version": "60",
  "keep_backups": true
}
```

Deletes **the whole server folder** — world, mods, configuration, logs — and installs again.
Backups stay; the interface promises that literally (`installation.vue:127-130`), so
`keep_backups` is fixed at `true` and a `false` is rejected with `400 invalid_request`. The
field is in the contract all the same, so that nobody assumes it was forgotten.

Response `202`: as for `install`, plus `"reset": true`.
Errors: `409 server_running`, `409 install_in_progress`, `400 unknown_loader`.

#### `POST /api/v1/servers/:id/reset-to-setup`

Permission: `RESET_SERVER` **and** the panel role `admin`. Corresponds to
`servers_v1.resetToOnboarding` (`installation.vue:954`), which the page shows to panel admins
only (`installation.vue:266`, `:56`).

No body. Puts the server back into first-time setup: `flows.intro = true`
(`installation.vue:961`), clear the console buffer, discard the loader details. The files stay
where they are — this route is there to redo a setup that went wrong, not to clean up.

Response `200`:

```json
{ "server_id": "01K2BXYQ0ZC9F3H1V2M8R7T4KD", "flows": { "intro": true } }
```

Errors: `409 server_running`, `403 forbidden` (no panel admin role).

#### `DELETE /api/v1/servers/:id`

Permission: the server's owner **or** a panel admin. Modrinth does not have this — there a server
ends with the subscription. The button belongs in the layout's `#extra` area
(`layout.vue:1028`), next to "Reset server".

The request goes through the query, so that no body hangs off a `DELETE`:
`DELETE /api/v1/servers/01K2BXYQ0ZC9F3H1V2M8R7T4KD?keep_backups=false`

Response `202`:

```json
{
  "server_id": "01K2BXYQ0ZC9F3H1V2M8R7T4KD",
  "state": "deleting",
  "backups_kept": false
}
```

The directory is deleted, the ports go back into the pool, the entry disappears from the
database. The system account stays — it belongs to the panel user, not to the server
(`docs/PLAN.md:138-146`).

| Error | When |
|---|---|
| `409 server_running` | stop it first. No `force` — a running process whose directory breaks away underneath it is exactly the state you do not want |
| `409 install_in_progress` | wait until it has finished |
| `403 forbidden` | neither owner nor panel admin |

No two-step confirmation in the protocol: the dialog in front of it (`ConfirmModal`) is enough,
and a confirmation token would be an invention no other caller knows.

---

## 3. WebSocket messages

One socket per server at `/api/v1/servers/:id/ws`, typed messages. Three messages belong to this
area. All three have a model in Modrinth's sync channel
(`api-client/src/modules/archon/types.ts:954-1006`), so none of them is invented.

**`install_state`** — progress of `install`, `repair`, `reset` and of fetching a Java runtime.
Modrinth knows nothing for this but the coarse status change; we need more, because the second
wave runs an installer that takes minutes.

```json
{
  "type": "install_state",
  "job_id": "01K2C3D4E5F6G7H8J9K0M1N2P3",
  "state": "installing",
  "step": "download",
  "progress": 0.42,
  "message": "paper-1.21.8-60.jar"
}
```

`state` is `"installing"`, `"available"` or `"broken"` — the same values as
`Archon.Servers.v0.Status` (`types.ts:581`), because `tabs.ts:55` and `installation.vue:922`
check for them. `step` is one of `resolve` · `download` · `verify` · `run_installer` ·
`write_config` · `done`. `progress` is `0.0`–`1.0`, or `null` when the length is unknown.
The output of a second-wave installer does **not** go through here but into the ordinary console
stream. Otherwise there would be two routes for the same thing.

**A second collision with the overview area.** `docs/api/servers.md:878-884` already carries
`InstallProgress` on the same socket, with `phase: resolving|downloading|installing|modpack|addons`
and `percent: 0..100`. My message has `step` with six different names and `progress: 0.0–1.0`.
Two progress messages for the same operation on one socket are one too many; the same holds as
for the loader catalog — one shape wins, and the choice is not mine alone.
My names additionally cover `verify` and `run_installer`, which the second wave needs.

**`startup_changed`** — after every `PATCH /startup`, even when another participant in the
session triggered it. Model: `world.startup.patch` (`types.ts:975-981`), which in the client
discards the query `['servers','startup','v1',serverId]` (`composables/server-panel-sync.ts:182`).

```json
{
  "type": "startup_changed",
  "java_version": 21,
  "jre_vendor": "temurin",
  "memory_mib": 6144,
  "startup_command": "java -Xmx6144M -XX:+UseZGC -jar server.jar nogui",
  "original_invocation": "java -Xmx6144M -jar server.jar nogui",
  "restart_required": true
}
```

**`network_changed`** — after every change to the allocations. Model:
`server.network.patch` (`types.ts:959-962`).

```json
{
  "type": "network_changed",
  "primary_port": 25566,
  "allocations": [
    { "port": 25565, "name": "Previously primary" },
    { "port": 25567, "name": "Simple Voice Chat" }
  ]
}
```

Explicitly **no** message for `server.properties`: the page reloads on every change of the power
state already (`properties.vue:447-449`), and two editors sitting on the same file at the same
time is not a case we want to solve. Whoever saves last wins; that is the same behavior as at
Modrinth.

---

## 4. Data types

```ts
// ------------------------------------------------------------------- common

export type Iso8601 = string
export type Ulid = string

export interface ApiError {
	error: string
	message: string
}

// -------------------------------------------------------------- properties

/** The 25 keys from properties.vue:333-359. Values are always strings. */
export interface KnownProperties {
	allow_cheats?: string | null
	allow_flight?: string | null
	difficulty?: string | null
	enforce_whitelist?: string | null
	force_gamemode?: string | null
	gamemode?: string | null
	generate_structures?: string | null
	generator_settings?: string | null
	hardcore?: string | null
	level_seed?: string | null
	level_type?: string | null
	max_players?: string | null
	max_tick_time?: string | null
	motd?: string | null
	pause_when_empty_seconds?: string | null
	player_idle_timeout?: string | null
	require_resource_pack?: string | null
	resource_pack?: string | null
	resource_pack_id?: string | null
	resource_pack_sha1?: string | null
	simulation_distance?: string | null
	spawn_protection?: string | null
	sync_chunk_writes?: string | null
	view_distance?: string | null
	white_list?: string | null
}

export interface ServerProperties {
	known: KnownProperties
	custom: Record<string, string>
	restart_required: boolean
}

export interface ServerPropertiesPatch {
	known?: Record<string, string | null>
	custom?: Record<string, string | null>
}

// ---------------------------------------------------------------- startup

export type JreVendor = 'temurin' | 'corretto' | 'graal'

export interface StartupOptions {
	java_version: number | null
	jre_vendor: JreVendor | null
	java_path: string | null
	memory_mib: number
	memory_max_mib: number
	extra_flags: string[]
	startup_command: string
	original_invocation: string
	managed_flags: string[]
	/** always there, often empty — patchStartup discards its response body, so it has to be in the GET */
	stripped_flags: string[]
	restart_required: boolean
}

export interface StartupOptionsPatch {
	java_version?: number | null
	jre_vendor?: JreVendor | null
	memory_mib?: number
	startup_command?: string | null
}

export interface JavaRuntime {
	major: number
	vendor: JreVendor
	version: string
	path: string | null
	source: 'system' | 'managed'
	installed: boolean
}

export interface JavaRuntimeList {
	runtimes: JavaRuntime[]
	default_major_for_game_version: number | null
}

// ------------------------------------------------------------ allocations

export interface Allocation {
	port: number
	name: string
}

/**
 * GET /allocations — a bare list, without the primary port.
 * network.vue:275 calls `.map` directly on the response; an envelope tears the page apart.
 */
export type AllocationList = Allocation[]

export interface CreateAllocationRequest {
	name: string
	/** panel admins only */
	port?: number
}

export interface RenameAllocationRequest {
	name: string
}

export interface SetPrimaryResponse {
	primary_port: number
	allocations: Allocation[]
	restart_required: boolean
}

// ---------------------------------------------------------------- loaders

export type LoaderId =
	| 'vanilla'
	| 'paper'
	| 'folia'
	| 'purpur'
	| 'leaf'
	| 'fabric'
	| 'velocity'
	| 'neoforge'
	| 'quilt'
	| 'forge'

export interface LoaderInfo {
	id: LoaderId
	name: string
	kind: 'vanilla' | 'server' | 'modloader' | 'proxy'
	install_kind: 'download' | 'installer'
	has_loader_versions: boolean
	supports_properties: boolean
	supports_content: boolean
	source: 'mojang' | 'papermc' | 'purpurmc' | 'leafmc' | 'fabricmc' | 'neoforged' | 'quiltmc' | 'minecraftforge'
	wave: 1 | 2
}

export interface LoaderList {
	loaders: LoaderInfo[]
}

export interface GameVersionEntry {
	version: string
	version_type: 'release' | 'snapshot'
}

export interface GameVersionList {
	loader: LoaderId
	game_versions: GameVersionEntry[]
	cached_until: Iso8601
}

/**
 * Covers LoaderVersionEntry (installation-settings/types.ts:29-36), but does not match it
 * word for word: the contract calls it `channelTag`, knows no `null` and no `released`.
 * Our page does the renaming.
 */
export interface LoaderBuild {
	id: string
	label: string
	stable: boolean
	channel_tag: 'ALPHA' | 'BETA' | null
	/** null for Purpur and Fabric — the sources there carry no date per build */
	released: Iso8601 | null
}

export interface LoaderBuildList {
	loader: LoaderId
	game_version: string
	/** newest first, at most 500, the installed build always among them */
	builds: LoaderBuild[]
	truncated: boolean
	cached_until: Iso8601
}

// ----------------------------------------------------------- installation

export type ContentPolicy = 'keep' | 'wipe_mods'

export interface InstallRequest {
	loader: LoaderId
	game_version: string
	loader_version: string | null
	content_policy: ContentPolicy
	wipe: boolean
}

export interface ResetRequest {
	loader: LoaderId
	game_version: string
	loader_version: string | null
	keep_backups: true
}

export type InstallStep =
	| 'resolve'
	| 'download'
	| 'verify'
	| 'run_installer'
	| 'write_config'
	| 'done'

export interface InstallJob {
	job_id: Ulid
	state: 'installing'
	steps: InstallStep[]
	reset?: true
	warnings?: ('properties_will_be_ignored' | 'content_may_be_incompatible')[]
}

export interface DeleteServerResponse {
	server_id: Ulid
	state: 'deleting'
	backups_kept: boolean
}

export interface ResetToSetupResponse {
	server_id: Ulid
	flows: { intro: boolean }
}

// ------------------------------------------------------------- websocket

export interface InstallStateMessage {
	type: 'install_state'
	job_id: Ulid
	state: 'installing' | 'available' | 'broken'
	step: InstallStep
	progress: number | null
	message: string | null
}

export interface StartupChangedMessage {
	type: 'startup_changed'
	java_version: number | null
	jre_vendor: JreVendor | null
	memory_mib: number
	startup_command: string
	original_invocation: string
	restart_required: boolean
}

export interface NetworkChangedMessage {
	type: 'network_changed'
	primary_port: number
	allocations: Allocation[]
}

export type SettingsWsMessage =
	| InstallStateMessage
	| StartupChangedMessage
	| NetworkChangedMessage
```

The error codes of this area, complete, so that nobody invents them twice:

```ts
export type SettingsErrorCode =
	| 'unauthorized'
	| 'forbidden'
	| 'server_not_found'
	| 'world_not_found'
	| 'internal_error'
	| 'invalid_property_key'
	| 'invalid_property_value'
	| 'property_is_panel_owned'
	| 'properties_unsupported'
	| 'invalid_java_version'
	| 'invalid_jre_vendor'
	| 'runtime_not_installed'
	| 'invalid_startup_command'
	| 'memory_budget_exceeded'
	| 'memory_too_small'
	| 'user_over_budget'
	| 'invalid_port'
	| 'invalid_name'
	| 'port_in_use'
	| 'port_unavailable'
	| 'port_pool_exhausted'
	| 'port_out_of_pool'
	| 'allocation_not_found'
	| 'allocation_limit'
	| 'primary_allocation'
	| 'already_primary'
	| 'unknown_loader'
	| 'unsupported_game_version'
	| 'build_not_found'
	| 'loader_not_found'
	| 'game_version_not_found'
	| 'loader_change_needs_wipe'
	| 'install_in_progress'
	| 'server_running'
	| 'upstream_unavailable'
	| 'no_space'
	| 'invalid_request'
```

---

## 5. Open questions and assumptions

### Decided, with reasoning

* **E1 — no property schema from the server** (2.1). The client knows the 25 keys statically; a
  schema would be a second truth. The price: our list in the backend has to be the same one as in
  `properties.vue:333-359`, and when we update the library, it has to be brought along.
* **E2 — `-Xmx` is cleaned up instead of rejected** (2.2). Reason: the interface cannot explain
  an error in any useful way, a cleaned-up field it can.
* **E3 — the startup command is a template** (2.2). `argv` is never built from an input
  (`docs/PLAN.md:191-192`). The consequence: everything that is not a flag disappears from the
  text field.
* **E4 — one unified loader endpoint** (2.4), no upstream formats passed through.
* **E5 — `worldId` is constantly `"default"`** (section 0), not `null`, because otherwise two
  queries never start.
* **E6 — the client adapter instead of a rebuilt Archon API** (decision 0.2). Ten targets of
  rewriting in the frontend, hooked into `request()`, against five rebuilt foreign paths in the
  backend.
* **E7 — `GET /allocations` is a bare list without the primary port**, because
  `network.vue:269-281` puts it in front itself (otherwise it would be there twice) and `:275`
  treats the response as an array (an envelope tears the page apart).
* **E8 — deleting and resetting demand a stopped server.** No `force`.

### Somebody else has to decide

1. **The content area** owns `isLinked`, `modpack`, all modpack methods, `previewSave`,
   `disableAllContent` and `disableIncompatibleContent` (gap L1). If `previewSave` does not come
   into being there, the diff view before a version change drops out with nothing in its place —
   the layout checks the field (`use-installation-form.ts:162`), but nobody notices the
   difference except the user.
2. **The backups area** has to serve `backups_queue_v1.create/list/cancelCreate`, otherwise the
   backup button in the warning dialogs of the loader change is dead (gap L2).
3. **The overview area** has to carry `loader`, `loader_version`, **`mc_version`**
   (that is what the field is called in the contract type, not `game_version`), `status` (with
   the value `installing`) and `net.ip`/`net.port` in the server object, provide
   `POST /api/v1/servers/:id/power` (with `409 server_installing` while a job from 2.5 is
   running), and make `GET /api/v1/servers/:id` reachable under **both** Archon paths (1.5) — the
   host of the tabs fetches them before any page renders (`ServerSettingsModal.vue:177`, `:186`).
   `PATCH /api/v1/servers/:id` is needed by nothing but our own version of `general.vue`.
4. **The loader catalog and the installation progress message are handed out twice.**
   `docs/api/servers.md` claims `GET /api/v1/loaders` with a different body and puts a second
   progress shape on the same socket (2.4 and 3). That has to be merged before anybody
   implements; as long as it is open, the two documents cannot both be built.
5. **The accounts and administration area** sets the port pool and delivers the memory budget per
   user, which `PATCH /startup` checks against.
6. **Where do Java runtimes come from?** I assume there is one, and hand out the list. Whether
   the panel fetches missing runtimes itself — Adoptium offers
   `GET https://api.adoptium.net/v3/assets/latest/{major}/hotspot?architecture=x64&image_type=jre&os=linux`
   with `binary.package.link` for that, checked on 2026-08-12 — or whether only the system
   runtimes it finds are offered, belongs to the process area. The contract carries both:
   `installed: false` means "obtainable".
7. **The DNS box in `network.vue:149-210`** needs no endpoint, but it needs `net.ip`. Whether we
   keep it is a question for the interface, not for the API.

### Measured afterwards, while building

* **The third kind of collision from 2.3 costs a real bind attempt, and that makes the tests
  sensitive to each other.** `free_on_the_machine` really binds, so two tests reaching for the same
  numbers at the same moment see each other's probe, and one of them counts the port as taken.
  Measured: `the_pool_hands_out_the_next_free_number` failed on roughly every eighth run of the
  file. Each test therefore gets a range of its own, clear of the panel's pool (25565–25700) **and
  above** `ip_local_port_range` (default 32768–60999): every outgoing connection on the machine —
  the running panel's included — takes a number out of that range, and the probe would read it as
  an occupied port
  (`settings/allocations.rs:292-299`).
* **Throwing `-Xmx` away (E2) has an edge that only showed up in operation** and is written out
  in CONTRACT.md 9.3: the page sends the command field back on **every** save, unchanged ones
  included. So whatever the panel writes into it comes back to the panel, and came out as "the
  panel removed your flag" although nobody had typed one, with the memory size from some time or
  other. That is why the rendered command **never** carries a managed flag
  (`settings/startup.rs:13-18`, counter-check `api/settings.rs:1166-1169`).
* **The loader catalog from 2.4 had two counting errors**, both found in the running panel and
  written out in CONTRACT.md 9.13/9.14: five hundred builds from the source plus one build that a
  server here runs on answered five hundred entries with `truncated: false`; and `POST …/install`
  with a game version whose whole series is still pre-releases (Paper 1.21.5) answered
  `202`, died at three percent and left the server `broken`.

### What I could not settle

* **The plan counts eight loaders in the first wave** (`docs/PLAN.md:379`), but its own table
  lists **seven** (`docs/PLAN.md:383-389`: Vanilla, Paper, Folia, Purpur, Leaf, Fabric,
  Velocity), and it speaks of "four sources" while there are five (Mojang, PaperMC, Purpur,
  Leaf, Fabric). My catalog has seven entries in wave 1. Which eighth loader was meant —
  Waterfall would be the obvious candidate from the same PaperMC source — is for the plan to say.
* **`docs/PLAN.md:429` claims that Modrinth's properties page shows the empty state
  "No properties found" for Velocity.** That is not true: this text only appears when a search
  finds nothing (`properties.vue:264-271`, condition `hasNoResults` → `isSearchActive`). With an
  empty file the page shows the warning "Some expected properties are missing from your
  server.properties…" instead (`properties.vue:5-9`, `:429`). So it works, but it says something
  wrong. Whether we change this text for proxies is a decision for the interface.
* **Whether `-Xmx` should apply to Velocity as well.** A proxy needs little memory, but falls
  under the same budget. I have treated it the same; a special case would be defensible.
* **Behavior after a failed installation.** I set `state: "broken"` and leave the files where
  they are, so that you can look. Whether it instead rolls back to the previous state
  automatically has consequences for the backups area and is not to be decided here.
* **Whether `run_installer` (second wave) runs under the same system account as the server.**
  Probably yes — the installer writes into its directory. The startup command differs afterwards
  (`docs/PLAN.md:398`), which the template from E3 has to cover: for NeoForge and Forge the
  argument file (`@user_jvm_args.txt`, `@…/win_args.txt`) comes instead of `-jar`. That is one
  line in the loader definition, but somebody who builds the process start has to write it.

**A note on all the `docs/PLAN.md` references in this document.** They were 25 lines too early
throughout — the plan gained a section after the first draft. All references have been brought up
to the state of 2026-08-12 with 505 lines and checked one by one.



