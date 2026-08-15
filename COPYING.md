# Copying

Copyright © 2026 the CraftPanel authors.

CraftPanel is licensed under the **GNU General Public License, Version 3 only**
([LICENSE](./LICENSE)).

The choice is not ours to make freely. The user interface is built on Modrinth's UI library,
which is GPL-3.0-**only**, not "or later". Anything derived from it inherits that, which rules
out AGPL and GPL-2 as well.

**Two things this licence does not cover**, and both are further down. One is Modrinth's brand —
the logo, the wordmark, the Rinthbot mascot and the Modrinth Servers icon. It is trademarked, was
deleted from this repository and may not come back. The other is `vendor/modrinth/api-client`,
which is LGPL-3.0.

## Third-party code in this repository

`vendor/modrinth/` contains a copy of parts of [modrinth/code](https://github.com/modrinth/code),
taken at commit `2a43792f` (2026-08-11). It is not published on npm, so it is vendored rather
than installed.

| Package | Licence | Where it says so |
|---|---|---|
| `vendor/modrinth/ui` | GPL-3.0-only | `ui/LICENSE`, `ui/COPYING.md` |
| `vendor/modrinth/assets` | GPL-3.0-only, minus the branding noted below | `assets/LICENSE`, `assets/COPYING.md` |
| `vendor/modrinth/api-client` | LGPL-3.0-only | `api-client/LICENSE`, its `package.json` |
| `vendor/modrinth/utils` | GPL-3.0-only | `utils/LICENSE` |
| `vendor/modrinth/blog` | ours, not theirs (see below) | — |
| `vendor/modrinth/tooling-config` | GPL-3.0-only | no licence file of its own, upstream either |

`vendor/modrinth/blog` is **not** a copy of Modrinth's blog package. It is a fourteen-line stub we
wrote, because `@modrinth/ui` imports `@modrinth/blog/changelog` and would not resolve without it:
it declares the same two types and returns an empty changelog. Modrinth's actual blog — the
articles, the compiled content, the images — is not in this repository and could not be, because
their `packages/blog/COPYING.md` reserves all rights to exactly that content.

The copy is not verbatim. Modrinth's own hosting surface (the purchase flow, billing, plan
selection and the pages that wire them together) has been removed, because this project has no
such thing.

## Mail templates

`crates/craftpanel/src/mail/templates/` is built from Modrinth's own email templates
(`apps/frontend/src/templates/emails/**` and `templates/shared/StyledTemplate.vue` in
[modrinth/code](https://github.com/modrinth/code), GPL-3.0-only like the rest). Taken over: the
header/card/footer layout, the "if the button does not work, here is the URL" block, the
`.ExternalClass` corrections for Outlook.com, the defusing of `a[x-apple-data-detectors]`, the type
scale and the button shape. Rewritten as plain HTML with `include_str!` rather than copied as Vue
components; the reason is in `docs/MAIL.md` 4.2.

Not taken over, and not to be reintroduced: the logo and the seven social icons (branding, see
below), the postal address of a company that is not us, and the Google Fonts link. The mails carry
no images at all.

## What was removed, and why it may not come back

Modrinth's **brand is not covered by the GPL**. Their `COPYING.md` reserves all rights to the
wrench-in-labyrinth logo, the wordmark, the Rinthbot mascot and the Modrinth Servers icon.
Those files are deleted from this repository and must stay deleted:

- `ui/src/components/brand/`
- `ui/src/components/servers/ModrinthServersIcon.vue`
- `ui/src/components/servers/marketing/`
- `assets/branding/`

`scripts/check-no-branding.sh` fails the build if any of them reappears, by path, by filename or
by identifier. That is trademark law, and it holds regardless of the GPL.

CraftPanel is not affiliated with or endorsed by Rinth, Inc. Nor with Mojang or Microsoft: the
name points at the game, and Minecraft is their trademark, not ours.
