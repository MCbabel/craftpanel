# Sign-in, users, roles and limits

Interface contract for the *Accounts* area. As of 2026-08-12.

All paths under `/api/v1/`. JSON, field names `snake_case`, IDs are ULIDs, timestamps RFC 3339
in UTC. Errors: HTTP status plus `{ "error": "<code>", "message": "<text>" }`. Sign-in through a
session cookie. Source references `path:line` are relative to the repository root.

---

## 1. The provider contract

Two contracts have to be served, and they sit at different levels:

1. **`ModrinthServerContext.currentUserPermissions`** — a single field, but every button in every
   shared layout depends on it.
2. **The building blocks under `vendor/modrinth/ui/src/components/servers/access/`** — tables and
   dialogs of the access page. They are components with props, not a `provide` contract; per the
   plan (`docs/PLAN.md:72`) they are adopted unchanged, so their props are just as binding.

The access page itself (`layouts/wrapped/.../access/access.vue`) sits under `wrapped/` and is
**not** adopted. Throughout this document it is the reference for *how* the building blocks are
used. We do not ship it.

### 1.1 `ModrinthServerContext.currentUserPermissions`

| Field | Type | Source | Where the value comes from |
|---|---|---|---|
| `currentUserPermissions` | `ComputedRef<Archon.Servers.v0.UserScope>` | `vendor/modrinth/ui/src/providers/server-context.ts:42` | `current_user_permissions` from the server object (*Servers* area), computed by us from panel role + membership. Format: see 1.2. |

The value is read only through `useServerPermissions()`
(`vendor/modrinth/ui/src/composables/server-permissions.ts:85`). The function injects the server
context **without a fallback**; if it is missing, `createContext` throws
(`vendor/modrinth/ui/src/providers/create-context.ts:66`). So every shared layout needs the
field, and the access page is only one of them.

**Type friction that has to be named.** `Archon.Servers.v0.UserScope` is `number`
(`vendor/modrinth/api-client/src/modules/archon/types.ts:631`), `Archon.ServerUsers.v1.UserScope`
by contrast `string | number` (ibid.:531). At runtime `hasServerPermission` accepts both
(`server-permissions.ts:55-59`). We deliver **strings**.

The cast sits where the server object is **built** (`current_user_permissions: mask as
unknown as number`), **not** in the `computed`: there the field is already `number` by type, so a
cast would do nothing. At runtime a string stays put, and that is exactly what
`parsePermissionString` takes apart (`server-permissions.ts:39-53`). Why the string: the numeric
form uses bits 55–63, so values around 9·10¹⁸ — exactly representable as a `double`, but
unreadable in logs and in the database and silently wrong at the smallest mistake.

### 1.2 The permission bits

Modrinth defines 15 bits (`server-permissions.ts:15-32`). They are hard-wired: a name that is not
in this table is **silently dropped** when parsing
(`server-permissions.ts:45-52`, `filter(... permission in serverPermissionBits)`). Bit names of our
own are therefore impossible as long as the file stays unchanged. We take ten and drop five.

| Bit | Value | Who checks it | With us |
|---|---|---|---|
| `BASE_READ` | `1<<63` | nobody in the library — the only occurrence is the role set in `access.vue:172,180` | **kept.** Means "may see the server". Enforced by us alone (every read endpoint, WS connection). |
| `POWER_ACTIONS` | `1<<62` | `components/servers/server-header/use-server-power-action.ts:19` | **kept.** Start/stop/restart/kill. |
| `EXEC_COMMANDS` | `1<<61` | `canExecuteCommands` (`server-permissions.ts:93`) has **no** user in the library | **kept.** We wire it ourselves through `ConsoleManagerContext.disableCommandInput` (`vendor/modrinth/ui/src/layouts/shared/console/providers/console-manager.ts:16-17`). |
| `FILES_WRITE` | `1<<60` | `layouts/shared/server-settings/pages/general.vue:169`, `.../advanced.vue:235`, `components/servers/admonitions/FileOperationAdmonition.vue:65` | **kept.** |
| `SETUP` | `1<<59` | `layouts/shared/server-settings/pages/installation.vue:116`, `components/servers/ServerSetupModal.vue:66`, `components/servers/admonitions/ServerPanelAdmonitions.vue:36` | **kept.** Install loader/version. |
| `BACKUPS` | `1<<58` | `layouts/shared/content-tab/components/modals/InlineBackupCreator.vue:83`, `ServerPanelAdmonitions.vue:36` | **kept.** |
| `ADVANCED` | `1<<57` | `layouts/shared/server-settings/pages/network.vue:244`, `.../advanced.vue:235`, `.../general.vue:169`, `.../properties.vue:315` | **kept.** Ports, startup command, Java runtime — **and `server.properties`**: `properties.vue` locks every input field behind it (ibid.:59,73,87,100-243) and offers the restart hint only with `POWER_ACTIONS` (ibid.:283). |
| `RESET_SERVER` | `1<<56` | `layouts/shared/server-settings/pages/installation.vue:116` | **kept.** Reinstall/reset (`docs/PLAN.md:470`). Separate from `SETUP` because it destroys data. |
| `MANAGE_USERS` | `1<<55` | access page: `access.vue:182` | **kept.** |
| `SERVER_ADMIN` | `(2⁶⁴−1) ^ (2¹⁵−1)` | `server-permissions.ts:73-79` — as a short circuit before every single check; `components/servers/access/permissions.ts:9` | **kept.** The owner bit; contains every bit from 15 up. |
| `SUPPORT_AGENT` | `1` | checked nowhere | **dropped.** The "support agent" role is explicitly out (`docs/PLAN.md:94`). |
| `INFRA_MANAGER`, `INFRA_MANAGER_READ`, `INFRA_SERVERS_XFER`, `INFRA_USERS` | `1<<1` … `1<<4` | checked nowhere | **dropped.** There are no nodes, regions or migrations here (`docs/PLAN.md:93-94`); on one machine "infrastructure" means nothing. |

Those are exactly the ten bits `docs/PLAN.md:479-480` speaks of.

**Wire format.** A string of bit names separated by ` | `. Examples:
`"SERVER_ADMIN"`, `"BASE_READ | POWER_ACTIONS"`, `""` (nothing). The empty string yields `0n`
(`server-permissions.ts:50`). Order and whitespace do not matter (`.trim()`, ibid.:47).

### 1.3 The two role levels

**Panel role** `admin | user`, applies to the whole panel (`docs/PLAN.md:308`).
**Server role** `owner | editor | viewer`, applies per server. In the interface the type is called
`ServerAccessRole` (`vendor/modrinth/ui/src/components/servers/access/types.ts:5`) and has exactly
these three values. The levels are independent (`docs/PLAN.md:311-312`).

Role → bits (our decision, congruent with `access.vue:171-180`):

| Role | Bits | Shown as |
|---|---|---|
| `owner` | `SERVER_ADMIN` | "Owner", orange (`AccessTable.vue:513-515`) |
| `editor` | `BASE_READ \| POWER_ACTIONS \| EXEC_COMMANDS \| FILES_WRITE \| SETUP \| BACKUPS \| ADVANCED` | "Editor", green |
| `viewer` | `BASE_READ \| POWER_ACTIONS` | "Limited", blue |

Bits → role is done by `apiPermissionsToAccessRole`
(`vendor/modrinth/ui/src/components/servers/access/permissions.ts:6-23`) and is **fixed**:
`SERVER_ADMIN` ⇒ owner; otherwise one of `EXEC_COMMANDS, FILES_WRITE, SETUP, BACKUPS, ADVANCED,
RESET_SERVER` ⇒ editor; otherwise viewer. Our role sets have to match that. They do.

Two consequences you have to know:

- **`viewer` may start and stop the server.** That is Modrinth's design, not an oversight: the
  description reads "Start, stop, and view the server without making changes"
  (`layouts/wrapped/hosting/manage/[id]/access/messages.ts:84-87`). Somebody who really should only
  watch would get `BASE_READ` alone — the role would still be `viewer` according to
  `apiPermissionsToAccessRole`, but the interface does not show the difference. See section 5.
- **`editor` may write and delete files.** `docs/PLAN.md:483` phrases the acceptance criterion
  "an editor can restart but cannot delete files". That describes our `viewer`, not the `editor`.
  See section 5, open question 1.
- `editor` gets **no** `RESET_SERVER` and **no** `MANAGE_USERS`. Both stay with the owner.
  `RESET_SERVER` is missing from Modrinth's `editorScopes` (`access.vue:171-179`), although
  `apiPermissionsToAccessRole` accepts it as an editor trait; we follow that.

**The panel admin.** `panel_role = "admin"` yields `current_user_permissions = "SERVER_ADMIN"` on
**every** server, without a membership row. That is why they do **not** appear in the member list.
Reason: `docs/PLAN.md:356-358` — "see all servers of all users".

### 1.4 The building blocks of the access page

Watch out: the component types are **camelCase**, our API is snake_case. The conversion happens in
our page, the way `access.vue:237-272` does it (around 35 lines). This does not contradict the
decision: snake_case saves the conversion where the *Archon types* are passed through — in the
audit log (1.4.3), and there it saves it completely.

#### 1.4.1 `ServerAccessMember` (`components/servers/access/types.ts:17-25`)

