# Interface comparison against the original

As of 2026-08-13. Model: `/root/ref-modrinth` (Modrinth monorepo, `apps/frontend` = Nuxt web,
`apps/app-frontend` = Tauri Vite). Vendored library: `vendor/modrinth/`.

Everything here was measured. Every finding below carries file, line and the measurement that
proves it.

---

## 1. What was checked

| Check | method | result |
|---|---|---|
| Style load list and order | file-by-file comparison of `web/src/styles/global.scss` + `web/src/main.ts` against `apps/frontend/{nuxt.config.ts,src/assets/styles/*}` and `apps/app-frontend/{index.html,src/main.js,src/assets/stylesheets/global.scss}` | 1 finding (fixed), 2 deviations without effect |
| Tailwind `content` list | two Tailwind runs with identical input, once our `content` list, once a superset over `../vendor/**`; byte comparison | **no finding**, output byte-identical |
| SVG processing | `vite-svg-loader` applied to the same files with our configuration and with Modrinth's; then the shipped bundle counted | **1 finding, P0 — this is the reported icon fault** |
| `main.ts` setup | line comparison against `apps/app-frontend/src/main.js` + `plugins/i18n.ts` + `i18n.config.ts` + `directives/overlayScrollbars.ts` | no missing extension |
| CSS variables | all `var(--x)` in `vendor/modrinth/ui/src` against all `--x:` in omorphia + our code + Modrinth's app styles | 7 app variables, 6 of them with a fallback value or away from our surfaces |
| Classes without a definition | all classes from Modrinth's app CSS that neither omorphia nor Tailwind nor we supply, against the class usage in `vendor/modrinth/ui` | 1 real finding (`.universal-card`) |
| Missing `provide` | import graph from our 61 directly mounted components, 249 files transitively, all `inject*()` calls collected and held against our 15 `provide*()` | no gap on rendered paths |
| The claim "four surfaces rebuilt" | line and coupling measurement on `packages/ui/src/layouts/wrapped/hosting/manage/` and `components/servers/ServerListing.vue` | **3 of 4 proved, 1 disproved** |
| Our own pages against vendored counterparts | every file under `web/src/pages` and `web/src/layouts` against `vendor/modrinth/ui/src/{components,layouts}` | **3 rebuilds**, not one; addendum in section 4 |

---

## 2. Findings

### P0 — Every second icon ships cropped (`web/vite.config.ts:10`)

This is the fault the user reported. Evidence in three steps.

**Step 1 — the configuration deviates.** Both Modrinth applications hand `vite-svg-loader` an SVGO
configuration of their own:

```
/root/ref-modrinth/apps/frontend/nuxt.config.ts:100–109
/root/ref-modrinth/apps/app-frontend/vite.config.ts:53–60
        svgLoader({
          svgoConfig: { plugins: [{ name: 'preset-default', params: { overrides: {
            removeViewBox: false,
            cleanupIds: { minify: false },
          }}}]},
        })
```

Here `web/vite.config.ts:10` says nothing but `svgLoader()`. Without an argument SVGO runs with
`preset-default`, and in it `removeViewBox` is **switched on**.

**Step 2 — the effect on a single file.** The same loader, the same source
(`vendor/modrinth/assets/icons/play.svg`), once with our configuration, once with Modrinth's:

```
our configuration:       { xmlns, width: "24", height: "24", fill, stroke, ..., "stroke-width": "2" }
Modrinth's configuration:{ xmlns, width: "24", height: "24", fill, stroke, ..., "stroke-width": "2",
                           viewBox: "0 0 24 24" }
```

**Step 3 — why that destroys the rendering.** Modrinth's own stylesheet gives every SVG a CSS
size:

```
vendor/modrinth/assets/styles/defaults.scss:85   svg { height: 1em; width: 1em; }
vendor/modrinth/assets/styles/classes.scss:411   .btn svg { width: 1.25rem; height: 1.25rem; }
```

An `<svg>` without a `viewBox` has no mapping from user units to pixels. So the drawing in the
range 0–24 is **not scaled** but cropped to 16 px (or 20 px in a button): what stays visible is the
top left corner, around two thirds of the area. That is exactly what "somehow odd" describes.

**Step 4 — extent in the shipped bundle.** Count of the embedded SVG attribute objects in
`web/dist/assets/*.js`:

```
inline <svg> in the bundle:  280
  without viewBox:           157   (56 %)
  with viewBox:              123
```

The 123 intact icons are the ones where SVGO *has to* keep `viewBox` because `width`/`height` are
missing (25 files, among them `icons/cloud.svg`, `icons/cog.svg`, `icons/x.svg`, all under
`external/`). That is why some of the icons look right and the rest do not. It explains the
inconsistent impression.

**Second half of the same line: `cleanupIds: { minify: false }`.** Otherwise SVGO shortens the ids
per file to `a`, `b`, … Since all icons are embedded into the same document, identical ids collide,
and `url(#a)` takes hold of the first occurrence in the document. Proved on
`vendor/modrinth/assets/external/color/google.svg`: our configuration produces
`<clipPath id="a">`, Modrinth's keeps `<clipPath id="_clip1">`. In the current bundle: `id="a"` 1×,
`id="b"` 2×: the two `b`s can switch each other off.

