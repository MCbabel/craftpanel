# Sign-up, verification, approval

As of 2026-08-14. Design and reasoning for section 20 of the contract: **a stranger comes to the
site, creates an account and is afterwards either in or visible in a queue — without the operator
doing anything by hand except approving.**

Every claim here has evidence in the existing code (file and function) or in a standard. Where a
decision goes against the obvious solution, the price is stated with it.

---

## 1. The load-bearing decision: a table of its own

An open application is **not a row in `users`**. It is a row in `registrations`, and the `users`
row only comes into being when the account becomes usable.

That is not a matter of taste. Three queries in the existing code decide it:

| Place | what would happen with a half-row in `users` |
|---|---|
| `users::reconcile` (`auth/users.rs`) | looks for `system_state = 'provisioning'` on **every** panel start and creates a system user for every row. On the next restart every throwaway application would get exactly the system account it must not get. |
| `users::search` (3.5) | is the way invitations reach other people's servers. An account without a system user would be invitable. |
| `users::page`, `promised`, `auth/disk.rs` | would count half-accounts in. `HostCapacity.allocated` would claim disk that was given away, and 12.1 says literally "what the admin has given away". |

With a table of its own **none** of these queries changes, and `api/admin.rs` did not have to be
touched for sign-up itself.

**The price** is one namespace across two tables. It is paid in 4.

The second reason is the helper: `craftpanel-helper` calls `useradd`, writes `/etc/passwd`,
`/etc/shadow`, `/etc/group` and creates a directory. Every throwaway application would be a
permanent entry in `/etc/passwd`, a UID out of a finite supply and a directory; cleaning up would
need `userdel`. A row in `registrations`, by contrast, costs nothing and is cleared away with one
`DELETE`.

## 2. Never an admin, three times over

1. `registrations` has **no role column**. There is nothing there that could carry a role.
2. Admission sets `role: PanelRole::User` as a literal
   (`registration/mod.rs`, `admit`).
3. A test sends `panel_role: "admin"` (and `limits`) in the body of 20.2 and reads the role off
   the finished account: `a_body_that_asks_for_admin_gets_a_plain_user`. serde drops unknown
   fields silently, so this test fails exactly when somebody ever takes the role from the body.

The sentence in `auth/cli.rs` ("There is no registration; this is the way in") and the reasoning
for `is_last_admin` have been corrected accordingly: the CLI remains the way to the **first**
admin.

## 2a. The second load-bearing decision: an unverified application holds nothing

A verification nobody has answered proves nothing about **who reads that mailbox**, so it must
not decide whose account the address becomes. If somebody applies with an address that already
carries an unverified application, that application is therefore replaced **entirely**: name,
password and token together, in one statement (`registration/store.rs::take_over`).

The attack this closes, in the order an attacker runs it: he applies with an address that is not
his and waits; the owner of the mailbox applies himself; the owner clicks the link that then
arrives. Until now that click made the **attacker's** account — the second form found the first
row, dropped the owner's name and password without a word and sent a fresh link for the
stranger's application (`registration/tests.rs:491-498`).

The trade this costs is the smaller one and it is named: a stranger can overwrite an application
that is still unverified and make the link in the applicant's mailbox worthless. That costs one
more trip to the form, goes no faster than 20.11 allows, and ends as soon as the account exists.

Three conditions carry the takeover:

* **`state = 'email_unverified'`.** A *verified* address was verified by whoever reads the
  mailbox. A form somebody else fills in must not reset him to unverified, nor take his place in
  the queue, nor turn the admin's yes into a stranger's account
  (`registration/tests.rs:578-581`).
* **`email = ?` on the normal form from 4.1.** That way `MAX@…` finds the same row too; keyed on
  the letters as written, the stranger's application would sit next to it
  (`registration/tests.rs:509-511`). And a caller cannot hand in an ID and an address that do not
  belong together.
* **`tokens_sent` starts at zero**, `created_at` is that of the new application. Otherwise five
  links used up by the predecessor would leave the new applicant with a row that can never send
  him one (7): a stranger would make an address unusable for everybody by requesting his own
  link five times (`registration/tests.rs:728-731`). And the seven days from 8 belong to the open
  application, not to the one it replaced.

**What follows from this for every step *after* looking up a token:** it has to hang on the
token, not on the ID. Because between the lookup and the write the row may have changed owner. It
affects three places, and each has been given a query of its own: the hash that admission writes
into the account (`password_hash_for_token`), the verification that puts a row into the queue
(`mark_verified`), and the name the resend greets its mail with (`replace_token` returns it
**after** the write). If one of them asks by ID, an account comes into being whose name is from
one application and whose password is from the other (`registration/tests.rs:660-666`).

