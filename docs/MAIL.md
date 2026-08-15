# Sending mail through Resend

As of 2026-08-13. The contract is in `docs/api/CONTRACT.md` section 19; here are the
measurements, the decisions and the ways to look at the mails without a Resend account.

**The panel can deliver a mail without a missing key bringing anything down, and the operator
sees the mails even when they have no Resend account at all yet.**

---

## 0. The three ways to look at the mails right now

None of the three needs a key, a network or a Resend account.

1. **On the machine, without the panel and without a database:**

   ```
   craftpanel mail preview --out /tmp/craftpanel-mail
   ```

   Writes sixteen files (eight mails, `.html` and `.txt` each) and prints the paths on
   standard output. `--kind reset_password` writes only one. Without `--out`,
   `/tmp/craftpanel-mail` is the default. In the tree without an installation:
   `cargo run -p craftpanel -- mail preview --out /tmp/craftpanel-mail`.

2. **In the panel:** Administration → Mail → "Look at the mails" — eight links, each opens
   `GET /api/v1/admin/mail/preview/<kind>` in a new tab (administrators only, `text/html`).

3. **Instead of sending:** if `CRAFTPANEL_MAIL_SINK=/path` is set, every mail becomes two
   files (`0600` in a `0700` directory) and **no** request goes onto the network. That way the
   whole sign-up runs through, clickable verification link included, without a Resend account.
   The panel writes a warning line about it at startup, and the admin surface shows it as a
   state of its own (`state = "file_sink"`); a silent redirection would be a trap.

And the fourth way, if a mail really did go out: `GET /admin/mail/outbox/{id}/content`
delivers the HTML that was sent, as long as the mail is not delivered (after that the body is
deleted, 19.7).

---

## 1. Resend, measured instead of guessed

Every row marked "measured" was checked against `api.resend.com` on 2026-08-13; the rest are
in Resend's documentation (URL in the last column).

| Item | Finding | Source |
|---|---|---|
| Endpoint | `POST https://api.resend.com/emails` | resend.com/docs/api-reference/emails/send-email |
| Auth | header `Authorization: Bearer re_…` | ibid. |
| Required fields | `from`, `to`, `subject` | ibid. |
| `to` | string or array, at most 50 | ibid. |
| Answer on success | `{"id":"49a3999c-…"}` — only the id | ibid. |
| Answer on error | `{"statusCode":…,"name":"…","message":"…"}` | **measured** |
| Missing key | `401` `missing_api_key`, `"Missing API Key"` | **measured** |
| Wrong key | `401` **`validation_error`**, `"API key is invalid"` | **measured** |
| Double-send protection | header `Idempotency-Key`, valid for 24 hours; same key + same body ⇒ the same answer without a second mail | resend.com/docs/dashboard/emails/idempotency-keys |
| Rate limit | 10 requests per second per team | resend.com/docs/api-reference/introduction |
| Free plan | 3,000 mails/month, 100/day, 1 domain | resend.com/pricing |
| Key permissions | `sending_access` ("Can only send emails") is enough | resend.com/docs/api-reference/api-keys/create-api-key |

**The most important measurement is the fourth from the bottom, and it contradicts the
documentation:** a wrong key comes back as `401` with the name `validation_error`, not as
`403 invalid_api_key`. But the same name `validation_error` also stands for "domain not
verified" (`403`). A mapping that reads the name first would therefore advise the operator to
verify a domain while their key is wrong. `mail/resend.rs::interpret` therefore reads **the
status first, then the name, then the text**, and `resend.rs`'s `#[ignore]` test against the
real service is the place that notices when Resend changes that:

```
cargo test -p craftpanel -- --ignored the_real_service
CRAFTPANEL_RESEND_KEY=re_… cargo test -p craftpanel -- --ignored the_real_service
```

### 1.1 What works without a verified domain — and what does not

As long as no domain is verified at Resend:

* The sender may only be `onboarding@resend.dev` — that is why it is the default.
* The recipient may only be **the address the Resend account was opened with**.
* Everything else: `403 validation_error` with "…domain is not verified" or "…own email
  address".

So on the first day, without a domain, the operator can press the "Send test mail" button and
see that it works. Sign-up mails to other people only go out after the domain is verified (MX
and TXT records for SPF and DKIM at resend.com/domains). The surface says both; the error case
says it again.

**The panel does not build domain administration.** It would need `full_access`; with a
sending key every domain query would be `401 restricted_api_key` and the surface permanently
red. The state "domain not verified" is not queried but **learned** from the answer to a send.

---