**To do** (not my file, not changed by me) — replace `web/vite.config.ts:10` with:

```ts
plugins: [
	vue(),
	svgLoader({
		svgoConfig: {
			plugins: [
				{
					name: 'preset-default',
					params: {
						overrides: {
							removeViewBox: false,
							cleanupIds: {
								minify: false,
							},
						},
					},
				},
			],
		},
	}),
],
```

That is literally Modrinth's block from `apps/app-frontend/vite.config.ts:53–68`. Check after the
change: `web/dist/assets/*.js` must contain no SVG attribute object without a `viewBox` any more.

---

### P1 — `tailwind-utilities.css` was not loaded — FIXED

Both Modrinth applications load the same file, the Nuxt application before the `@tailwind`
directives, the Tauri application after them:

```
/root/ref-modrinth/apps/frontend/src/assets/styles/tailwind.css:1        (before)
/root/ref-modrinth/apps/app-frontend/src/assets/stylesheets/global.scss:5 (after)
    @import '@modrinth/ui/src/styles/tailwind-utilities.css';
```

We stand before, like the Nuxt application. No difference comes of it: none of the names in the
file is a Tailwind class name, there is nothing to override.

The file lies vendored under `vendor/modrinth/ui/src/styles/tailwind-utilities.css` and was never
imported by us. It contains:

- `--ease-out-expo`
- `.floating-expand-*` — the fade in and out for menus that unfold. Used in
  `components/base/DropdownFilterBar.vue:131,164`,
  `components/base/buttons/TeleportPopoutMenu.vue:126`,
  `layouts/shared/files-tab/components/FileContextMenu.vue:3`, the context menu in the file
  manager is one we mount.
- `.heading-xl` … `.heading-4xl`

Evidence beforehand, in the built CSS: `floating-expand` 0 hits, `ease-out-expo` 0 hits.

**Fixed** in `web/src/styles/global.scss:1`. Evidence afterwards:
`grep -c floating-expand-enter-from web/dist/assets/index-*.css` → 1, `ease-out-expo` → 1.

Without the file nothing visibly collapsed: with no transition definition Vue falls back to
duration 0. The menus snapped open instead of easing open. The class of fault was the same as with
the preflight finding: a file Modrinth loads and we do not.

---

### P2 — `.universal-card` is missing (error state of the content page)

`layouts/shared/content-tab/layout.vue:762` renders the error state as

```html
<div class="universal-card flex flex-col items-center gap-4 p-6">
```

`.universal-card` is **not** part of omorphia but of Modrinth's application-specific
`apps/frontend/src/assets/styles/components.scss`. That file is not a library, it does not lie
under `vendor/` and therefore cannot be vendored. Without it the error card has no background, no
border and no radius. The text stands loose on the page.

Full measurement: of 82 classes defined exclusively in Modrinth's app CSS, `vendor/modrinth/ui`
uses twelve; eleven of them are defined by the components in their own `<style>` block or lie on
surfaces we do not mount. `.universal-card` is the only real gap, and it hits exactly one place.

To do: take the rule from `components.scss` (the lines for `.base-card, .universal-card`) over into
`web/src/styles/global.scss`, nine declarations, all through omorphia variables:
`padding: 1rem; position: relative; min-height: var(--font-size-2xl);
background-color: var(--surface-3); border-radius: var(--radius-lg);
border: 1px solid var(--surface-4); margin-bottom: var(--gap-md);
outline: 2px solid transparent;`. Not entered by me, because a single card in the error state does
not justify the copy of somebody else's stylesheet as long as it is not decided whether
`components.scss` gets vendored as a whole.

---

### P3 — omorphia lies twice in the shipped CSS

`vendor/modrinth/assets/index.ts:9` contains `import './omorphia.scss'`. Whoever imports any icon
from `@modrinth/assets` already pulls in the complete base stylesheet.
`web/src/styles/global.scss:7–13` imports six of its seven parts a second time.

Measurement on `web/dist/assets/`:

| Marker | `index-*.css` | `ui-*.css` |
|---|---|---|
| `@font-face` | 5 | 5 |
| `--color-brand:` | 3 | 3 |
| `.card{` | 1 | 1 |
| `hljs` | 0 | 35 |

Both files are linked in the `<head>`. Compiled on their own the six parts come to 55 014 bytes
uncompressed; against 131 190 + 130 818 bytes of CSS in total that is around 20 % ballast.

Not fixed, because removing it turns the cascade order around: today omorphia stands after
`@tailwind utilities`, afterwards it would stand before. Checked which rules that would affect at
all. The intersection between omorphia classes and generated Tailwind classes is exactly two:
`.sr-only` (both hide, no difference) and `.table` (omorphia `display: grid` against Tailwind
`display: table`; `class="table"` is used nowhere in `vendor/modrinth/ui`). So the rebuild is safe,
but it belongs to task #17 (shrink the bundle) and needs a visual check, not the end of this round.