| Field | Type | Where from |
|---|---|---|
| `id` | `string` | `member.id` from `GET /servers/:id/members`. `access.vue:253` builds `${serverId}-${userId}` instead; we deliver a real ULID, it serves only as the `row-key` (`AccessTable.vue:9`). |
| `user.id` | `string` | `member.user.id` |
| `user.username` | `string` | `member.user.username` |
| `user.avatarUrl` | `string?` | `member.user.avatar_url` — **always `null`** with us. The contract wants `string \| undefined`, not `null`; the conversion has to write `avatar_url \|\| undefined`, the way `access.vue:257` does, otherwise it is a type error. `Avatar` then tints through `tint-by="username"` (`AccessTable.vue:19-25`). No image upload. |
| `role` | `ServerAccessRole` | `member.role`. Delivered directly, not derived from the bits — saves the page the call to `apiPermissionsToAccessRole`. |
| `joinedAt` | `string \| null` | `member.joined_at`. Null while an invitation is open; the table then shows "Pending" (`AccessTable.vue:63-72`). |
| `inviteResendAvailableAt` | `string \| null` (optional, in fact needed) | `member.invite_resend_available_at`. Controls the lock on the resend button and its label "Resend in {seconds}s" (`AccessTable.vue:548-568`). Without a value the button is active again at once. |
| `pending` | `boolean` (optional, in fact needed) | `member.pending` = `joined_at == null`. Switches to resend/cancel invitation instead of revoke access (`AccessTable.vue:77,94-97`). |
| `isOwner` | `boolean` (optional, in fact needed) | `member.is_owner` = `role == "owner"`. Without the field the owner gets a role select instead of a fixed badge (`AccessTable.vue:34-41`) and action buttons (`AccessTable.vue:76`). |

`ServerAccessUser` extends `AuditActor` (`types.ts:11-15`, `events/types.ts:4-9`); the extra
fields `avatarUrl` and `profilePath` are optional and stay empty.

Further props of the table that do not come from the API: `roles: ServerAccessRoleOption[]`
(three entries with labels of their own, `access.vue:189-205`), `canManageUsers`
(from `useServerPermissions()`), `permissionDeniedMessage`, `userProfileLink`. The four events
`updateRole`, `resendInvite`, `cancelInvite`, `removeMember` (`AccessTable.vue:322-327`) map onto
2.8, 2.10 and twice onto 2.9.

**`userProfileLink` cannot be switched off.** `getUserProfileLink` reads
`props.userProfileLink?.(username) ?? '/user/' + encodeURIComponent(username)`
(`AccessTable.vue:538-541`). If our function returns `undefined`, the `??` branch takes over, and
out comes exactly the dead link `/user/<name>` we wanted to avoid. Three ways, none of them free:

1. add a route `/user/:username` to our own router, even if it is only a redirect;
2. pass a function that itself returns a **function** (`() => () => {}`) — `AutoLink` then renders
   a `<button>` instead of a link (`components/base/AutoLink.vue:21-28`), but the link color stays
   (`AccessTable.vue:17`);
3. accept the dead link.

**Proposal: (1).** The audit log needs the same route, and there without any choice (1.4.3).
Somebody else has to confirm this, see section 5, open question 20.

#### 1.4.2 Inviting and removing

`GrantAccessModal` (`components/servers/access/GrantAccessModal.vue:169-184`):

| Prop | Where from |
|---|---|
| `members` | the same list as above — used to detect duplicates (`GrantAccessModal.vue:373-392`) |
| `searchUsers(query) => ServerAccessInviteSuggestion[]` | `GET /users/search?query=`. From one character on (`GrantAccessModal.vue:195`), debounced by 250 ms (ibid.:412). |
| `friendIds` | **There are no friend lists here.** If the field stays empty, the dialog shows the checkbox "Also send a friend request" as soon as a user is selected (`GrantAccessModal.vue:318-321`). So we pass the IDs of all search hits as `friendIds` — then `targetIsFriend` is always true and the box never appears. `GrantServerAccessPayload.addAsFriend` is then always `false` (ibid.:465) and we ignore it. |
| `canGrant`, `permissionDeniedMessage` | `MANAGE_USERS` |

The sixth prop `suggestions` (`GrantAccessModal.vue:172`) stays empty: it is the offline
alternative to `searchUsers`, and we have `searchUsers`.

`ServerAccessInviteSuggestion` (`types.ts:46-51`): `id`, `username`, `avatarUrl?`, `email?`.
`email` stays empty: we have no e-mail addresses (section 5).

**Two labels in the dialog cannot be reached through props** and carry someone else's brand:
the field is called "Modrinth username", the hint below it "Do not use their Minecraft username."
(`GrantAccessModal.vue:214-232`), and the footer text links hard-coded to
`href="/news/article/server-access/"` (ibid.:97-100). That is not a question for this contract but
part of the de-branding — recorded here only so it does not get missed.

`RemoveAccessModal` (`components/servers/access/RemoveAccessModal.vue:102-120`) gets
`username`, `avatarUrl`, `role`, `joinedAt`, `pending` from the member object plus
`shouldCancel`, `canRemove` and `permissionDeniedMessage` from the state of the page. It emits
only `remove`; which of the two cases is meant the page knows from `shouldCancel`
(`access.vue:515-542`).

#### 1.4.3 Audit log: `ServerAuditLogEntry` and `ParsedAuditEvent`

`ServerAuditLogEntry` (`types.ts:27-33`) does **not** come straight from JSON but from
`parseAuditEvent(entry, lookups)` (`components/servers/access/events/parser.ts:40`). That function
sits under `components/` and is adopted unchanged, so its input is our contract,
and that input is Archon-shaped and snake_case.

| Field | Where from |
|---|---|
| `id` | assigned by us (`entry.id`); `access.vue` builds itself a hash instead (`audit-log-utils.ts:227-237`) because Archon delivers no IDs. We deliver them. |
| `actor` | `parseAuditEvent` creates it from `entry.actor` + `page.users` (`parser.ts:333-352`) |
| `world` | `parseAuditEvent` creates it from `entry.world_id` (`parser.ts:379-385`) — **always `null`** with us |
| `event` | return value of `parseAuditEvent` |
| `timestamp` | `entry.timestamp` |

Input per entry — `Archon.Actions.v1.ActionEntry`
(`vendor/modrinth/api-client/src/modules/archon/types.ts:172-178`):

| Field | Value with us |
|---|---|
| `actor` | `{ "type": "user", "user_id": "<ULID>" }`. We do **not** use `type: "support"` — the table shows the Intercom icon for it (`AuditLogTable.vue:80-82`), a protected trademark. |
| `action.action` | name from the catalog in 2.14 |
| `action.metadata` | object, different per action — mandatory fields in 2.14. If a field is missing, the entry falls back to `UnknownEvent` and shows "Unknown event <name>" (`parser.ts:287-288`, `UnknownEvent.vue:3-4`). |
| `server_id` | ULID of the server |
| `world_id` | **always `null`.** One world per server. `parseAuditEvent` copes with that (`parser.ts:304,379-381`), the world column is hidden through `show-world-column="false"` (`access.vue:68`), and the world filter category appears only if both hold (`audit-log.ts:71-73`). **No constant is needed.** Should a shared component demand a value after all, the constant meant for it is `SERVER_SCOPED_ACTION_LOG_WORLD_FILTER = "__server_scoped__"` (`audit-log-utils.ts:22`) and means exactly "server-wide, no world". |
| `timestamp` | RFC 3339 |

`AuditEventLookups` (`events/types.ts:62-69`) per page:

| Field | Where from |
|---|---|
| `serverId` | known |
| `users` | `page.users` — `Record<user_id, { username, avatar_url }>`. Without a hit the table shows the raw ID (`parser.ts:346-348`). |
| `addons` | `page.addons` — `Record<project_id, { title, slug, icon_url }>` from our content cache; otherwise the display shortens to eight characters of ID (`parser.ts:436,606-609`). |
| `versions` | `page.versions` — `Record<version_id, { name, version_number }>` |
| `worldById` | empty `Map` |
| `backupById` | `Map` from the backup endpoint (*Backups* area). If a backup is missing, `BackupEvent` still renders with `backup: undefined` (`parser.ts:266-267`, `backupEntity` returns `null`). |

`AuditLogTable` (`AuditLogTable.vue:272-299`) also demands seven props — `entries`,
`hasActiveExternalFilters`, `hasMore`, `loading`, `loadingMore`, `showWorldColumn`,
`suppressRowTransitions` —, emits `load-more` and carries nine `v-model`s: `query`,
`timeframeMode`, `timeframePreset`, `timeframeLastAmount`, `timeframeLastUnit`,
`timeframeCustomStartDate`, `timeframeCustomEndDate`, `sortDirection` and `filters`
(`ServerAuditLogFilters`, so `userId`/`worldId`).

Three of them are not self-explanatory:

- **We do not bind `filters` and `query`.** Both have defaults
  (`AuditLogTable.vue:286,294-299`) and filter **on the client** across the pages already loaded
  (ibid.:446-469). Modrinth does not bind them either; the actor filter runs server-side there.
  Same with us: `actor` in 2.14. Two filter paths for the same thing are a trap: the client-side
  ones stay unused.
- **`hasActiveExternalFilters` is mandatory as soon as 2.14 is filtered.** Without that flag the
  table shows "no activity yet" instead of "nothing matches the filters" on an empty result
  (`AuditLogTable.vue:505-518`).
- **The filter bar is not free.** `AuditLogTable` renders filters **only** if the slot `#filters`
  is filled (`AuditLogTable.vue:15-18`); the component brings none of its own. Modrinth fills it
  with `DropdownFilterBar` (which sits under `components/base/`, so it comes along) and the
  categories from `useAccessAuditLog` — and **those** sit under `wrapped/`
  (`audit-log.ts:273`, `audit-log-utils.ts:73-185`, together around 500 lines including one
  translation per action name). That is **work of our own**; without it `actor` and `action` from
  2.14 have no control.