## 2. Where what lies

| File | What is in it |
|---|---|
| `mail/mod.rs` | the service: `configured` / `send` / `notify`, the rate limits, the state, the cleaning of header values |
| `mail/resend.rs` | the client, `ApiKey`, `MailError` and the mapping table from 19.11 |
| `mail/store.rs` | `mail_settings` and `mail_outbox`, every query exactly once |
| `mail/key.rs` | the key file: `0600` in `0700`, written and renamed |
| `mail/message.rs` | the eight mails, their values, their subjects — and **the building of the links** |
| `mail/render.rs` | shell, templates, escaping, example values, the palette test |
| `mail/queue.rs` | the worker, the wait times, the daily-limit case |
| `mail/sink.rs` | `CRAFTPANEL_MAIL_SINK` |
| `mail/cli.rs` | `craftpanel mail preview` |
| `mail/templates/` | `shell.html`, `shell.txt`, `manual_link.html` and eight pairs |
| `mail/testdata/` | Resend's answers, one file per row of the table |
| `api/mail.rs` | the eight endpoints |
| `web/src/api/mail.ts` | the shapes and the calls of the surface |
| `web/src/pages/admin/mail.ts` | the computing part of the surface (without Vue, with a test beside it) |
| `web/src/components/mail-words.ts` | the words and colors of the states |
| `web/src/pages/admin/Mail.vue` | the surface |

No new dependency, in neither of the two trees.

---

## 3. The key

The model is playit (`0008_playit_per_user.sql`, `playit/agent.rs:345-371`):

```
<data_dir>/mail/            0700
<data_dir>/mail/api_key     0600
```

Writing goes to `api_key.part`, then `sync_all`, then `rename`: half a file is never read.
**Not one character** of the key stands in the database, not even a stub like `re_…AB12`: with
exactly one key in the whole panel a leftover hint gains nothing and would be a partial secret
in every copy of the database. What is shown is "stored on ⟨date⟩" (`key_set_at`).

The test `the_key_is_a_file_and_stands_in_no_column_of_the_database` reads **every column of
every table** and looks for the key text. Counter-check (`mark_key` also writes the key into
`mail_settings.reply_to`): exactly that test fails with "mail_settings.reply_to carries the
key".

`state` reads the **file**, not `key_set_at`: a row that claims a key while the file is gone
would have the panel call outward with nothing.

---

## 4. The design

### 4.1 Where it comes from

Shell and palette come word for word from Modrinth's own mail templates
(`/root/ref-modrinth/apps/frontend/src/templates/emails/**`, GPL-3.0-only like the rest; noted
in `COPYING.md`): the split into header/card/footer, the block "if the button does not work,
here is the URL", the `.ExternalClass` corrections for Outlook.com, the defusing of
`a[x-apple-data-detectors]`, the font-size set and the button shape (12 px radius, 14 px bold,
padding 12/16).

**Did not come along:** the logo, the seven social icons, the postal address of a company we
are not, and the Google Fonts link. A test (`nothing_of_somebody_elses_brand_and_no_image_at_all`)
holds that down, `scripts/check-no-branding.sh` from the other side.

**No images, none at all.** `link_base` can be a LAN address, and blocking images is the
default in many places. The wordmark header is text.

### 4.2 Why `.html` and not `.vue`

1. Modrinth's own mail shell carries the colors as **hand-entered hex values**
   (`templates/shared/StyledTemplate.vue`), not as a reference to `variables.scss`. Copy the
   `.vue` and you copy the same handwork and still have no coupling to the interface.
2. The Vue route would bring Node into the Rust build (`@vue-email/render` and
   `@vue-email/components`, both 0.0.x), a generated checked-in artifact and `pnpm` between
   "I change a line" and "I see it".
3. A mail cannot use anything from our interface anyway: no flexbox, no CSS variables, no
   external stylesheet, no web font.
4. This way everything is checkable with `cargo test` — without Node, without a network.

The coupling comes from a test instead, and that is stricter than a copy.

### 4.3 The palette, and the yardstick from outside

`the_palette_is_the_one_the_interface_uses` reads `vendor/modrinth/assets/styles/variables.scss`
through `include_str!` — **the same file the interface loads** (`web/src/styles/global.scss`) —
resolves `var(--…)` chains and compares every token with our hex value:

| Token | light | dark |
|---|---|---|
| `--color-bg` | `#ebebeb` | `#16181c` |
| `--color-raised-bg` | `#f8f8f8` | `#27292e` |
| `--color-divider` | `#dddddd` | `#34363c` |
| `--color-contrast` | `#1a202c` | `#ffffff` |
| `--color-base` | `#2c2e31` | `#b0bac5` |
| `--color-secondary` | `#484d54` | `#96a2b0` |
| `--color-brand` | `#00af5c` | `#1bd96a` |
| `--color-link` | `#1f68c0` | `#4f9cff` |
| `--color-accent-contrast` | `#ffffff` | `#000000` |
| `--radius-md` | 0.75rem = 12 px | ditto |

It is checked in both directions: the value *stands like that* in `variables.scss`, **and** it
occurs in our templates (the light one on the element, the dark one in the media query).
Counter-check: turn `#00af5c` into `#00ff00` ⇒ only this test fails ("--color-brand in the
light theme"), all the others stay green — exactly the way "the same design" otherwise rots
unnoticed.

Looked at in a real Chromium on top of that (not as a test, because that would need a browser
running): each of the ten tokens comes out in both modes as exactly the value above.

### 4.4 How the shell handles twelve mail programs

* Table layout, styles on the element, 600 px with `max-width`.
* **Light is the base version**, dark only a `@media (prefers-color-scheme: dark)` addition
  with `!important`. Programs that strip media queries (Outlook.com) keep the light version —
  not a broken one.
* Preview text right after `<body>`, hidden with `mso-hide:all`, with a chain of invisible
  characters behind it, so the mailbox does not fill the line up with the beginning of the
  body text.
* The button as a table with an `<a>` inside, so Outlook's Word renderer does not lose the
  surface.
* `@media (max-width: 600px)`: padding 32 → 20 px, heading 28 → 24 px.
* Font `Inter, -apple-system, …, Arial, sans-serif` — Inter first, because the interface is
  Inter, but **without** a web font link.

A residual risk nobody here can clear away: Outlook 2016, Outlook.com and Gmail are not
checked. The three preview routes from section 0 make it possible to look, without taking a
user as a guinea pig.

### 4.5 The placeholders

Syntax `{{name}}` with double braces, because single ones occur in every CSS block. Filling
happens in **one** pass, so an inserted value is never read again — otherwise a user named
`{{footer}}` pulls the footer into their own mail text. For the HTML every value is escaped
(`& < > " '`), for the text version not: an `&` in a name is an `&` in the text part.

Three rules as tests:

* after filling, neither the HTML nor the text contains the sequence `{{`;
* the placeholder sets of `x.html` and `x.txt` are **equal**; otherwise a link could stand in
  the HTML version and be missing in the text version (counter-check: rename a placeholder
  only in the `.txt` ⇒ "reset_password: the two templates disagree");
* every URL in an `href` begins with `https://` or `http://`.

### 4.6 The text version is wrapped after filling

The lines in the `.txt` templates are only for whoever edits them. What gets wrapped is the
**finished** version, at 78 columns, because a hand-set line only holds as long as the value
is no longer than its placeholder: `{{when}}` is eight characters, the timestamp for it
twenty-three, and a 78-column line in `password_changed.txt` became 94, with "session was
signed out." trailing behind. The footer carries `link_base` and was 110 to 114 columns long
in every mail.

What that means: the **blank line** between two paragraphs is the only break that survives;
two links under each other in one paragraph otherwise end up on one line. A word is never
split; a link is a word and gets a line of its own, even when it grows longer.

Two tests, each with a counter-check (take `wrap` out of `assemble` ⇒ both fail):
`no_line_of_a_text_part_is_wider_than_the_window` (78 stands there by hand, not as
`TEXT_WIDTH`) and `a_value_longer_than_its_placeholder_does_not_break_the_lines_around_it`.

### 4.7 The font-size floor: 12 px

`--font-size-xs` is the smallest step the interface uses, and
`scripts/mobile-check.py` counts everything below it as a fault on a phone. A mail is read on
the same phone, and its smallest line is exactly the one a reader has to type out when the
button does not work. The footer and "copy and paste this address" were at 11 px and are now
at 12. Guarded by `no_mail_sets_type_smaller_than_the_interfaces_smallest_step`, which reads
the yardstick from `vendor/modrinth/assets/styles/defaults.scss` — from outside, like the
palette.

---

## 5. The queue

**Everything is queued except the test mail.** A sign-up must not hang on Resend's runtime
(5 s to connect, 20 s to answer), and a lost reset mail locks somebody out; a queue in memory
loses it at a restart, the row in the database survives one. The exception has a reason: with
the test mail the foreign answer **is** the result; queued, the button would say "saved" and a
wrong key would stay unnoticed.

The worker (`mail/queue.rs`, started in `main.rs` with `mail.start()`):

* woken on enqueue (`Notify`), plus every 30 s for retries that have come due;
* at most **two sends per second**, only ever one request, order `created_at`;
* `queued → sending → sent | queued(+retry) | failed`;
* `Idempotency-Key` = **the row ULID**. At startup, rows left hanging in `sending` are reset to
  `queued`: the process died in the middle of sending and does not know whether Resend
  already has the mail. Within 24 h Resend answers with the same id and sends **no** second
  mail;
* retries only for something temporary: 30 s, 2 min, 8 min, 30 min, 2 h (window ≈ 2 h 40),
  then `failed`. Everything permanent (wrong key, unverified domain, rejected content) becomes
  `failed` **immediately**: twenty attempts with the same wrong key help nobody;
* `daily_quota_exceeded` is the special case: the counter does not go up, `next_attempt_at`
  moves to the start of the next UTC day instead;
* on success `html` and `text` are set to `NULL` (the body carries the link in clear text,
  while the token lies only as a hash in its own table);
* rows older than 30 days fall daily (`mail::spawn_purge`).

Three rate limits, all counted with `SELECT count(*) … WHERE created_at > ?`, so across
restarts:

| Rate limit | Threshold | Answer |
|---|---|---|
| per address and kind | 5 in 60 min | `429 mail_rate_limited` |
| test mails | 10 in 60 min | `429 mail_rate_limited` |
| panel-wide | `daily_limit` (default 100) in 24 h | `429 mail_quota_reached` |

The panel-wide counter is **the same** count the surface shows as `sent_today` (everything the
panel promised to send in 24 h, without the finally failed ones): a rate limit that counts
differently from the number on display is a rate limit nobody understands.

---

## 6. The seam for the two areas that stand on it

```rust
mail.configured().await -> bool                       // may sign-up be offered?
mail.send(Message::VerifyEmail { to, username, token, valid_for }).await -> Result<Id, MailError>
mail.notify(Message::PasswordChanged { to, username, when }).await   // never an error upward
```

* `Message` is a named template with its values — eight variants, each with exactly the fields
  of its mail. Add a mail and the compiler leads you to every place that has to decide
  something.
* `Recipient::account(user_id, address)` or the `Recipient::stranger` substitute
  `Recipient::address(address)`. With `user_id`, deleting the account takes its unsent post
  with it (`ON DELETE CASCADE`, checked).
* **This area builds the link.** The caller gives the token, not the URL. `verify_email`
  becomes `<link_base>/verify-email#<token>` (in the fragment, so the token reaches no
  server), `reset_password` becomes `<link_base>/reset-password#<token>` (in the fragment too, 21.5). If `link_base` is missing, the answer is
  `409 mail_no_link_base` — for the four mails with a link; the four without go on.
* on success `send` returns the ULID of the outbox row. The error is a `MailError`, and
  `impl From<MailError> for Failure` means: in a handler `mail.send(…).await?` is enough, and
  the user sees the code from 1.7 along with its sentence.
* Both **log every refusal** (`tracing::warn`, with kind, recipient and code). "Not set up" is
  silent to the outside, but not in the log.

For sign-up that means: `if !mail.configured().await { /* hide the form */ }`, and ask
`mail.send(...)` **before** creating the application, if the application would be pointless
without a mail.

---

## 7. What is explicitly not built

* **No bounce handling.** What is visible is "accepted by Resend", not "arrived in the
  mailbox". Webhooks would need a public, unauthenticated endpoint with signature checking,
  `GET /emails/{id}` would need `full_access`. The surface labels it exactly that way.
* **No domain administration** (1.1).
* **No second provider, no SMTP.** The client sits behind `Outgoing`; a second provider is a
  module later and not a rebuild.
* **No attachments, no scheduled sending, no bulk sending.**
* **No second language.** The mails are English like the interface (`web/src/i18n.ts`). German
  would be sixteen more files and a "language" field per user.

---

## 8. What the operator has to decide

1. **Domain.** Until a domain is verified at resend.com/domains, only the test mail to your
   own account address goes out. Recommendation: a subdomain like `mail.your-domain.tld`, so
   the sending reputation stays separate.
2. **Sender name and reply address.** Default `craftpanel <onboarding@resend.dev>`, no reply
   address. Should anybody be able to reply?
3. **Daily limit.** Default 100 (= the free plan). 0 switches our own rate limit off.
4. **Retention.** 30 days in the outbox; the audit log keeps 180.