Side finding from the same check, **without** effect: our order of the parts deviates from
`omorphia.scss` (`defaults` before `classes` here, the other way round at Modrinth) and we import
`reset.scss` on top, although `defaults.scss:1` already pulls it. Both orders were compiled and
compared: five selectors appear twice (`html`, `body`, `h1`, `.btn`, `*`), not a single pair sets
the same property. No difference in the result. The double `reset.scss` import matches, by the way,
what `apps/frontend/src/assets/styles/global.scss:1` does as well.

---

### P4 — Modrinth's application supplies seven CSS variables, we do not

Full list (all `var(--x)` in `vendor/modrinth/ui/src` that neither omorphia nor we define, but that
Modrinth sets in its app styles):

What counts is not whether a component reads the variable but whether its rule stands in the
shipped CSS. Sorted by that (`grep` in `web/dist/assets/*.css`):

| Variable | used in | in the built CSS | effect here |
|---|---|---|---|
| `--medal-promotion-text-orange` | `ButtonFrame.vue:68`, `TeleportOverflowMenu.vue:109` | no | none, has the fallback `var(--color-orange)` |
| `--size-mobile-navbar-height` | `NotificationPanel.vue:271`, `FloatingActionBar.vue:295` | **yes, 2×** | below 750 px `bottom` fell away, the notification surface stayed at `bottom: 5rem` |
| `--size-mobile-navbar-height-expanded` | the same ones | **yes, 2×** | ditto |
| `--color-text` | `EnvironmentIndicator.vue:107` | **yes, 2×** | environment badge on the browse page without a text color |
| `--color-text-secondary` | `DatePicker.vue:1704,1708` | **yes, 2×** | arrow color in the date picker |
| `--spacing-card-xs` | `DoubleIcon.vue:21` | no | none, the component is not in the bundle |
| `--top-bar-height` | `PopupNotificationPanel.vue`, `NotificationStack.vue` | no | none, the components are not in the bundle |

**Fixed** for the four that are actually shipped, in `web/src/styles/global.scss` under `:root`.
Not with Modrinth's numbers: `--size-mobile-navbar-height: 3.5rem`
(`apps/frontend/src/assets/styles/global.scss:343`) measures their navigation bar **at the bottom
edge**. Ours stands at the top (`layouts/AppShell.vue:3`), and Modrinth's value would have lifted
the notification surface on narrow windows over a bar that does not exist here. Hence `0px`. For
the two colors we take the omorphia equivalents (`--color-base`, `--color-secondary`) instead of
Modrinth's fixed values, so that they follow the theme switch.

Not found, although looked for: `--gap-*`, `--radius-btn*`, `--text-*`, `--weight-*`, `--icon-*`
from Modrinth's "TO BE MOVED TO OMORPHIA" block are used **zero times** by `vendor/modrinth/ui`.
`--color-surface-4` and `--color-surface-5` (6× each) are defined nowhere in Modrinth's own source
tree either. The same behavior there as here.

---

### P5 — Inter comes from Modrinth's CDN

`vendor/modrinth/assets/styles/inter.scss` loads all five weights from
`https://cdn-raw.modrinth.com/fonts/inter/…`. That is their code unchanged, so not a comparison
fault, but for a panel on a machine without internet it means: fallback to
`-apple-system`/`sans-serif`, different metrics, everything looks "slightly off". And every page
load of a self-hosted tool reports in to Modrinth. To be decided outside this round: ship the fonts
along and override `inter.scss` in `global.scss` with `@font-face` blocks of our own using the same
family names.

---

## 3. What is expressly in order

**The Tailwind `content` list catches everything.** According to the assignment that was the most
likely further finding. It is disproved. Two runs of `tailwindcss` with identical input:

```
content: ['./index.html','./src/**/*.{js,ts,vue}',
          '../vendor/modrinth/ui/src/**/*.{js,ts,vue}','../vendor/modrinth/ui/index.ts']
  → 99 069 bytes, md5 5c97635a0086d7a6152915015e213f7f

content: ['./index.html','./src/**/*.{js,ts,vue,mdx,html}',
          '../vendor/**/*.{js,vue,ts,mdx,cjs,mjs}','!../vendor/**/node_modules/**']
  → 99 069 bytes, md5 5c97635a0086d7a6152915015e213f7f
```

Byte-identical. Reason: `vendor/modrinth/ui/src` contains only `.vue`, `.ts`, `.json`, `.png`,
`.css` — no extension the list misses; and in `assets`, `api-client`, `blog` there is not a single
`class=`. Modrinth's own list (`../../packages/**`) is broader, but no more productive.

A positive check on top: fourteen classes that occur exclusively in vendored components and in none
of our files, searched for in the built CSS: fourteen hits, among them `@[800px]:grid` and
`@[640px]:flex-row`, which also proves that `@tailwindcss/container-queries` from the preset takes
effect.