The table's free-text search works **on the client** over `event.searchText` of the pages already
loaded (`AuditLogTable.vue:446-469`), hence the large page size (Modrinth: 200,
`audit-log.ts:43`).

**`parseAuditEvent` produces five hard-coded Modrinth paths** for which there is no prop and no
option. Adopt the function unchanged and you adopt these routes with it:

| Produced in | Target | Visible as |
|---|---|---|
| `parser.ts:350` | `/user/<name>` | actor name in every log row (`AuditLogTable.vue:72-76,531-533`) |
| `parser.ts:398` | `/user/<name>` | the user named in `UserAccessEvent` |
| `parser.ts:440,543-544` | `/project/<slug>/version/<id>` | content events |
| `parser.ts:474-481` | `/hosting/manage/<server_id>/files?path=…&editing=…` | file events |
| `parser.ts:462-465` | `/hosting/manage/<server_id>/backups?backup=…` | backup events |

That is a requirement on the **router**, not on the API: either our server pages are called
`/hosting/manage/:id/…` too, or we set up redirects. `/project/…` sensibly points at
modrinth.com, `/user/:username` at our own user page (1.4.1). Without that, five kinds of link run
into nothing. See section 5, open question 20.

### 1.5 `AuthProvider` — optional

`vendor/modrinth/ui/src/providers/auth.ts:12-22`:

| Field | Type | Where from |
|---|---|---|
| `session_token` | `Ref<string \| null>` | **we do not have this** — the session sits in the cookie. We set a placeholder (`"cookie"`) or `null`; no layout we adopt reads the field. |
| `user` | `Ref<AuthUser \| null>` | `GET /me`. `Labrinth.Users.v3.User` demands `id`, `username`, `created`, `role`, `badges`, `campaigns` (`vendor/modrinth/api-client/src/modules/labrinth/types.ts:1650-1671`). `role` is `'developer' \| 'moderator' \| 'admin'` (ibid.:1583) — our `panel_role` maps: `admin → "admin"`, `user → "developer"`. `badges: 0`, `campaigns: { pride_26: null }`. |
| `isReady` | `Ref<boolean>` | true as soon as `GET /me` has been answered once |
| `requestSignIn` | function | redirect to our sign-in screen |

**Is this needed?** Only by `layouts/wrapped/*` and `layouts/shared/user-profile/layout.vue:519`
— we adopt neither (`wrapped/` per `docs/PLAN.md:76`; the user profile is Modrinth's public
profile page with projects and collections). So the provider is **not mandatory**.
We provide it anyway because it costs three lines.