**And an exception in `claim_name_for_sign_up` belongs with it:** whoever has lost his mail fills
in the same form again, and his **own** open application must not answer him `username_taken`;
otherwise all he has left is the resend from 20.4, and that is exactly the door the takeover
closes. The price, likewise named: `202` instead of `409` tells somebody who already knows a name
that has not been given out yet that this name sits on the address he has just guessed. Asking
destroys the application that was asked about — so the question is anything but quiet — and what
6 is really about stays untouched: a guess at an **address** is answered with `202` either way
(`registration/tests.rs:622-625`).

## 3. The path, state by state

```
(nothing) --POST /auth/register--> email_unverified --POST /auth/verify-email-->
    approval off -> users row + system user -> done
    approval on  -> awaiting_approval --admin--> approve -> users row -> done
                                             --> reject -> row gone + 30-day block
```

What sign-in makes of this is in 20.8. Two sentences about it that are easily lost:

* **The order is the contract.** The password first, then the state. Only somebody who knows the
  password gets `403 email_unverified`; otherwise the two codes would be a directory of everybody
  who has applied here. If `users::by_name` finds nothing, sign-in checks the real hash from
  `registrations`: that costs exactly one argon2, as before, and only when nothing is there
  either does `verify_against_nobody()` run.
* **No session for half-accounts**, and none after the verification click either. A `Caller` is a
  full account everywhere in the panel; a second, weaker kind of session would have to be checked
  at every one of the 138 method/path pairs, and one forgotten guard would be a user with a
  server and no approval. `must_change_password` is expressly **not** borrowed for this.

## 4. Uniqueness across two tables

`users::claim_name` asks both tables, `claim_name_in_users` only `users`. The difference is not
cosmetic:

* **12.3 and 12.5** ask both. A name an applicant is holding while he reads his mail must not be
  taken from him by hand.
* **Admission** (20.13) asks only `users`. It must not ask both, because the row whose name is
  being given out stands in `registrations` itself — every admission would fail against itself.
  That happened once while building and is the reason there are two functions.

The narrow race remains: between "reserved" and "in `users`" an admin can create the same name by
hand. It ends **loudly** with `409 username_taken` at admission, not quietly: the application
row stays, and the interface says that it has to be started over.

Addresses: `registrations.email UNIQUE` and `CREATE UNIQUE INDEX users_email`. Several `NULL`s
stay allowed, because accounts created by hand need no address. `users::map_taken` reads the
message of the violated index and turns it into `username_taken` or `email_taken` instead of
`500 internal`.

### 4.1 The normal form (20.10)

Trimmed, lower-cased throughout, exactly one `@`, local part 1–64, domain with a dot, no control
characters, whole address ≤ 254 (RFC 5321 §4.5.3.1).

Two of these points have a reason that is not obvious. **No control characters** is the door in
front of the door: a `\r\n` in an address is the way a foreign `Bcc:` gets into our mail. The mail
layer cleans header values anyway (19.3), but here it is rejected first
(`registration/address.rs:106-107`). And **a domain without a dot** is a local host name; mail to
it never leaves the machine, so an account behind it could never be verified
(`registration/address.rs:45-46`).

Otherwise the check is thin on purpose: the real check of an address is whether a mail arrives,
and Resend gives the answer to that (19.11). A stricter pattern would reject addresses that work
— `postmaster@[192.0.2.1]` is legal, and plus, dot and hyphen are legal almost everywhere. What
this place really has to get right is the **comparison**, because it is the comparison that keeps
one person from having two accounts (`registration/address.rs:1-9`).

The local part is folded too, although RFC 5321 distinguishes it: every provider folds it, and
`Max@` and `max@` as two accounts would be a door to duplicate accounts.

**No provider-specific normalization.** `max+1@gmail.com` and `max+2@gmail.com` stay two
addresses. For Gmail they are one mailbox, for many others they are not, and a rule that throws
other people's addresses together turns real people away. The test is called
`a_plus_tag_stays_its_own_address`, so that nobody "fixes" it later. The consequence is named and
is caught elsewhere: approval on (the default), the daily volume of mail, the manual block.

## 5. Brakes, with numbers

All of it in `auth/rates.rs`, one bucket per key, the shape of `auth/brake.rs`, but by "how
often is something allowed at all" instead of by failed attempts. `brake.rs` stays untouched and
stays **separate**: shared counters would mean that a storm of resets behind one NAT locks the
*sign-in* of everybody behind it.

| What | Limit | Why this number |
|---|---|---|
| 20.2 per sender IP | 3 in 60 min, 10 a day | a household signs up once, behind NAT three times as well; ten a day caps a machine well below Resend's free hundred |
| 20.4 per address | 1 in 5 min | without it the endpoint is a mail bomb on somebody else's mailbox; five minutes reads to a person as "coming shortly" |
| tokens per application | 5, counted in the row | whoever cannot find five links needs a different address — and this limit survives a restart, unlike the buckets |
| 20.3 per IP | 30 in 60 min, **failures only** | a person clicks once; only failures count, so that a second click or a mail scanner uses nothing up |
| panel-wide per day | `daily_limit` from 19.2 | expressly **no** second counter: the one from 19.10 stands in a table and survives a restart |