**`main.ts` is complete.** Against `apps/app-frontend/src/main.js` line by line:
`floating-vue/dist/style.css` ✓, `overlayscrollbars/overlayscrollbars.css` ✓, `VueQueryPlugin` ✓,
`router` ✓, `FloatingVue` with the same two themes `ribbit-popout` and `dismissable-prompt`,
identical values ✓, `i18nPlugin` ✓, directive `overlay-scrollbars` ✓. Missing: Pinia, Sentry,
`VueScanPlugin`, `i18nDebugPlugin` — Pinia is used **zero times** by `vendor/modrinth/ui` (no
`defineStore`, no `storeToRefs`), the other three are Modrinth's operational tools.
`web/src/directives/overlay-scrollbars.ts` against `apps/app-frontend/src/directives/overlayScrollbars.ts`:
only the identifier names differ, behavior identical. `web/src/i18n.ts` is the merged version of
their `plugins/i18n.ts` + `i18n.config.ts`.

**No missing `provide` on rendered surfaces.** From the import graph of our 61 directly mounted
components, 249 files transitively, 18 different `inject*()` places were found. Ten of them we
serve. The eight open ones (`injectFilePicker`, `injectTags`, `injectServerSettingsModal`,
`injectLoadingState`, `injectFileDrop`, `injectAppBackup`, `injectUserCountryContext`,
`injectStylesheet`) were looked up one by one:

- `injectStylesheet` is a false alarm, a class method in
  `layouts/shared/console/composables/log-highlight-addon.ts`, not a Vue `inject`.
- Six are called with `null` as a fallback (`createContext` in
  `vendor/modrinth/ui/src/providers/create-context.ts:56–73` throws only without a fallback).
  `layouts/shared/files-tab/layout.vue`, for one, checks `if (filePicker?.pickFiles)` afterwards
  and otherwise uses the document dialog.
  But a fallback does not mean "without consequence", and exactly one of them costs something
  visible: `content-tab/components/modals/content-updater-modal/index.vue:313` calls
  `injectTags(null)` and on `null` skips the collapsing of the game versions (`:611`); in the
  update dialog of the content tab, which we mount, it then reads
  `1.20.1, 1.20.2, 1.20.3, 1.20.4` instead of `1.20.1–1.20.4`. The tags have long been there
  (`providers/browse-manager.ts` reads them); all that is missing is a `provideTags` in the host.
  Stands in section 5.
- `injectFilePicker()` and `injectTags()` **without** a fallback stand only in
  `layouts/shared/server-settings/pages/installation.vue`. This page lands in the bundle through
  the collective export `server-settings/pages/index.ts`, but is not rendered by us: we mount
  `ServerSettingsPropertiesPage` (`properties.vue`) and `InstallationSettingsLayout`
  (`layouts/shared/installation-settings/layout.vue`).

**The theme switch is right.** `variables.scss:398,424` use `@extend .dark-mode`; in the compiled
output the selector list `.dark-mode, .retro-mode, .oled-mode, .dark, :root[data-theme=dark]`
carries the dark values. `web/src/composables/theme.ts:24–27` sets exactly one of these classes on
the `<html>` plus `color-scheme`. `html { @extend .light-properties }` supplies the light defaults,
the class wins on specificity. Correct.

**`#teleports` is there.** `web/index.html:10`. Used by `DropdownFilterBar.vue:130,163`,
`MultiSelect.vue:110`, `Combobox.vue:99`, `FileContextMenu.vue:2`, `FloatingPanel.vue:194`.

**No color literals in our code.** `web/src` searched for `#rgb`, `rgb(`, `rgba(`, `hsl(` — zero
hits. No `<style>` block in any of our `.vue` files: 0 lines of CSS of our own.

---

## 4. "Is the UI really identical to the original?"

Honestly: **the building blocks yes, the composition no.** With numbers.

### What is literally their code

| Package | Files | Lines |
|---|---:|---:|
| `vendor/modrinth/ui/src` | 501 | 87 055 |
| `vendor/modrinth/api-client/src` | 111 | 14 491 |
| `vendor/modrinth/assets` | 16 | 4 358 |
| `vendor/modrinth/utils` | 11 | 2 390 |
| **Total** | **639** | **108 294** |

Unchanged, no patch, no copy with a change. Directly mounted: **61 of their `.vue` components**
plus 38 named imports from `@modrinth/assets` (icons and illustrations); through the import graph
132 of their components are reachable.

### What is our code

| Area | Files | Lines |
|---|---:|---:|
| `web/src/pages` | 18 | 6 599 |
| `web/src/providers` (the contract fillers) | 7 | 3 636 |
| `web/src/api` (client, types, socket — no interface) | 6 | 3 312 |
| `web/src/layouts` | 3 | 638 |
| `web/src/composables` | 5 | 349 |
| `web/src/directives` + `styles` | 2 | 64 |
| **Total** | **45** | **14 846** |

Without `api/` that leaves **11 534 lines of interface code**. Ratio of our own to their interface
code: **1 : 7.5**.

### What has no model at all

