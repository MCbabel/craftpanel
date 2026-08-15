# Forgotten password

As of 2026-08-14. Design and reasoning for section 21 of the contract. **Whoever has forgotten his
password fetches a new one himself over his address, and the operator who has no mail key yet
reaches the same place with one command in the terminal.**

---

## 1. The path

Four steps, three public endpoints, one nudge from an admin.

1. **Enter the address.** `POST /auth/password-reset` → **always** `202` with no body. The call
   does exactly four things: read the body, ask the brake, count the brake up, hand off the job.
2. **Mail with a link.** The detached task does the work: normalize the address, look for the
   account, check the cool-down, delete old tokens, mint a new one, write the row, queue the mail.
   If it finds no account, it does nothing.
3. **Set a new password.** `POST /auth/password-reset/confirm` → `204`, **without `Set-Cookie`**.
4. **Everything old falls.** All the account's sessions, all remaining tokens,
   `must_change_password = 0`, and `password_changed` goes out if an address is on file.

Alongside that, `POST /auth/password-reset/verify`, which gives the **name** behind a token
without using it up: whoever has the token got the mail, and a form without that line is an
imposition.

**No new session** after setting it (OWASP: "Don't automatically log the user in"). That costs one
sign-in step and has a second use: a missing approval (20.8) cannot be got around this way,
because the way back leads through the sign-in.

## 2. Where the token lies, and why not in a file

256 random bits in base64url, in the table **only** as SHA-256 with a `UNIQUE` index, the form of
`sessions.token_hash`. No argon2: 256 real random bits are not a password, they cannot be guessed.

On the project rule: playit puts its key down as a file with 0600, because it is used
**outgoing**: the panel reads it and calls a foreign service with it. A reset token has to be
found **incoming**, over its own value; a lookup over a directory tree of files would be a
hand-written table scan. It belongs in the database, and there only as an imprint. That gives it
the same property that makes the file valuable: a copy of `panel.db` — a bug report, a backup —
contains no access.

`auth/secret.rs` holds `fresh`, `digest` and `base64url` for sessions, verifications and reset
tokens together. Before, that stood in `session.rs`; three copies of base64url would be three
chances to grab the padded alphabet.

## 3. The numbers

| Property | Setting | Reason |
|---|---|---|
| Validity | **30 minutes** | ASVS 5.0 6.5.5 demands at most ten minutes for out-of-band requests, but means push and OTP by that; a mail link has to survive delivery, the spam folder and a phone in a pocket. In-house precedent for a deadline: `playit/mod.rs`, `CLAIM_DEADLINE = 15 min` |
| One-time | yes, over `used_at` | ASVS 6.5.5 — and **unlike the verification token** (20.9), which answers more than once inside its deadline. That one only shows an address, this one opens an account |
| Number | one open per account; a new request deletes the older ones | keeps the window small and makes "usable once" unambiguous |
| Cool-down (21.1) | one mail per 60 s, five per hour per account, dropped silently. **The hourly half does not fire in the existing code** (contract 17.15, measured) | stops a stranger from filling a mailbox with our mail and using up the daily volume while doing it. Counted over the `created_at` of the rows, so it **survives a restart** — only `mint` deletes these rows itself before the number can reach five |
| Cool-down (21.4) | no waiting time, but five links per hour per account → `429 too_many_attempts`, and **checked before the old token falls** | an admin on the phone should not have to wait 60 s; but unlimited it must not be either, because every press is a mail in somebody else's mailbox **and** throws away the account's open link. A counter of its own (`rates::ADMIN_RESET_PER_ACCOUNT`), so that an admin does not use up the user's brake. **The mail brake from 19.10 does not catch this**: it sits behind the delete and counts what the public form has already sent — when it is full, the press takes the live token and the replacement mail is refused (contract 17.15, measured) |
| Brake | ten attempts in 15 min, per address and per sender IP → `429 too_many_attempts` | the numbers and the build of the sign-in brake, but an instance of its own |
| one code for three cases | `400 invalid_reset_token` for unknown, expired, used up | three codes would be an oracle about the state of other people's tokens |
| Cleaning up | used and expired rows 24 h after their end, on every new request | the way sign-in cleans up sessions; no extra background task, no extra wire in `main.rs` |
| in the link | `<link_base>/reset-password#<token>` — in the **fragment**, like 20.9, and struck out of the address bar by the page at once with `history.replaceState` | a fragment reaches no server: `?token=` would stand, together with the request for the page, in the access log of the reverse proxy (nginx logs `$request` **and** `$http_referer`) and after that in the `Referer` of every file the page loads. `replaceState` comes too late for that: the request has already gone out. The page still reads `?token=`, so that an older link does not lead nowhere |

A password that is too short does **not** count towards the brake and does not use up the token.
Otherwise somebody locks himself out because he has typed something too short three times. The
test for it types something too short twelve times and demands that the thirteenth attempt with a
proper password still works.