The first verification mail counts against the five minutes of 20.4. Without that one line,
"sign up" and then "send again" put two mails into somebody else's mailbox within seconds,
exactly what the brake stands against. The test for it is
`a_second_mail_to_one_address_inside_five_minutes_is_not_sent`, and it found the fault while
building.

The buckets live in memory, at the price `brake.rs` admits to: a restart forgets a count. They
are handed into the service as an `Arc` and not reached for globally; otherwise two tests share
one address and use up each other's quota, which made 22 tests fail at once while building.

## 6. No disclosure, three times over

1. **Wording and length.** For every input 20.2 and 20.4 answer `202` with the same body. That is
   checked byte by byte, headers included:
   `new_known_and_blocked_addresses_answer_identically`.
2. **Cost.** argon2 runs in **every** case, with a known address and with a block too. Checked
   with the counter `password::argon2_runs()`, which now counts hashing as well (before, only
   verifying); `a_known_and_an_unknown_address_cost_the_same_argon2`.
3. **No name field.** 20.4 takes an address and nothing else. "Username or e-mail" would be a
   name oracle for strangers.

The username, by contrast, is **not** a secret: 3.5 gives names out to anybody signed in, and a
form that may not check the name is unusable. So an honest `409 username_taken`.

With a known address, `address_already_registered` goes to the **existing** address: the owner
learns that somebody has used his address, and gets the way to sign in and to reset. That is
OWASP's recommendation. If the address belongs to an application that is still unverified, it
gets a fresh verification link instead: whoever fills in the form a second time has lost the
mail, and a dead end would be a person who never gets finished.

## 7. The token (20.9)

256 random bits in base64url (43 characters), stored **only** as SHA-256 with a `UNIQUE` index —
the same pattern as `sessions.token_hash`. The three lines for it have been moved out of
`auth/session.rs` into `auth/secret.rs`, together with the RFC 4648 vectors: three copies of
base64url would be three chances to grab the padded alphabet.

On the project rule about secrets: playit's file-with-0600 form is right for an **outgoing**
long-lived key that the panel reads in order to call a foreign service with it. A verification
token has to be found **incoming**, over its own value; a lookup over a directory tree would be a
hand-written table scan. It belongs in the table, and there only as an imprint — which gives it
the same property that makes the file valuable: a copy of `panel.db` carries no access.

Valid for **24 hours**. A verification grants no access, it only shows "this address exists", and
people read mail in the evening. For exactly that reason the reset link has a much shorter
deadline (21.5).

In the link it sits in the **fragment**: `<link_base>/verify-email#<token>`. A fragment reaches no
server, so it lands in no access log (`main.rs` hangs `TraceLayer` over everything) and in no
`Referer` — this panel loads Inter from Modrinth's CDN. The page accepts `?token=` as well and
clears it out of the address bar at once. It is redeemed with a `POST` from the page, so it
stands in no server URL (1.2).

**Why a second click usually answers anyway.** If the application is waiting for approval, its
row is alive, and the token stays attached to it, so that the second click gets the same `200`.
A mail scanner does not burn it: the link is a `GET` on a page of our interface, redemption is a
`POST`. After an admission without approval the row is gone, and the answer is `404` with the
sentence "if you have already verified, sign in". **The reset link must not copy this leniency**
(21.5), and does not, see `docs/PASSWORD-RESET.md` 4.

## 8. Cleaning up (20.12)

One task in the panel process, every six hours and once at startup, in the pattern of
`audit::spawn_purge`:

* `email_unverified` older than **7 days**: gone. The link is dead after 24 hours, seven days
  leave room for "at the weekend", after that name and address are free again.
* `awaiting_approval` older than **30 days**: gone. Whoever does not look for 30 days is not
  looking any more, and the operator is allowed to be on holiday.
* `registration_blocks` with `until <= now`: gone. `until IS NULL` stays; that is the operator's
  manual block.

**No block list for throwaway providers.** Such lists are out of date on the day they ship and
they hit real people. The approval switch is the answer, plus the manual block.

## 9. What is still open

* **Approval is the default.** Without it the daily volume of mail alone carries the weight of
  the plus tags (4.1). If you want "straight in", you switch it off and get a warning on the
  settings page for it.
* **Self-signed-up users get `default_limits`** like every other account — today 50 GiB of
  promised disk per account. A second set of defaults just for self-signed-up users is doable,
  but it would be the operator's decision, not the build's.
* **`signup_ip`** is stored, shown in the queue and deleted at admission. Without it the brake
  keeps working, the triage does not.