Nothing in the interface. Every one of our pages is composed of their components; the listing of
the Modrinth imports per file stands in section 6. What has no model is the REST interface
underneath, and according to `docs/PLAN.md` that is wanted exactly this way.

### The claim "four surfaces rebuilt because Modrinth's versions hung on billing"

Verified, not taken over. Occurrences of `billing`, `subscription`, `stripe`, `pyro`, `purchase`,
`Plan`, `node` were counted in the removed files:

| Surface | Modrinth's file | Lines | billing coupling | our version | verdict |
|---|---|---:|---|---|---|
| Server list | `layouts/wrapped/hosting/manage/index.vue` | 968 | `billing` 16, `subscription` 23, `purchase` 12, `Stripe` 9, `Plan` 10 | `pages/servers/Index.vue`, 513 lines | **proved** |
| Server card | `components/servers/ServerListing.vue` | 654 | `subscription` 24, `suspend` 14, `Billing` 5 | contained in `Index.vue` | **proved** |
| Server frame | `layouts/wrapped/hosting/manage/root.vue` | 1 627 | `node` 25/`Node` 7, `stripe` 4, `Billing` 4, `pyro` 6 | `layouts/ServerFrame.vue`, 327 lines | **proved** |
| Creation flow | `components/flows/creation-flow-modal/` | 2 684 | **zero** | `pages/servers/New.vue`, 926 lines | **disproved** |

**The fourth point is not right.** `CreationFlowModal` lies fully vendored under
`vendor/modrinth/ui/src/components/flows/creation-flow-modal/` (14 files, 2 684 lines), is free of
billing and works to the same contract pattern as the `shared/` layouts: pure properties in,
`create` event out (`index.vue:22–58`). The stages `SetupTypeStage`, `CustomSetupStage`,
`ModpackStage`, `FinalConfigStage`, `ImportInstanceStage` are there. The caller is there too:
`components/servers/ServerSetupModal.vue` (234 lines).

Two contracts are missing for mounting it, which `CustomSetupStage.vue` and `ModpackStage.vue`
demand without a fallback:

- `provideTags({ gameVersions, loaders })` — `vendor/modrinth/ui/src/providers/tags.ts`.
  `web/src/providers/installation-settings.ts` fetches both today already.
- `provideFilePicker({ pickImage, pickFiles?, pickModpackFile })` —
  `vendor/modrinth/ui/src/providers/file-picker.ts`. `pickFilesFromDocument` already exists in
  `web/src/composables/`.

Estimated effort: around 40 lines of contract filler against 926 lines that fall away. That is the
biggest open point of this comparison after the icon fault, but it does not belong in my files and
is an assignment of its own.

### Addendum: `New.vue` is not the only rebuild

The four surfaces above were the claimed ones. A pass over *every* page of our own against
`vendor/modrinth/ui/src/{components,layouts}` finds two more. Both lie under
`layouts/shared/server-settings/pages/`, the same directory we already mount `properties.vue` from
(`pages/servers/Settings.vue:8`):

| Our file | Lines | vendored counterpart | Lines | billing coupling | verdict |
|---|---:|---|---:|---|---|
| `pages/servers/settings/Network.vue` | 420 | `server-settings/pages/network.vue` | 447 | **zero** | rebuild |
| `pages/servers/settings/Advanced.vue` | 326 | `server-settings/pages/advanced.vue` | 425 | **zero** | rebuild |
| `pages/servers/settings/General.vue` | 378 | `server-settings/pages/general.vue` | 421 | `billing` 2, `subscription` 4, `pyro` 2, `node` 2, `.modrinth.gg` hard-wired (l. 252–330) | own version justified |
| `pages/servers/settings/Installation.vue` | 37 | `installation-settings/layout.vue` | — | — | mounted, not a rebuild |

`network.vue` and `advanced.vue` take no properties and emit no event (`defineProps`/`defineEmits`:
zero hits in both). They fetch nothing but `injectNotificationManager`,
`injectModrinthServerContext` and `injectModrinthClient`. We provide all three already;
`properties.vue` runs over exactly the same ones. What is missing is two adapter modules in the
form that `web/src/composables/archon-adapters.ts` builds three times already:

- `archon.options_v1.getStartup/patchStartup` for `advanced.vue:281,388`
- `archon.servers_v0.getAllocations/reserveAllocation/deleteAllocation/updateAllocation`
  for `network.vue:259,312,351,369`

`worldId`, which `advanced.vue:232,282` reads, `web/src/providers/server-context.ts:498` provides
already. That puts 746 lines of our own against 872 vendored ones that do the same.

So the right number: **three** surfaces rebuild something that is available vendored: `New.vue`,
`Network.vue`, `Advanced.vue`. All three belong in assignments of their own, none of them in the
files of this comparison.

### What could not be vendored

`packages/ui/src/layouts/wrapped/` as a whole (5 111 lines) and
`components/servers/{ServerListing,ServersPromo,ModrinthServersIcon}.vue` + `marketing/`, removed
while debranding, see `COPYING.md`. On top of that Modrinth's application-specific stylesheets
(`apps/frontend/src/assets/styles/{global,layout,utils,components}.scss`, 35 141 bytes compiled
together), which are not a library and therefore do not lie under `vendor/`. What of that is
actually missing stands in P2 and P4: one class (open) and four variables (fixed).