The library has no sign-in screen (Modrinth's sign-in lives in their Nuxt part). The sign-in page,
the password dialog and the admin user management are work of our own out of
`components/base/`, modeled on `layouts/shared/user-profile/components/edit-user-modal.vue`
(`docs/PLAN.md:301-302`).

### 1.6 What the contract does not give

- **Panel user management.** No Modrinth contract, no component. Modrinth has no interface for
  resource limits (`docs/PLAN.md:300`). Endpoints 2.16–2.23 serve a page we build ourselves.
- **Where an admin action came from.** If an admin changes something on somebody else's server,
  the log shows them as an ordinary user. Modrinth's way would be `actor.type = "support"`
  (`parser.ts:337-341`), but that comes with a brand and the label "Support (name)". A gap,
  left open on purpose.
- **Panel-wide events.** Limit changed, user created, user deleted: there is no renderer for them
  in `events/`. The audit log stays tied to a server. A gap, see section 5.
- **`addAsFriend`, `email`, avatar images, subdomains, SFTP credentials** — fields of the contracts
  that stay empty (see above).
- **Five hard-coded Modrinth paths in `parseAuditEvent`** and the profile link of the member table
  that cannot be switched off. Not an API question, but a condition for the adopted components
  not pointing into nothing (1.4.1, 1.4.3).
- **The audit log's filter bar.** `AuditLogTable` brings none; the categories sit under
  `wrapped/` (1.4.3).

---

## 2. The endpoints

### 2.0 Common ground

**Session cookie.** Name `craft_session` (follows the final product name,
`docs/PLAN.md:498`). `HttpOnly`, `SameSite=Lax`, `Path=/`, `Secure` as soon as it is served over
HTTPS. Content: a 256-bit random number in base64url; the database holds only its SHA-256.
Lifetime 30 days, sliding — every request extends it, written at most once an hour.
Passwords: Argon2id.

**CSRF.** `SameSite=Lax` covers the normal case. On top of that every modifying endpoint checks the
`Origin` header, if present, against the configured panel origin → otherwise `403
csrf_origin_mismatch`. Requests with `Content-Type: application/x-www-form-urlencoded` or
`multipart/form-data` are rejected with `415` on JSON endpoints.

**Permission column.** `—` = no sign-in; `Session` = signed in; `BASE_READ`,
`MANAGE_USERS` = server permission per 1.2; `admin` = `panel_role == "admin"`.

**Error codes** (stable, machine-readable):

| Status | Code | Meaning |
|---|---|---|
| 400 | `validation_failed` | field missing or value out of range |
| 400 | `weak_password` | under 10 characters |
| 400 | `invalid_role` | role unknown |
| 400 | `role_not_assignable` | `owner` cannot be assigned |
| 400 | `cannot_invite_self` | inviting yourself |
| 400 | `cannot_remove_owner` | the owner cannot be removed |
| 400 | `invalid_transfer_target` | target user missing, the same user, or without a finished system user |
| 401 | `unauthenticated` | no cookie or an expired one |
| 401 | `invalid_credentials` | name or password wrong |
| 403 | `forbidden` | signed in, but the permission is missing |
| 403 | `wrong_password` | old password wrong |
| 403 | `csrf_origin_mismatch` | foreign origin |
| 403 | `cannot_delete_self` | an admin deleting themselves |
| 404 | `user_not_found` / `server_not_found` / `member_not_found` / `invitation_not_found` | |
| 409 | `username_taken` | name taken |
| 409 | `already_member` | already a member or invited |
| 409 | `user_has_servers` | deleting without a decision about the servers |
| 409 | `servers_running` | servers are still running |
| 409 | `last_admin` | last admin |
| 409 | `system_user_not_ready` | system user missing or broken |
| 409 | `user_busy` | another management operation on this user is running (2.20, 2.22) |
| 409 | `over_limit` | budget exceeded. Only **named** here because 2.22 refers to it; it is raised in the *Servers* area (create, start) |
| 415 | `unsupported_media_type` | |
| 429 | `too_many_attempts` | sign-in throttle |
| 500 | `internal` | |

---

### 2.1 `POST /api/v1/auth/login` — sign in

Permission: `—`

```json
{
  "username": "max",
  "password": "korrekthorsebatterystaple"
}
```

`200`, sets `Set-Cookie: craft_session=...; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000`.
The body is the same as for `GET /me` (2.3).

Errors: `401 invalid_credentials` (also for an unknown name — no difference on the outside);
`429 too_many_attempts` after ten failed attempts per account and per source IP within
15 minutes, locked for 15 minutes; `400 validation_failed`.

There is **no** sign-up. The installer creates the first admin through a subcommand of the
binary (`docs/PLAN.md:360`), not over HTTP.

---

### 2.2 `POST /api/v1/auth/logout` — sign out

Permission: `Session`. No body.

`204`, deletes the cookie (`Max-Age=0`) and the session row. With no cookie also
`204` — signing out is idempotent.

---

### 2.3 `GET /api/v1/me` — who am I

Permission: `Session`

```json
{
  "id": "01K2F7Q8H3N4M5P6R7S8T9V0W1",
  "username": "max",
  "avatar_url": null,
  "panel_role": "user",
  "created": "2026-07-01T09:12:44Z",
  "last_login": "2026-08-12T13:58:02Z",
  "must_change_password": false,
  "system_user": {
    "state": "ready",
    "name": "craft-01K2F7Q8H3N4M5P6R7S8T9V0W1",
    "uid": 6104,
    "error_message": null
  },
  "limits": {
    "memory_bytes": 8589934592,
    "cpu_mode": "cap",
    "cpu_cores": 4.0,
    "pids_max": 512
  },
  "usage": {
    "memory": {
      "limit_bytes": 8589934592,
      "allocated_bytes": 10737418240,
      "used_bytes": 3221225472
    },
    "cpu": { "limit_cores": 4.0, "used_cores": 1.24 },
    "pids": { "limit": 512, "used": 137 },
    "servers": { "total": 3, "running": 1 },
    "over_limit": true,
    "over_limit_dimensions": ["memory"],
    "measured_at": "2026-08-12T14:03:11Z"
  },
  "capabilities": {
    "can_create_servers": false,
    "can_start_servers": false,
    "can_manage_panel_users": false,
    "blocked_reason": "over_limit"
  },
  "session": {
    "id": "01K2G9ZZ0A1B2C3D4E5F6G7H8J",
    "expires": "2026-09-11T13:58:02Z"
  }
}
```

`401 unauthenticated` if no valid cookie is present. That is how the sign-in screen knows it has to
show itself.

`limits` and `usage` are both there on purpose: `limits` feeds the form, `usage` the displays
— and `usage.memory.limit_bytes` appears there a second time so that a bar can be drawn from
**one** object. `usage.memory.allocated_bytes` is the sum of the `-Xmx` of all your own servers,
whether they run or not (`docs/PLAN.md:320-322`). That field is the limit in the "Memory" step of
the wizard (`docs/PLAN.md:332`).

---

### 2.4 `POST /api/v1/me/password` — change password

Permission: `Session`

```json
{
  "current_password": "korrekthorsebatterystaple",
  "new_password": "tr0ubadour-and-more"
}
```

`204`. Side effect: **all other** sessions of the user are discarded, the calling one stays and
gets a fresh cookie (rotation). Running WebSockets of the discarded sessions are closed with
`4401` (section 3).

Errors: `403 wrong_password`, `400 weak_password` (minimum length 10 characters, no further rules),
`400 validation_failed`.

---

### 2.5 `GET /api/v1/users/search` — search users (for the invite dialog)

Permission: `Session`

`GET /api/v1/users/search?query=an&limit=10`

```json
{
  "users": [
    { "id": "01K2F81M2N3P4Q5R6S7T8V9W0X", "username": "anna", "avatar_url": null },
    { "id": "01K2F82X3Y4Z5A6B7C8D9E0F1G", "username": "andre", "avatar_url": null }
  ]
}
```

Prefix search on the username, case-insensitive, `limit` at most 25.
The answer contains **only** ID, name, avatar — no role, no limits, no servers.

This is a deliberate disclosure: every signed-in user can walk the list of names this way. On one
machine among people who know each other that is defensible, and without search there is no
invitation. See section 5.

---

### 2.6 `GET /api/v1/servers/:server_id/members` — list members

Permission: `BASE_READ`

```json
{
  "members": [
    {
      "id": "01K2FA0B1C2D3E4F5G6H7J8K9M",
      "user": { "id": "01K2F7Q8H3N4M5P6R7S8T9V0W1", "username": "max", "avatar_url": null },
      "role": "owner",
      "permissions": "SERVER_ADMIN",
      "joined_at": "2026-07-04T18:22:10Z",
      "invited_at": "2026-07-04T18:22:10Z",
      "last_invite_sent": null,
      "invite_resend_available_at": null,
      "pending": false,
      "is_owner": true
    },
    {
      "id": "01K2FA1N2P3Q4R5S6T7V8W9X0Y",
      "user": { "id": "01K2F81M2N3P4Q5R6S7T8V9W0X", "username": "anna", "avatar_url": null },
      "role": "editor",
      "permissions": "BASE_READ | POWER_ACTIONS | EXEC_COMMANDS | FILES_WRITE | SETUP | BACKUPS | ADVANCED",
      "joined_at": "2026-07-05T08:01:44Z",
      "invited_at": "2026-07-04T19:00:00Z",
      "last_invite_sent": "2026-07-04T19:00:00Z",
      "invite_resend_available_at": null,
      "pending": false,
      "is_owner": false
    },
    {
      "id": "01K2FA2Y3Z4A5B6C7D8E9F0G1H",
      "user": { "id": "01K2F82X3Y4Z5A6B7C8D9E0F1G", "username": "andre", "avatar_url": null },
      "role": "viewer",
      "permissions": "BASE_READ | POWER_ACTIONS",
      "joined_at": null,
      "invited_at": "2026-08-12T13:59:00Z",
      "last_invite_sent": "2026-08-12T13:59:00Z",
      "invite_resend_available_at": "2026-08-12T14:01:00Z",
      "pending": true,
      "is_owner": false
    }
  ]
}
```

The owner is always the first entry and always present: it is not a real membership row but is
generated from `server.owner_id`. Panel admins do **not** appear (1.3).

**Where the `id` of the owner entry comes from.** There is no membership row it could come from,
and `id` is mandatory: it is the table's `row-key` (`AccessTable.vue:9,149`) and the
invitation key (2.11). For it we deliver the **server ULID**. It is stable, unique within the list
and cannot collide with any member ULID, because membership rows get their own. Endpoints
2.8–2.10 and 2.12–2.13 do not accept it — 2.8/2.9 address through `:user_id` anyway and answer
`400 cannot_remove_owner` for the owner, or cannot be triggered from the interface in the first
place (`AccessTable.vue:35,76`).

Errors: `404 server_not_found` (also when the server exists but the caller has no
`BASE_READ` — no difference on the outside), `401 unauthenticated`.

---

### 2.7 `POST /api/v1/servers/:server_id/members` — invite

Permission: `MANAGE_USERS`

```json
{
  "user_id": "01K2F82X3Y4Z5A6B7C8D9E0F1G",
  "role": "viewer"
}
```

`201` with the member object from 2.6 (`pending: true`, `joined_at: null`).

The invitation is open until the invited person accepts it (2.11). Until then they have **no**
access: `current_user_permissions` is empty for them, and the server appears in their list only as
an invitation.

Errors: `404 user_not_found`; `409 already_member` (also when an invitation is already open);
`400 cannot_invite_self`; `400 role_not_assignable` for `"owner"`; `400 invalid_role`;
`403 forbidden`.

---

### 2.8 `PATCH /api/v1/servers/:server_id/members/:user_id` — change role

Permission: `MANAGE_USERS`

```json
{
  "role": "editor"
}
```

`200` with the changed member object. Takes effect at once, even while an invitation is still open.

If a role is lowered while the person is connected, their WebSockets for this server are **not**
closed (the bits the socket needs are still there with `BASE_READ`); the individual actions are
refused on the next request.

Errors: `404 member_not_found`; `400 role_not_assignable` for `"owner"` — the interface does not
offer `owner` at all (`AccessTable.vue:464-465`, `access.vue:460`), the check is the second
line; `400 invalid_role`; `403 forbidden`.

---

### 2.9 `DELETE /api/v1/servers/:server_id/members/:user_id` — remove or cancel an invitation

Permission: `MANAGE_USERS`, **or** the caller is removing themselves.

`204`. One and the same call for both cases; the interface differs only in the
label: `cancelInvite` (`access.vue:511-513`) and `removeMember` (`access.vue:545-547`)
both end up in `removeMemberAccess`, which calls `delete` (`access.vue:554`).

Side effect: all WebSockets of this user to this server are closed with `4403`
(section 3).

Errors: `404 member_not_found`; `400 cannot_remove_owner`; `403 forbidden`.

---

### 2.10 `POST /api/v1/servers/:server_id/members/:user_id/reinvite` — resend the invitation

Permission: `MANAGE_USERS`

No body.

```json
{
  "sent": true,
  "cooldown_seconds": 120,
  "member": {
    "id": "01K2FA2Y3Z4A5B6C7D8E9F0G1H",
    "user": { "id": "01K2F82X3Y4Z5A6B7C8D9E0F1G", "username": "andre", "avatar_url": null },
    "role": "viewer",
    "permissions": "BASE_READ | POWER_ACTIONS",
    "joined_at": null,
    "invited_at": "2026-08-12T13:59:00Z",
    "last_invite_sent": "2026-08-12T14:05:30Z",
    "invite_resend_available_at": "2026-08-12T14:07:30Z",
    "pending": true,
    "is_owner": false
  }
}
```

Within the cooldown the endpoint answers **`200` with `sent: false`** and the remaining
wait, not with an error — that is exactly how the interface reads it (`access.vue:488-492`,
type `ReinviteResponse`, `vendor/modrinth/api-client/src/modules/archon/types.ts:553-556`).
Cooldown 120 seconds, as Modrinth's default (`access.vue:159`).

**What "send" means here.** We have no mail delivery. The call refreshes
`last_invite_sent` — nothing more. In particular there is **no** dismissal flag it could clear:
no endpoint sets one, `GET /invitations` knows no such field, and inventing one would mean
inventing a notification surface with it. So in v1 the button stays a gesture without a recipient;
the cooldown is the only thing that visibly happens. See section 5, open question 10.

Errors: `404 member_not_found`; `409 already_member` if the invitation was accepted long ago;
`403 forbidden`.

---

### 2.11 `GET /api/v1/invitations` — my open invitations

Permission: `Session`

```json
{
  "invitations": [
    {
      "id": "01K2FA2Y3Z4A5B6C7D8E9F0G1H",
      "server": { "id": "01K2F9AB1C2D3E4F5G6H7J8K9M", "name": "Survival" },
      "role": "viewer",
      "invited_by": {
        "id": "01K2F7Q8H3N4M5P6R7S8T9V0W1",
        "username": "max",
        "avatar_url": null
      },
      "invited_at": "2026-08-12T13:59:00Z",
      "last_invite_sent": "2026-08-12T14:05:30Z"
    }
  ]
}
```

`id` is the same ID as `member.id` in 2.6: an open invitation *is* a membership row without
`joined_at`.

---

### 2.12 `POST /api/v1/invitations/:invitation_id/accept` — accept

Permission: `Session`, and the invitation has to be addressed to the caller.

No body. `200` with the member object (`pending: false`, `joined_at` set).

Errors: `404 invitation_not_found` (also for somebody else's invitation); `409 already_member` if
it was already accepted.

---

### 2.13 `POST /api/v1/invitations/:invitation_id/decline` — decline

Permission: as in 2.12. No body. `204`. The membership row disappears; a new invitation
is possible afterwards.

---

### 2.14 `GET /api/v1/servers/:server_id/audit-log` — audit log

Permission: `BASE_READ`

Query parameters:

| Parameter | Type | Default | Meaning |
|---|---|---|---|
| `limit` | 1…500 | 200 | page size; 200 like Modrinth (`audit-log.ts:43`) |
| `offset` | ≥ 0 | 0 | |
| `order` | `asc` \| `desc` | `desc` | |
| `min_datetime` | RFC 3339 | — | period from (inclusive) |
| `max_datetime` | RFC 3339 | — | period to (inclusive) |
| `actor` | ULID, repeatable | — | OR-combined |
| `action` | action name, repeatable | — | OR-combined; unknown names give `400 validation_failed` |

We take `limit`/`offset`/`order`/`min_datetime`/`max_datetime` from Modrinth under the same names
(`vendor/modrinth/api-client/src/modules/archon/actions/v1.ts:17-23`), but replace their
JSON-encoded `filter` parameter (ibid.:18) with repeated parameters. Reason: the caller is
our own page, and a JSON blob in the URL is hard to read and hard to cache for no good
reason.

```json
{
  "next_offset": 200,
  "data": [
    {
      "id": "01K2GB3C4D5E6F7G8H9J0K1M2N",
      "actor": { "type": "user", "user_id": "01K2F7Q8H3N4M5P6R7S8T9V0W1" },
      "action": {
        "action": "user_invited",
        "metadata": {
          "user_id": "01K2F82X3Y4Z5A6B7C8D9E0F1G",
          "permissions": "BASE_READ | POWER_ACTIONS"
        }
      },
      "server_id": "01K2F9AB1C2D3E4F5G6H7J8K9M",
      "world_id": null,
      "timestamp": "2026-08-12T13:59:00Z"
    },
    {
      "id": "01K2GB4N5P6Q7R8S9T0V1W2X3Y",
      "actor": { "type": "user", "user_id": "01K2F81M2N3P4Q5R6S7T8V9W0X" },
      "action": { "action": "server_started", "metadata": null },
      "server_id": "01K2F9AB1C2D3E4F5G6H7J8K9M",
      "world_id": null,
      "timestamp": "2026-08-12T12:41:07Z"
    },
    {
      "id": "01K2GB5Y6Z7A8B9C0D1E2F3G4H",
      "actor": { "type": "user", "user_id": "01K2F81M2N3P4Q5R6S7T8V9W0X" },
      "action": {
        "action": "file_deleted",
        "metadata": { "path": "/plugins/EssentialsX.jar" }
      },
      "server_id": "01K2F9AB1C2D3E4F5G6H7J8K9M",
      "world_id": null,
      "timestamp": "2026-08-12T12:39:55Z"
    }
  ],
  "users": {
    "01K2F7Q8H3N4M5P6R7S8T9V0W1": { "username": "max", "avatar_url": null },
    "01K2F81M2N3P4Q5R6S7T8V9W0X": { "username": "anna", "avatar_url": null },
    "01K2F82X3Y4Z5A6B7C8D9E0F1G": { "username": "andre", "avatar_url": null }
  },
  "addons": {
    "AANobbMI": { "title": "Sodium", "slug": "sodium", "icon_url": null }
  },
  "versions": {
    "yaoBL9D9": { "name": "Sodium 0.5.8", "version_number": "mc1.20.4-0.5.8" }
  }
}
```

`next_offset` is `null` on the last page — that is what the pagination expects
(`audit-log.ts:142-143`). `users` contains every actor occurring in the page slice **and**
every user named in metadata. `addons`/`versions` are filled from our content cache;
unknown entries are left out (the display then shortens to the ID).

**The event catalog.** Only names `parseAuditEvent` knows are shown readably
(`parser.ts:53-289`); everything else ends up at "Unknown event". Out of Modrinth's 42 names
(`audit-log-utils.ts:24-67`) we take 39 and leave out three.

| Action | Mandatory metadata | Renderer (source) | Produced in area |
|---|---|---|---|
| `server_created` | none | `BasicStringEvent` (`parser.ts:27-38`) | Servers |
| `server_started`, `server_stopped`, `server_restarted`, `server_killed` | none | `BasicStringEvent` | Servers |
| `server_repaired`, `server_reset` | none | `BasicStringEvent` | Settings |
| `server_reallocated` | none | `BasicStringEvent`, text "Reallocated server" (`BasicStringEvent.vue:24-27`) | Settings — **we use it for "`-Xmx` of this server changed"**; there is no name of our own |
| `console_cleared` | none | `BasicStringEvent` | Console |
| `console_command_executed` | `{ command: string }` | `ConsoleEvent` (`parser.ts:255-259`) | Console |
| `changed_server_name` | `{ name: string }` | `ServerMetaEvent` (`parser.ts:54-64`) | Settings |
| `user_invited` | `{ user_id: string, permissions: string }` | `UserAccessEvent` (`parser.ts:83-95`) | **Accounts** |
| `user_permission_modified` | `{ user_id, permissions }` | `UserAccessEvent` | **Accounts** |
| `user_invite_revoked` | `{ user_id }` | `UserAccessEvent` (`parser.ts:96-107`) | **Accounts** |
| `user_removed` | `{ user_id }` | `UserAccessEvent` | **Accounts** |
| `addon_added`, `addon_disabled`, `addon_enabled`, `addon_deleted`, `addon_updated` | `{ addons: [{ addon_id, version_id }] }` | `AddonEvent` (`parser.ts:108-125`) | Content |
| `addon_uploaded` | `{ file_names: string[] }` | `AddonEvent` (`parser.ts:126-134`) | Content |
| `modpack_changed`, `modpack_unlinked` | `{ spec: { platform: "modrinth", project_id, version_id } }` or `{ spec: { platform: "local_file", filename, name, version_id } }` | `ModpackEvent` (`parser.ts:135-158,516-568`) | Content |
| `port_allocation_added`, `port_allocation_removed` | `{ port: number }` | `NetworkEvent` (`parser.ts:159-169`) | Settings |
| `loader_version_edited` | `{ new_loader: string\|null, new_version: string\|null }` — the key `new_version` **must** be present (`parser.ts:172`) | `ConfigEvent` | Settings |
| `game_version_edited` | `{ new_version: string }` | `ConfigEvent` | Settings |
| `server_properties_modified` | `{ properties: { "<key>": "<value>" } }` | `ConfigEvent` (`parser.ts:189-203`) | Settings |
| `startup_command_modified` | `{ command: string }` | `ConfigEvent` | Settings |
| `java_runtime_modified` | `{ vendor: string }` | `ConfigEvent` | Settings |
| `java_version_modified` | `{ version: number }` | `ConfigEvent` | Settings |
| `file_uploaded`, `file_deleted`, `file_edited` | `{ path: string }` | `FileEvent` (`parser.ts:228-238`) | Files |
| `file_renamed` | `{ from: string, to: string }` | `FileEvent` | Files |
| `backup_created`, `backup_restored`, `backup_deleted` | `{ id: string }` | `BackupEvent` (`parser.ts:260-272`) | Backups |
| `backup_renamed` | `{ id, from, to }` | `BackupEvent` | Backups |

Left out: `changed_server_subdomain` (no subdomains, `docs/PLAN.md:94`), `server_plan_changed`
(no plans), `sftp_login` (no SFTP, `docs/PLAN.md:97`).

**Retention.** Entries stay 180 days, after that a daily run clears them out. When a server is
deleted, its log goes too.

---

### 2.15 `GET /api/v1/admin/host` — what the machine can give

Permission: `admin`

```json
{
  "cpu_cores": 16,
  "memory_bytes": 67386437632,
  "reserved_memory_bytes": 2147483648,
  "assignable_memory_bytes": 65238953984,
  "allocated": { "memory_bytes": 34359738368, "cpu_cores": 12.0 },
  "used": { "memory_bytes": 21474836480, "cpu_cores": 3.72, "pids": 412 },
  "user_count": 4,
  "default_user_limits": {
    "memory_bytes": 4294967296,
    "cpu_mode": "cap",
    "cpu_cores": 2.0,
    "pids_max": 512
  },
  "measured_at": "2026-08-12T14:03:11Z"
}
```

`allocated` is the sum of the user limits (not of the `-Xmx`), so what the admin has
given away. If it exceeds `assignable_memory_bytes`, the machine is oversubscribed — that is
allowed and the admin's business, the interface warns (`docs/PLAN.md:354`).
`default_user_limits` comes from `config.toml` and is the preset when creating (2.17).

---

### 2.16 `GET /api/v1/admin/users` — list panel users

Permission: `admin`

`GET /api/v1/admin/users?query=an&limit=50&offset=0`

```json
{
  "users": [
    {
      "id": "01K2F7Q8H3N4M5P6R7S8T9V0W1",
      "username": "max",
      "avatar_url": null,
      "panel_role": "user",
      "created": "2026-07-01T09:12:44Z",
      "last_login": "2026-08-12T13:58:02Z",
      "must_change_password": false,
      "system_user": {
        "state": "ready",
        "name": "craft-01K2F7Q8H3N4M5P6R7S8T9V0W1",
        "uid": 6104,
        "error_message": null
      },
      "limits": {
        "memory_bytes": 8589934592,
        "cpu_mode": "cap",
        "cpu_cores": 4.0,
        "pids_max": 512
      },
      "usage": {
        "memory": {
          "limit_bytes": 8589934592,
          "allocated_bytes": 10737418240,
          "used_bytes": 3221225472
        },
        "cpu": { "limit_cores": 4.0, "used_cores": 1.24 },
        "pids": { "limit": 512, "used": 137 },
        "servers": { "total": 3, "running": 1 },
        "over_limit": true,
        "over_limit_dimensions": ["memory"],
        "measured_at": "2026-08-12T14:03:11Z"
      }
    },
    {
      "id": "01K2F81M2N3P4Q5R6S7T8V9W0X",
      "username": "anna",
      "avatar_url": null,
      "panel_role": "admin",
      "created": "2026-06-28T20:04:00Z",
      "last_login": "2026-08-11T22:15:31Z",
      "must_change_password": false,
      "system_user": {
        "state": "ready",
        "name": "craft-01K2F81M2N3P4Q5R6S7T8V9W0X",
        "uid": 6101,
        "error_message": null
      },
      "limits": {
        "memory_bytes": 17179869184,
        "cpu_mode": "share",
        "cpu_cores": 8.0,
        "pids_max": 1024
      },
      "usage": {
        "memory": {
          "limit_bytes": 17179869184,
          "allocated_bytes": 4294967296,
          "used_bytes": 0
        },
        "cpu": { "limit_cores": 8.0, "used_cores": 0.0 },
        "pids": { "limit": 1024, "used": 0 },
        "servers": { "total": 1, "running": 0 },
        "over_limit": false,
        "over_limit_dimensions": [],
        "measured_at": "2026-08-12T14:03:11Z"
      }
    }
  ],
  "total": 2
}
```

`query` searches the username, `limit` at most 200 (default 50).

**What this answer costs.** `usage.*.used_*` comes from the cgroup files (`memory.current`,
`cpu.stat`, `pids.current`), so three file reads per user — negligible at a two-digit user
count. The values are cached for 5 seconds; `measured_at` says how old they are.

---

### 2.17 `POST /api/v1/admin/users` — create a panel user

Permission: `admin`

```json
{
  "username": "andre",
  "password": "first-password-please-change",
  "panel_role": "user",
  "must_change_password": true,
  "limits": {
    "memory_bytes": 4294967296,
    "cpu_mode": "cap",
    "cpu_cores": 2.0,
    "pids_max": 512
  }
}
```

`limits` and `must_change_password` may be omitted; then `default_user_limits` from 2.15
and `true` apply.

`201`:

```json
{
  "id": "01K2F82X3Y4Z5A6B7C8D9E0F1G",
  "username": "andre",
  "avatar_url": null,
  "panel_role": "user",
  "created": "2026-08-12T14:07:02Z",
  "last_login": null,
  "must_change_password": true,
  "system_user": {
    "state": "ready",
    "name": "craft-01K2F82X3Y4Z5A6B7C8D9E0F1G",
    "uid": 6107,
    "error_message": null
  },
  "limits": {
    "memory_bytes": 4294967296,
    "cpu_mode": "cap",
    "cpu_cores": 2.0,
    "pids_max": 512
  },
  "usage": {
    "memory": { "limit_bytes": 4294967296, "allocated_bytes": 0, "used_bytes": 0 },
    "cpu": { "limit_cores": 2.0, "used_cores": 0.0 },
    "pids": { "limit": 512, "used": 0 },
    "servers": { "total": 0, "running": 0 },
    "over_limit": false,
    "over_limit_dimensions": [],
    "measured_at": "2026-08-12T14:07:02Z"
  }
}
```

**The system user.** When creating, the service calls the helper with `create-user <id>`
(`docs/PLAN.md:187`): system user `craft-<id>`, directory `users/<id>/`, ownership and
`2770` permissions, plus the cgroup `user-<id>` with the limits. That takes milliseconds, so the
normal case is `state: "ready"` in the same answer.

`system_user.state`:

| Value | Meaning |
|---|---|
| `provisioning` | The helper call is running or the service was restarted in the middle of it. The interface polls `GET /admin/users/:id` every two seconds. |
| `ready` | System user, directory and cgroup are in place. Only now can the user create servers. |
| `error` | The helper refused or failed. `error_message` carries the plain text (e.g. `"useradd: UID range exhausted"`). The panel user exists all the same and can sign in — they just cannot create servers (`capabilities.blocked_reason = "system_user_not_ready"`). Catch up through 2.23. |

Errors: `409 username_taken`; `400 weak_password`; `400 validation_failed` (name: 3–39 characters,
`[a-z0-9_-]`, lowercase — the length follows Modrinth's display, `edit-user-modal.vue:57`);
`403 forbidden`.

A failed helper call is **not** a `500`: the panel user is created, the answer
is `201` with `state: "error"`. Anything else would leave a half-created row behind.

---

### 2.18 `GET /api/v1/admin/users/:user_id` — one user

Permission: `admin`. Answer: an object as in 2.16, plus:

```json
{
  "id": "01K2F7Q8H3N4M5P6R7S8T9V0W1",
  "username": "max",
  "avatar_url": null,
  "panel_role": "user",
  "created": "2026-07-01T09:12:44Z",
  "last_login": "2026-08-12T13:58:02Z",
  "must_change_password": false,
  "system_user": {
    "state": "ready",
    "name": "craft-01K2F7Q8H3N4M5P6R7S8T9V0W1",
    "uid": 6104,
    "error_message": null
  },
  "limits": {
    "memory_bytes": 8589934592,
    "cpu_mode": "cap",
    "cpu_cores": 4.0,
    "pids_max": 512
  },
  "usage": {
    "memory": {
      "limit_bytes": 8589934592,
      "allocated_bytes": 10737418240,
      "used_bytes": 3221225472
    },
    "cpu": { "limit_cores": 4.0, "used_cores": 1.24 },
    "pids": { "limit": 512, "used": 137 },
    "servers": { "total": 3, "running": 1 },
    "over_limit": true,
    "over_limit_dimensions": ["memory"],
    "measured_at": "2026-08-12T14:03:11Z"
  },
  "owned_servers": [
    {
      "id": "01K2F9AB1C2D3E4F5G6H7J8K9M",
      "name": "Survival",
      "memory_bytes": 6442450944,
      "running": true
    },
    {
      "id": "01K2F9CD2E3F4G5H6J7K8M9N0P",
      "name": "Creative",
      "memory_bytes": 2147483648,
      "running": false
    },
    {
      "id": "01K2F9EF3G4H5J6K7M8N9P0Q1R",
      "name": "Test world",
      "memory_bytes": 2147483648,
      "running": false
    }
  ],
  "active_sessions": 2
}
```

`owned_servers` is the basis for the delete dialog (2.20) and explains the number in
`allocated_bytes` (6+2+2 GiB = 10 GiB > the 8 GiB limit → over).

Errors: `404 user_not_found`.

---

### 2.19 `PATCH /api/v1/admin/users/:user_id` — change

Permission: `admin`

```json
{
  "username": "andré",
  "panel_role": "admin",
  "password": "new-password-from-the-admin",
  "must_change_password": true
}
```

All fields may be omitted; only those sent are changed. `200` with the object from 2.18.

- **Changing the name** does *not* change the system user — it is named after the ID precisely
  because names change (`docs/PLAN.md:140-141`).
- **Setting the password** discards all sessions of the person concerned and closes their
  WebSockets with `4401`. An admin does **not** need their old password for it; this is the route
  for "forgot password", because we have no e-mail.
- **Limits** are not changed by this endpoint; 2.22 does that.

Errors: `409 username_taken`; `400 weak_password`; `409 user_busy` (2.20); `404 user_not_found`.

On self-service, to be unambiguous: an admin **may** rename themselves, set their own password
and change `must_change_password` on themselves. They may **not** demote themselves to `user`
while they are the only admin — there is exactly one code for that, `409 last_admin`, and the same
code applies when an admin would demote **another** last admin.
`403 cannot_delete_self` belongs to 2.20 and does not occur here.

---

### 2.20 `DELETE /api/v1/admin/users/:user_id` — delete

Permission: `admin`

The plan demands an explicit decision about the servers (`docs/PLAN.md:369-371`). It
sits in the query string so that no body hangs off `DELETE`:

| Call | Effect |
|---|---|
| `DELETE /api/v1/admin/users/:user_id` | `409 user_has_servers` as soon as the user owns a server. With no servers of their own: deletes at once. |
| `DELETE /api/v1/admin/users/:user_id?servers=delete` | Servers gone along with their directories and backups, then the system user gone. |
| `DELETE /api/v1/admin/users/:user_id?servers=transfer&transfer_to=<user_id>` | Servers pass to the target user, then the system user gone. |

`204` on success.

**Condition in both cases: none of the user's servers is running** → otherwise
`409 servers_running`. The admin stops them first; we shoot nothing down
(`docs/PLAN.md:364-365` as the principle).

**The race in between.** Between the check "nothing is running" and the move there is work, and in
that time anybody with `POWER_ACTIONS` — the user themselves, but also an editor on one of their
servers — can start a server. A `chown -R` under a running Java process is exactly the kind of
fault you only see weeks later. So: the call first puts the user on `busy`; while that holds,
"start server", `POST /servers`, 2.19 and 2.22 answer `409 user_busy` for this user. Only then
does the check and the move happen. The same lock applies to 2.22, because cgroup values are
written there too.

**What transferring costs.** The server changes system user, so its directory moves from
`users/<old>/servers/<sid>/` to `users/<new>/servers/<sid>/` and is re-owned (`chown -R` through
the helper). On the same file system that is a `rename` plus a recursive `chown` — no copying, but
with a large world it is hundreds of thousands of inodes and therefore **seconds to minutes**.
That does not belong in an HTTP answer, and it does not fit with deletion running explicitly in
the background just below. So the same route: `rename` and the database still inside the call,
`chown -R` afterwards in the background, the user stays `busy` until the end. If that pushes the
target user over their budget, **the transfer happens anyway**; afterwards they count as over the
limit (2.22) and cannot start anything new. The admin ordered it explicitly, and "nothing gets
shot down" holds here too.

**Cleaning up.** Database rows and the system user disappear inside the call; the tree under
`users/<id>/` is removed in the background. The answer does not wait for `rm -rf` of 40 GiB.

Errors: `403 cannot_delete_self`; `409 last_admin`; `409 user_has_servers`; `409 servers_running`;
`409 user_busy`; `400 invalid_transfer_target` (target missing, the same user, or with
`system_user.state != "ready"`); `404 user_not_found`.

---

### 2.21 `GET /api/v1/admin/users/:user_id/limits` — read the limits

Permission: `admin`

```json
{
  "limits": {
    "memory_bytes": 8589934592,
    "cpu_mode": "cap",
    "cpu_cores": 4.0,
    "pids_max": 512
  },
  "usage": {
    "memory": {
      "limit_bytes": 8589934592,
      "allocated_bytes": 10737418240,
      "used_bytes": 3221225472
    },
    "cpu": { "limit_cores": 4.0, "used_cores": 1.24 },
    "pids": { "limit": 512, "used": 137 },
    "servers": { "total": 3, "running": 1 },
    "over_limit": true,
    "over_limit_dimensions": ["memory"],
    "measured_at": "2026-08-12T14:03:11Z"
  },
  "host": {
    "cpu_cores": 16,
    "assignable_memory_bytes": 65238953984
  }
}
```

`host` appears here a second time so that the slider knows its ceiling without fetching 2.15 as
well.

---

### 2.22 `PUT /api/v1/admin/users/:user_id/limits` — set the limits

Permission: `admin`

```json
{
  "memory_bytes": 6442450944,
  "cpu_mode": "cap",
  "cpu_cores": 3.0,
  "pids_max": 512
}
```

Complete replacement, all four fields mandatory. Answer `200` as in 2.21 with the **new** values.

Implemented in the cgroup `user-<id>` (`docs/PLAN.md:229-232,230-235`):

| Field | cgroup |
|---|---|
| `memory_bytes` | `memory.high` = value, `memory.max` = value × 1.25 (the emergency brake, `docs/PLAN.md:262-266`) |
| `cpu_cores`, `cpu_mode: "cap"` | `cpu.max` = `round(cores × 100000) 100000` |
| `cpu_cores`, `cpu_mode: "share"` | `cpu.weight` = `clamp(round(cores / host_cores × 10000), 1, 10000)`; `cpu.max` = `max` (no ceiling). A share instead of a ceiling, as described in `docs/PLAN.md:271-275`. |
| `pids_max` | `pids.max` |

**The important case: a limit below what is already allocated.** The call **succeeds**. It throws
nobody off and refuses nothing. The answer shows the result:

```json
{
  "limits": {
    "memory_bytes": 6442450944,
    "cpu_mode": "cap",
    "cpu_cores": 3.0,
    "pids_max": 512
  },
  "usage": {
    "memory": {
      "limit_bytes": 6442450944,
      "allocated_bytes": 10737418240,
      "used_bytes": 3221225472
    },
    "cpu": { "limit_cores": 3.0, "used_cores": 1.24 },
    "pids": { "limit": 512, "used": 137 },
    "servers": { "total": 3, "running": 1 },
    "over_limit": true,
    "over_limit_dimensions": ["memory"],
    "measured_at": "2026-08-12T14:09:40Z"
  },
  "host": {
    "cpu_cores": 16,
    "assignable_memory_bytes": 65238953984
  }
}
```

`over_limit = true` means exactly what `docs/PLAN.md:364-367` lays down:

- What runs keeps running. No process is ended, `memory.high` only throttles.
- `POST /servers` → `409` with `error: "over_limit"` (*Servers* area).
- "Start server" → `409 over_limit` while `allocated_bytes > limit_bytes`.
- The user concerned sees it on their own `GET /me`:
  `capabilities.can_create_servers = false`, `can_start_servers = false`,
  `blocked_reason = "over_limit"`.

They are free again as soon as `allocated_bytes ≤ limit_bytes` — so after deleting a server or
lowering its `-Xmx`, **not** when little memory happens to be in use. What is checked is the
allocated sum, never the momentary usage (`docs/PLAN.md:320-322`).

`over_limit_dimensions` is always `[]` or `["memory"]` today: CPU and processes are not
allocated in advance, so there is nothing to exceed with them. The field is a list because that
can change with per-server limits (`docs/PLAN.md:502`).

Errors: `400 validation_failed` (`memory_bytes` < 512 MiB, `cpu_cores` ≤ 0, `pids_max` < 64);
`404 user_not_found`; `409 user_busy` (2.20); `403 forbidden`. **No** error when the machine is
oversubscribed — that is the admin's decision, visible in 2.15.

---

### 2.23 `POST /api/v1/admin/users/:user_id/system-user/retry` — catch up on the system user

Permission: `admin`. No body.

Allowed only with `system_user.state ∈ {error, provisioning}`. Calls the helper again with
`create-user <id>` and answers `200` with the user object from 2.18.

Without this endpoint an account would stay permanently unusable after a helper failure, and the
only way out would be to delete it and create it again.

Errors: `409 system_user_not_ready` with the message if the second attempt fails too
(`state` stays `error`); `404 user_not_found`.

---

## 3. WebSocket

The *Accounts* area brings **no message types of its own** into the server socket
(`/api/v1/servers/:id/ws`). It lays down how the socket is authenticated and ended.

**Handshake.** The connection carries the same cookie as every other request; there is no token in
a query parameter (it would end up in logs). Checked before the upgrade:
session valid, server present, `BASE_READ` there. If a check fails, there is no upgrade
but an ordinary HTTP answer with the error object from 2.0.

**Close codes** for a connection that is already standing:

| Code | When | What the interface does |
|---|---|---|
| `4401` | session expired, signed out, password changed (2.4/2.19) | the provider sets `isWsAuthIncorrect = true` (`vendor/modrinth/ui/src/providers/server-context.ts:46`), does **not** reconnect, leads to the sign-in screen |
| `4403` | access revoked or invitation canceled (2.9) | the same flag, plus a return to the server list |
| `4404` | server deleted | return to the server list |
| `1012` | the service is restarting | the provider reconnects with backoff |

`isWsAuthIncorrect` is a mandatory field of the server context; Modrinth feeds it from the
socket events `auth-incorrect`/`auth-ok`
(`vendor/modrinth/ui/src/composables/server-manage-core-runtime.ts:268-280`). That runtime hangs
off the Archon client and is not adopted — with us the flag comes from the close code. A
message for it is unnecessary.

---

## 4. Data types

```ts
// ---------- Roles and permissions ----------

export type PanelRole = 'admin' | 'user'

/** Congruent with ServerAccessRole,
 *  vendor/modrinth/ui/src/components/servers/access/types.ts:5 */
export type ServerRole = 'owner' | 'editor' | 'viewer'

/** The ten bits we keep. Names must match those in
 *  vendor/modrinth/ui/src/composables/server-permissions.ts:15-32 literally. */
export type PermissionBit =
  | 'BASE_READ'
  | 'POWER_ACTIONS'
  | 'EXEC_COMMANDS'
  | 'FILES_WRITE'
  | 'SETUP'
  | 'BACKUPS'
  | 'ADVANCED'
  | 'RESET_SERVER'
  | 'MANAGE_USERS'
  | 'SERVER_ADMIN'

/** Bit names joined by ' | '. Empty string = no permissions. */
export type PermissionMask = string

// ---------- Users ----------

export interface UserRef {
  id: string
  username: string
  avatar_url: string | null
}

export type SystemUserState = 'provisioning' | 'ready' | 'error'

export interface SystemUser {
  state: SystemUserState
  name: string
  uid: number | null
  error_message: string | null
}

export type CpuMode = 'cap' | 'share'

export interface UserLimits {
  memory_bytes: number
  cpu_mode: CpuMode
  cpu_cores: number
  pids_max: number
}

export interface MemoryUsage {
  limit_bytes: number
  /** Sum of the -Xmx of all own servers, running or not. */
  allocated_bytes: number
  /** memory.current of the user cgroup. */
  used_bytes: number
}

export interface CpuUsage {
  limit_cores: number
  /** Averaged from cpu.stat over the measurement window. */
  used_cores: number
}

export interface PidUsage {
  limit: number
  used: number
}

export interface ServerCounts {
  total: number
  running: number
}

export type LimitDimension = 'memory' | 'cpu' | 'pids'

export interface UserUsage {
  memory: MemoryUsage
  cpu: CpuUsage
  pids: PidUsage
  servers: ServerCounts
  /** true as soon as allocated_bytes > limit_bytes. */
  over_limit: boolean
  over_limit_dimensions: LimitDimension[]
  measured_at: string
}

/** Both reasons can apply at once. Order:
 *  system_user_not_ready beats over_limit, because without a system user nothing works at all
 *  and the budget then does not matter. If neither is set, the value is null. */
export type BlockedReason = 'over_limit' | 'system_user_not_ready' | null

export interface Capabilities {
  can_create_servers: boolean
  can_start_servers: boolean
  can_manage_panel_users: boolean
  blocked_reason: BlockedReason
}

export interface SessionInfo {
  id: string
  expires: string
}

export interface PanelUser {
  id: string
  username: string
  avatar_url: string | null
  panel_role: PanelRole
  created: string
  last_login: string | null
  must_change_password: boolean
  system_user: SystemUser
  limits: UserLimits
  usage: UserUsage
}

export interface Me extends PanelUser {
  capabilities: Capabilities
  session: SessionInfo
}

export interface OwnedServerRef {
  id: string
  name: string
  memory_bytes: number
  running: boolean
}

export interface AdminUserDetail extends PanelUser {
  owned_servers: OwnedServerRef[]
  active_sessions: number
}

export interface HostCapacity {
  cpu_cores: number
  memory_bytes: number
  reserved_memory_bytes: number
  assignable_memory_bytes: number
  allocated: { memory_bytes: number; cpu_cores: number }
  used: { memory_bytes: number; cpu_cores: number; pids: number }
  user_count: number
  default_user_limits: UserLimits
  measured_at: string
}

// ---------- Membership ----------

export interface ServerMember {
  id: string
  user: UserRef
  role: ServerRole
  permissions: PermissionMask
  /** null while the invitation is open. */
  joined_at: string | null
  invited_at: string
  last_invite_sent: string | null
  invite_resend_available_at: string | null
  pending: boolean
  is_owner: boolean
}

export interface ServerMemberList {
  members: ServerMember[]
}

export interface AddMemberRequest {
  user_id: string
  role: Exclude<ServerRole, 'owner'>
}

export interface UpdateMemberRequest {
  role: Exclude<ServerRole, 'owner'>
}

/** Shape of Archon.ServerUsers.v1.ReinviteResponse,
 *  vendor/modrinth/api-client/src/modules/archon/types.ts:553-556 */
export interface ReinviteResponse {
  sent: boolean
  cooldown_seconds: number | null
  member: ServerMember
}

export interface Invitation {
  id: string
  server: { id: string; name: string }
  role: ServerRole
  invited_by: UserRef
  invited_at: string
  last_invite_sent: string | null
}

// ---------- Audit log ----------
// Shape of Archon.Actions.v1.*, so that parseAuditEvent runs unchanged:
// vendor/modrinth/api-client/src/modules/archon/types.ts:152-218

export type AuditAction =
  | 'server_created'
  | 'server_reallocated'
  | 'server_repaired'
  | 'server_reset'
  | 'server_started'
  | 'server_stopped'
  | 'server_restarted'
  | 'server_killed'
  | 'console_cleared'
  | 'console_command_executed'
  | 'changed_server_name'
  | 'user_invited'
  | 'user_invite_revoked'
  | 'user_permission_modified'
  | 'user_removed'
  | 'addon_added'
  | 'addon_uploaded'
  | 'addon_disabled'
  | 'addon_enabled'
  | 'addon_deleted'
  | 'addon_updated'
  | 'modpack_changed'
  | 'modpack_unlinked'
  | 'port_allocation_added'
  | 'port_allocation_removed'
  | 'loader_version_edited'
  | 'game_version_edited'
  | 'server_properties_modified'
  | 'startup_command_modified'
  | 'java_runtime_modified'
  | 'java_version_modified'
  | 'file_uploaded'
  | 'file_deleted'
  | 'file_renamed'
  | 'file_edited'
  | 'backup_created'
  | 'backup_renamed'
  | 'backup_restored'
  | 'backup_deleted'

export interface AuditActorRef {
  type: 'user'
  user_id: string
}

export interface AuditEntry {
  id: string
  actor: AuditActorRef
  action: { action: AuditAction; metadata: Record<string, unknown> | null }
  server_id: string
  /** Always null: one world per server. */
  world_id: null
  timestamp: string
}

export interface AuditLogPage {
  next_offset: number | null
  data: AuditEntry[]
  users: Record<string, { username: string; avatar_url: string | null }>
  addons: Record<string, { title: string; slug: string | null; icon_url: string | null }>
  versions: Record<string, { name: string; version_number: string | null }>
}

export interface AuditLogQuery {
  limit?: number
  offset?: number
  order?: 'asc' | 'desc'
  min_datetime?: string
  max_datetime?: string
  actor?: string[]
  action?: AuditAction[]
}

// ---------- Sign-in ----------

export interface LoginRequest {
  username: string
  password: string
}

export interface ChangePasswordRequest {
  current_password: string
  new_password: string
}

export interface CreateUserRequest {
  username: string
  password: string
  panel_role: PanelRole
  must_change_password?: boolean
  limits?: UserLimits
}

export interface UpdateUserRequest {
  username?: string
  panel_role?: PanelRole
  password?: string
  must_change_password?: boolean
}

export type DeleteUserServers = 'delete' | 'transfer'

export interface ApiError {
  error: string
  message: string
}
```

---

## 5. Open questions and assumptions

**Decided, with reasons**

1. **`editor` may write files.** `docs/PLAN.md:483` demands as acceptance "an editor can
   restart but cannot delete files". That is not available with Modrinth's role sets:
   their `editorScopes` contain `FILES_WRITE` (`access.vue:171-179`), and the description of the
   role reads "Manage instance content, files, backups, and other settings"
   (`messages.ts:76-79`). Rehanging the bits means changing the description texts in Modrinth's
   components — exactly what we do not want. **Proposal: rewrite the acceptance criterion onto
   `viewer`** ("a viewer can restart but cannot delete files"), and for `editor` you check
   "may delete files, but may not manage members and may not reset the server". That tests the
   same mechanism. **Somebody else has to confirm this.**
2. **Permissions as a string, not as a number.** Costs one `as unknown as number` cast in the
   provider (1.1), and saves 19-digit numbers in the database and the log.
3. **Roles are presets, not freely settable bits.** The API takes `role` and returns
   `permissions`. Setting individual bits would be possible but has no interface:
   the table knows only a select with three values (`AccessTable.vue:463-471`).
4. **Invitations need an acceptance.** Without one, "resend" would make no sense, and the
   interface shows "Pending", cancel and resend hard-wired
   (`AccessTable.vue:77-98`). So: inviting creates an entry without `joined_at`, and the
   invited person accepts (2.12) or declines (2.13).
5. **One endpoint for removing and canceling** (2.9), because the interface calls `delete`
   for both (`access.vue:511-513,545-547`).
6. **Deleting demands stopped servers.** The plan says "nothing gets shot down"; that holds
   for cleaning up too. The admin stops them first.
7. **A transfer may push the target user over.** The alternative would be to let the deletion
   fail on a budget that can only be adjusted afterwards — a chicken-and-egg problem.
8. **`must_change_password`.** Passwords set by an admin are passed on by word of mouth. If you
   do not want that, never set the field to `true`; the interface costs one dialog.
9. **`GET /users/search` is open to everybody signed in.** No search, no invitation. Only
   usernames are disclosed. Defensible on a machine among people who know each other; anybody who
   wants it tighter would need a switch in `config.toml`. **Not decided.**

**Gaps I could not close**

10. **"Resend" without a mailbox.** There is no mail delivery and no notification path
    outside the panel. The call only refreshes the timestamp. Anybody who wants real
    notifications needs either SMTP in `config.toml` or a notification surface in the panel —
    neither is in the plan. **A decision is needed.**
11. **No panel-wide audit log.** "Limit changed", "user created", "user deleted"
    are recorded nowhere, because `events/` has no renderer for them and a second log would be a
    second surface. Proposal for later: the same table with `server_id =
    null` and a plain list of its own in the user management.
12. **Admin actions are not recognizable as such in the log** (1.6). Modrinth's means for it
    carries a brand.
13. **`viewer` with and without the right to start cannot be told apart.** `BASE_READ` alone and
    `BASE_READ | POWER_ACTIONS` both give the role "Limited"
    (`permissions.ts:12-22`). If we want a real read-only role, the access page needs a
    fourth choice and therefore a component of its own instead of `AccessTable`. Today: `viewer`
    may start and stop.
14. **`cpu_mode: "share"` and the display.** In share mode `used_cores` can be above
    `limit_cores`; a bar reading "1.8 of 1.0 cores" looks like a bug but is right. The
    user management has to show that mode differently from a ceiling. **A design question, open.**
15. **Thresholds for the emergency brake.** `memory.max = memory.high × 1.25` is a proposal, not a
    measured figure. The plan only says "well above" (`docs/PLAN.md:258`). To be checked in P6.

**Borders with other areas**

16. `current_user_permissions` belongs in the server object and is delivered there (*Servers*
    area); format and computation are here (1.1/1.2).
17. Changing the owner of a **single** server (not as part of deleting a user) belongs to the
    server endpoint; the rules — stopped, re-owning through the helper, going over allowed — are
    the same as in 2.20.
18. `over_limit` blocks creating and starting. That is enforced in the *Servers* area; the
    error code there is `409 over_limit`.
19. `backupById` for the audit log comes from the *Backups* area, `addons`/`versions` from
    the *Content* area. Both may be missing; the display then falls back to IDs.

**Added during the review**

20. **Five routes we did not choose.** `parseAuditEvent` and `AccessTable` produce
    `/user/<name>`, `/project/<slug>/version/<id>`, `/hosting/manage/<id>/files` and
    `/hosting/manage/<id>/backups` hard-coded, with no prop against it (1.4.1, 1.4.3). Either our
    pages are called the same, or redirects are needed. The *Server frame* area decides that,
    not this one — but somebody has to decide. **Not decided.**
21. **The audit log's filter bar is work of our own.** Around 500 lines under `wrapped/`
    (`audit-log.ts:273`, `audit-log-utils.ts:73-185`), including one translation per action name.
    Without it `actor` and `action` from 2.14 cannot be operated. Not a contract problem, but an
    item that is not in the plan's "contract filler, ~200–400 lines per area"
    (`docs/PLAN.md:74`).
22. **`allocated.cpu_cores` in 2.15 adds up unlike things.** With `cpu_mode: "cap"` allocated
    cores are a ceiling, with `"share"` a share without an upper bound. The sum is still a number,
    but it means something only in pure cap operation. Either report two numbers or label the
    display. **A design question, open.**
23. **`GET /me` measures on every call.** `usage` comes from the cgroup files, and `used_cores`
    needs two measurement points. The 5-second cache (2.16) is only enough if a
    background tick measures anyway — the metrics socket brings one (*Servers* area).
    If that stands still, the first `used_cores` after a pause is `0.0`. Acceptable, but it
    belongs on the record.