### 3.1 Why the used row stays

Redemption sets `used_at` and deletes **only the remaining** tokens of the account. The first
build deleted them all, its own included, and then `used_at` was ornament: "works exactly once"
hung on the row being gone, not on it being marked as used. The counter-test found that (it did
**not** fail, although `used_at IS NULL` had been taken out of the query). The row that stays also
carries the only trace when somebody uses the form to pester a stranger (`requested_ip`,
`user_agent`), until the broom takes it 24 hours later.

## 4. No disclosure — and time is part of it

1. **Wording.** `202` with no body for every input. The interface always shows the same sentence:
   "If there is an account for this address, a mail is on its way."
2. **Headers and length.** No `Set-Cookie`, no `Retry-After`, no body. That is a test, not an
   intention: `a_known_and_an_unknown_address_answer_identically` compares status, headers and
   body.
3. **Time.** In both cases the handler *does* the same thing, instead of taking the same time. No
   `SELECT`, no `INSERT` with `fsync`, no network call in the request path — everything lies in a
   detached task, that is **after** the answer. So there is no difference to measure.

The way point 3 is checked is deliberately **clock-free** (a lesson from task #21): a gate made of
`tokio::sync::Notify` holds the work shut, and the test demands that the `202` is already there
while it is still shut. If `begin` is `await`ed in the handler instead of detached, the test runs
into its time limit and fails cleanly. Verified, not claimed.

This freedom can break quietly: if somebody later builds a check "does the address exist at all"
into the handler, in order to answer `400` earlier, no other test turns red. That is why the
reason stands as a comment directly above the handler.

## 5. Accounts with peculiarities

| State | Behavior | Reason |
|---|---|---|
| `must_change_password = 1` | allowed, and the flag falls | he has now chosen for himself; the guard does not send him to `/change-password` afterwards |
| open application (both states) | **no mail, no token**, answer `202` | there is no account that could carry a password. The way for that is 20.4, and a verification mail from here would be the worse variant, because one click would verify a stranger's account |
| account without an address | no mail | exactly the operator's case on day one; his way is 6 |
| no mail set up | `202`, but **no row** | a token nobody can learn of is a row waiting to be stolen |
| `busy = 1` | is **not** checked | `busy` protects file system and UID work (12.6); here `users.password_hash` is written and sessions are deleted. A `409 user_busy` would be an error nobody understands, and from outside it would give away that the account exists |
| `system_state = 'error'` | allowed | signing in works, creating a server does not; that has nothing to do with the password |

## 6. The two ways without mail (task #25) — and the address

```
craftpanel admin passwd     --username max [--print-password | --password-stdin]
craftpanel admin reset-link --username max [--base-url https://panel.example.com] [--minutes 30]
craftpanel admin email      --username max [--address max@example.test | --remove]
```

`passwd` sets the hash, throws away all sessions and all open tokens and follows the same rule as
the first creation: a password that comes out of the terminal (`--print-password`) wants to be
replaced; one the operator types himself does not.

`reset-link` mints a token and writes **the link** to standard output; everything human to
standard error, as with `create`, so that the link can be piped on. The basis is `link_base`
from 19.2; if it is missing, `--base-url` is required. With `http://` the command warns, because
the token then travels in the clear, but it does not refuse: on a home network `http` is the only
possible value.

`email` is the command without which the other two remain the *only* ways: until this round
nobody but an `UPDATE` in the database could give an address to an account created by hand; 12.3
and 12.5 took the field, but the interface had none and the CLI wrote `email: None`. Both accounts
on this machine were thereby shut out of the whole of section 21. It works for every account,
checks the way 12.5 does (`invalid_email`, `email_taken` against `users` and open applications)
and throws the open tokens away on a change (section 7). A subcommand of its own and not a switch
on `passwd`, because `passwd` ends every session of the account: adding an address afterwards
should not sign anybody out.

**No mail from the CLI**: there the operator is standing at the machine himself.

## 7. What else a password change invalidates (21.8)

**Every** password change discards the account's open tokens — 3.4, 12.5, 21.3 and the CLI.
Without this rule a link mailed long ago opens an account the owner has just taken back, and that
is exactly the case in which somebody changes his password *because* he suspects a break-in. The
three lines for it stand in `api/session.rs`, `api/admin.rs` and `auth/cli.rs`.

The same goes for **every change of address** (12.5 and `admin email`), from the other side: the
link lies in the old mailbox. Whoever changes the address and then redeems the link from before
takes over the account: the order is the whole trick, and that is why the token falls with the
address. Removing the address counts too. The **sessions** stay in place: an address is not a
means of signing in, and the field is not the password. Saving the same address again, written
differently too, is not a change. Otherwise a token dies on the second press of "Save".