---

## 5. Changes to other people's files that are needed

| File | Line | Change | Urgency |
|---|---|---|---|
| `web/vite.config.ts` | 10 | `svgLoader()` → `svgLoader({ svgoConfig: … })` with `removeViewBox: false` and `cleanupIds.minify: false`, literally as in `apps/app-frontend/vite.config.ts:53–68` | **P0 — fixes the reported icon fault** |
| `web/src/pages/servers/New.vue` | all | replace with `CreationFlowModal` from `vendor/modrinth/ui`, plus `provideTags` and `provideFilePicker` | P2, own assignment |
| `web/src/pages/servers/settings/Network.vue` | all | replace with `ServerSettingsNetworkPage`, plus the `archon.servers_v0` allocations in `archon-adapters.ts` | P2, own assignment |
| `web/src/pages/servers/settings/Advanced.vue` | all | replace with `ServerSettingsAdvancedPage`, plus `archon.options_v1` in `archon-adapters.ts` | P2, own assignment |
| `web/src/composables/modrinth-host.ts` | — | `provideTags` once in the host: `ContentUpdaterModal` (content tab, mounted) calls `injectTags(null)` and without tags leaves the game versions unabridged (`content-updater-modal/index.vue:611`) | P3 |
| `web/src/main.ts` | — | no change needed | — |
| `vendor/**` | — | no change needed | — |

---

## 6. Touched

- `web/src/styles/global.scss`, four changes against `HEAD`:
  1. `@import '@modrinth/ui/src/styles/tailwind-utilities';` (extension left off so that Sass
     embeds the file instead of producing an `@import url()`) — P1 above.
  2. `@import '@modrinth/assets/styles/reset.scss';`, like
     `apps/frontend/src/assets/styles/global.scss:1`.
  3. `@layer base { a { color: inherit; text-decoration: none } }`, Modrinth's preset switches
     `preflight` off (`tailwind-preset.ts:259`), and both applications set the rule themselves
     (`apps/frontend/…/global.scss:383`, `apps/app-frontend/…/global.scss:87`). Without it Chrome's
     blue underline would stand under every tab.
  4. The four shipped variables under `:root` — P4 above.
- `docs/INTERFACE-PARITY.md` — this document.
- `web/tailwind.config.ts` — not touched, no fault found in it.

`pnpm --filter @craftpanel/web build` runs green.

### Modrinth components per surface of ours

| Our file | Lines | mounted Modrinth components |
|---|---:|---|
| `layouts/AppShell.vue` | 152 | `Avatar`, `NewModal`, `TeleportOverflowMenu`, `ThemeSelector` + 7 icons |
| `layouts/ServerShell.vue` | 159 | `ErrorInformationCard`, `LoadingIndicator`, `NotificationPanel` + 5 icons |
| `layouts/ServerFrame.vue` | 327 | `Admonition`, `Badge`, `Button`, `CopyCode`, `NavTabs`, `PageHeader`, `PageHeaderActions`, `PanelServerActionButton`, `ServerIcon`, `ServerInfoLabels`, `ServerNotice`, `ServerPanelAdmonitions` + 7 icons |
| `pages/servers/Index.vue` | 513 | `Admonition`, `Avatar`, `Badge`, `Button`, `ButtonLink`, `CopyCode`, `EmptyState`, `LoadingIndicator`, `ProgressBar`, `ServerIcon`, `ServerInfoLabels`, `SmartClickable`, `StatItem`, `StyledInput` + 7 icons |
| `pages/servers/New.vue` | 926 | `Admonition`, `Button`, `ButtonLink`, `Card`, `Checkbox`, `Chips`, `Combobox`, `LoadingIndicator`, `ProgressBar`, `Slider`, `StyledInput` + 8 icons — **rebuilds `CreationFlowModal` all the same** |

---

## 7. The seam in operation — what the vendored code prescribes to our code

Addendum, 2026-08-15. Sections 1–6 measure the **adoption**. This one collects what the vendored
building blocks impose on our own code at **run time**: types that have to stay congruent,
hard-wired places in the vendored code, and the two measurements that correct our pages against
Modrinth's building blocks. These sentences stood as comments in `web/src/**` until today and
nowhere else. Line numbers of our files are the state before that; those of the vendored code stand
firm as long as `vendor/` is not swapped.

### 7.1 Types that have to stay congruent

`web/src/api/types.ts` writes the contract down. In these places the shape is **not free**, though,
because a vendored component reads it:

| Our shape | their shape | what happens on a deviation |
|---|---|---|
| `ServerRole` | `ServerAccessRole`, `components/servers/access/types.ts:5` | directly assignable; otherwise an `as` at every place it is used |
| the ten permission bits | `composables/server-permissions.ts:15-32` | names literal; unknown ones are discarded there **silently** — a typo turns into "permission missing" |
| permission mask as text, bit names joined by `' \| '` | `parsePermissionString` calls `value.split('\|')` | no array; a list would arrive there as one element |
| `FileEntry` | `FileItem`, `files-tab/types.ts:1` | directly assignable; `mtime` in Unix seconds is binding through `FileTableRow.vue:303` |
| `ExtractDryRun` | `ExtractDryRunResult`, `files-tab/types.ts:64` | congruent |
| `PropertiesFields` | literally Modrinth's shape; the creation wizard builds it (`creation-flow-context.ts:524-541`) | — |
| `LoaderVersion` | `LoaderVersionEntry`, `installation-settings/types.ts:29-36` | **not** congruent: the contract calls it `channelTag`, knows no `null` and no `released`. Our page does the renaming |
| Backup queue | `Archon.BackupsQueue.v1.BackupQueueOperation` except for `operation_id` | the field is declared there as `number` (nullable) and carries a ULID here, passed through in the adapter as `unknown as number` (10.1). Nothing computes with it — whoever computes with it computes with `NaN` |
| Resend invitation | `Archon.ServerUsers.v1.ReinviteResponse` | — |
| Audit log | `Archon.Actions.v1.*` | so that `parseAuditEvent` runs unchanged |
| `environment` | `Labrinth.Versions.v3.Version.environment` | `null` leaves the warning triangle out |
| Dependency resolution | `labrinth/types.ts:41-50` | the same values |
| `newLoaderVersion` | `ContentDiffPreview` is not nullable there | the provider turns it into `''` |
| Bulk progress | `BulkOperationStatus.progress` is a **count** | it is the denominator of the bar, not a share |
| `PowerState` | value-identical with Modrinth's | the translation table stays all the same, so that a drift apart breaks the translator and not the interface. What differs is the surroundings: `installing` is a `Server.status` here and never a run state, an OOM kill arrives as `crashed` with `oom_killed: true` (4.6, 13.4) |
| `Server.status` | `Archon.Servers.v0.Status` knows no `deleting` | the value goes out unchanged; the vendored components compare only against `installing` and `suspended` |
| `SyncProgress`, `ContentError` | from `InstallingBanner.vue`, **not** exported as a type there | rebuilt field for field and thereby assignable to the props (13.4) |

Two places are more dangerous than the rest:

* **Lock reasons (5.10).** Four of the five identifiers are compared as strings in the vendored
  code. According to the contract an unknown code counts as harmless; that only holds with our
  fallback description: `formatMessage` and `ServerPanelAdmonitions.vue:64` read `reason.id`
  **unchecked**, and a missing entry would tear the whole server page down.
* **Loader display names (9.11).** `ServerSetupModal.vue:98-100` compares against `'Vanilla'` and
  otherwise calls `toLowerCase()`. Every name has to give the `LoaderId` again in lower case.
  Folia, Leaf and Velocity are missing from Modrinth's union; the display name goes out all the
  same, because the vendored components only lower-case it.

On top of that the numbers we did not choose ourselves: `CONSOLE_CLIENT_BUFFER_LINES` is mclo.gs'
`maxLines`, `CONSOLE_CLIENT_BUFFER_BYTES` lies below their `maxLength`, and the answer from
`POST https://api.mclo.gs/1/analyse` deviates from Modrinth's type on purpose: `name` and `version`
are nullable, because the real API delivers `null` there as soon as it does not recognize the
loader or the version (measured on 2026-08-12).

### 7.2 What the vendored code hard-wires

| Their place | what it does | our answer |
|---|---|---|
| `ServerSubdomainLabel.vue:18` | hard-appends `.modrinth.gg` | `domain` stays `""`, so that the label stays invisible |
| `BackupItem.vue:116` | assembles the download address itself, with a fixed `https://` | on a panel without TLS only a dead link would be left — our own button stands next to it |
| `ServerManageStats.vue:196` | links the storage tile hard to Modrinth's own path | rewrite in `router.ts`, otherwise the click lands on our not-found page |
| `server-settings/pages/properties.vue:320` | links to the file page with `?path=&editing=` | fallback route in `router.ts`, read in `pages/servers/file-link.ts` |
| `browse-tab/layout.vue:323` | wires the author of every card to Modrinth's paths, `AutoLink` turns that into a router link | a jump of our own, the same tab as with the project links |
| `ServerLoaderLabel.vue:6` | shows a loading shimmer on `null` that never ends | without an installation our sentence stands underneath: nothing is loading, there is nothing |
| `LoadingIndicator` | positions its heading absolutely | without an anchor it lands at the page edge |
| `BaseTerminal:9` | holds a curtain while a backlog is expected | a blind `true` would leave it lying there for ever — the second time the console tab is entered no backlog comes any more (13.2) |
| `console/layout.vue:343-347` | redraws the whole terminal as soon as the array gets shorter | trimming happens 20 % below the limit, so once every 5 000 lines instead of at every block |
| `console/layout.vue:380-384` | draws in one go at the end of the backlog | do not let it draw over `loading`: if you attach in the middle of the backlog, the flag already stands at `false` and the watcher never starts |
| `console/layout.vue:239-241` | the tooltip overwrites the placeholder text of the input field | with "server is not running" the tooltip stays empty (15.4) |
| `FileNavbar.vue:404` | recognizes mclo.gs by the path **without** a leading `/` | 15.3: the path goes in without a leading `/` |
| `BackupCreateModal.vue:164`, `use-inline-backup.ts:136` | recognize the throttling **by the text** of the error message | the adapter delivers exactly that text |
| `content-updater-modal/index.vue:227` | locks its update button while the selection is the installed version | when updating, the new one is preselected, when changing version, the installed one |
| `ContentCardTableItem.project` | is mandatory and is read unchecked | if Modrinth's card is missing, the replacement card at least carries the project id, so that `/mod/<id>` resolves; only title and icon are missing then |
| `use-browse-search.ts:212` | deep watcher on the filters, resets the pagination | `providedFilters` must not be rebuilt at every `content_changed` — whoever is reading on page four would otherwise stand on page one again |
| `use-browse-search.ts:226,317` | debounce without `signal`, then `router.replace` | the search does not run through `projects_v3.search()` but through a call of our own with an abort signal; otherwise an outdated query writes its parameters into the address of the page the user is standing on by now |
| `Table.vue` | hands its rows out as `unknown` | through the index the type is preserved |
| `use-installation-form.ts:200` | swallows every throw out of our save | without a message of our own the save button would stand there wordless |
| `ServerSettingsModal.vue:104` | uses particular message ids | our settings page uses the same ones |
| `content/index.vue:556` | the model: a rejected file drops out, the rest go up | otherwise one "no" costs the whole selection |

Two measurements that correct our pages against their building blocks, both at 390 px:

* **`NavTabs` is `w-fit overflow-x-auto`**, and in a column `fit-content` settles on the content
  width instead of on the space: the bar was 690 px wide, so it had nothing to scroll, and Backups,
  Access and Settings lay out of view and out of reach. That is why Modrinth wraps it in exactly
  one box: the wrapper takes the width, the bar scrolls inside it
  (`hosting/manage/root.vue:232-239`). Our frame and the settings page do the same.
* **The right-hand group of the content page's toolbar** (`content-tab/layout.vue:897`) stands on
  `flex items-center gap-2` without wrapping, its buttons are `shrink-0`. As soon as "Update all"
  joins in, it measures 389 px together with "Alphabetical" and "Refresh" and thus stands 23 px
  beyond the 342 px a 390 px device gives (measured on a Paper with twelve plugins;
  `scripts/mobile-check.py` reported an inflated viewport of 413 px for it). It may wrap, its own
  row does so already (`flex-wrap` on the parent); the rule stands in our `<style>` block, the
  vendored code stays untouched.

### 7.3 The foreground over Modrinth's client (9.18)

`composables/modrinth-host.ts` hooks into `request()` and not into `buildUrl()`, because
`buildUrl()` sees neither method nor body; `buildUrl` glues `<root><version><path>` together, and
our paths start at `/api/v1`. A path the table does not know is **not** passed on: that would run
Archon's path against our backend and would give a `404` that the caller would take for a statement
about their data.

`composables/archon-adapters.ts` lays the foreground over it. Four rules for that:

* The spread evaluates the getters and freezes the remaining modules as fields; `labrinth` and
  `kyros` the foreground inherits over the prototype chain.
* `inject` always reads the parent, never your own provision: the foreground may only be fetched
  in the `setup` of a page, and only above the components that fetch the client.
* The contracts (15.6) are fetched by the console, the editor, the access page and the settings
  pages **without a fallback value**: if one is missing, rendering already throws. The fourth, the
  notification basket, hangs on `AppShell.vue` and thereby also covers the pages outside a server:
  copy buttons also stand on the account page, the administration page and the server list.
* `ServerFrame.vue` provides the table and not the tabs: `ServerPanelAdmonitions` addresses
  `backups_queue_v1` itself, and without the table the frame itself would already run into
  "No panel route for".

One header too many costs something visible here: **no headers of our own and no `userAgent`**: a
single extra header forces the preflight at mclo.gs that their `POST /1/log` does not answer (6.7).

### 7.4 Two ways end in the vendored code

At the config button of the content row and at the lock on the search page the last links belong to
Modrinth: the row has exactly one place meant for a button of its own (`getOverflowOptions`), and
the lock takes effect at the button, at the selection and at the bar below. If one of them falls
away in a vendor update, our surface disappears **silently**. Against that stand
`pages/servers/config-button-reachable.test.ts` and `providers/browse-manager.test.ts`: text guards
that drop a line exactly then.

### 7.5 Two small things with consequences for publication

* **We never use `type: 'support'`.** The icon for it is somebody else's trademark.
* **`cycleValue` from `@modrinth/utils` would do the same as our three lines**, but it drags a
  package without types into `vue-tsc` and with it the whole vendor tree full of `implicitly any`.
