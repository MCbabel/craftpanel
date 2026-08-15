# Security

## Reporting

Use GitHub's private form: **Security → [Report a vulnerability](https://github.com/MCbabel/craftpanel/security/advisories/new)**.
It opens an advisory that only you and the maintainers can read, with a private fork to fix it in.
Not a public issue, not a pull request, not a comment on one: the helper runs as root, so anything
that touches it wants a quiet fix first.

If the form is not available to you, open an ordinary issue that says you have something for the
security tracker and nothing else — no version, no steps — and you will be invited to the advisory.

## What belongs in the report

- **The version.** `craftpanel --version`, or the commit you built from.
- **What it takes to do it.** No account at all, any signed-in user, a user with a role on somebody
  else's server (which role, which permission bits), or an administrator. A finding that needs an
  administrator is a different thing from one that needs nobody.
- **What it gets.** Code as root, code as another user, another user's files or console, a session,
  one of the stored keys, a machine that stops answering.
- **The steps.** The request with its headers and body beats a paragraph describing it. Say whether
  the privileged helper is involved, and whether the panel or a game server was running at the time.
- **The log, if it helped.** `journalctl -u craftpanel` and `journalctl -u craftpanel-helper`. Read
  what you paste: log lines carry file names, e-mail addresses and public addresses.

## Where it hurts most

Roughly in order. A report against any of these gets looked at first.

1. **The privileged helper** — `crates/craftpanel-helper/`. It is the only part that runs as root.
   Its whole vocabulary is five commands: create an account, delete an account, set limits, hand
   back a subtree, start a process. It executes exactly one binary, the supervisor; the Java command
   line reaches it as data and is run unprivileged. It refuses a uid below 1000 and works under
   `/var/lib/craftpanel/users` and nowhere else. Its socket is `0660` and belongs to the group
   `craftpanel`, so every process in that group can speak to it. Anything that widens the
   vocabulary, gets it to execute something else, gets it to write or chown outside that root, or
   reaches the socket from outside the group, is the most serious class this project has.

2. **Path resolution** — `crates/craftpanel/src/files/jail.rs` and
   `crates/craftpanel-helper/src/beneath.rs`. Every file operation is resolved with `openat2` under
   `RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS`, plus `RESOLVE_NO_SYMLINKS` on the way in; on a kernel
   without `openat2` the fallback is one `openat` per segment, which is the weaker of the two paths.
   Escapes out of a server directory, symlink races and TOCTOU windows count. So does unpacking:
   `crates/craftpanel/src/files/archive.rs` takes entry names straight from an uploaded zip.

3. **Rights per account** — `crates/craftpanel/src/auth/` and `model.rs`. Two panel roles (admin,
   user), three server roles (owner, editor, viewer) over ten permission bits. Anything that lets
   one user read or touch another user's server, files, backups, console or audit log; anything
   where a viewer does what only an owner may; anything a sign-up that is unverified or still
   waiting for approval can already reach.

4. **Secrets and sessions.** The Resend key, the playit key of a user and the Google refresh token
   of a user live as files with mode `0600` in directories with mode `0700` under the data
   directory — never in the database, and not meant to turn up in an API answer or in the log.
   Passwords are argon2id; failed sign-ins are braked per account and per address. The session
   cookie is `HttpOnly` and `SameSite=Lax`, and carries `Secure` only when the request arrived over
   TLS, which is read off `X-Forwarded-Proto` from whatever proxy sits in front. Leaks, forgeries
   and sessions that outlive what ended them belong here.

5. **The installer** — `install.sh`. It is run as root out of a pipe from `curl`. It writes systemd
   units, and its update path moves old units aside and chowns whole trees. Anything that gets it to
   write, move or chown somewhere it was not meant to is in scope.

## What is not a finding

- **Plain HTTP.** The panel binds `127.0.0.1:8080` and speaks HTTP. TLS is the job of the proxy in
  front of it. Exposing the port straight to the internet is a deployment decision, not a hole here.
- **What a user's own server does.** Mods and plugins somebody installs run with that user's rights,
  deliberately. The boundaries the panel keeps are between panel users, and between them and root —
  not between a user and the server they started themselves.
- **An attacker who is already root**, or already in the `craftpanel` group. Both are inside every
  boundary there is.
- **The loaders that are not wired up.** NeoForge, Quilt and Forge answer `502` and say so. That
  they appear in the list is a gap, and it is written down in the README.
- **A scanner's output with no path through the panel.** A dependency with a version number in an
  advisory database, and no call from here that reaches the affected code, is a pull request that
  bumps `Cargo.lock` or `pnpm-lock.yaml` — not an advisory.

## What happens then

One person maintains this. There is no rotation and nobody is paid to be on call, so the honest
shape of it is: a first answer normally within a week. If two weeks pass with no answer at all, open
a public issue saying you are waiting on a private report — the fact that you are waiting, not what
you found.

The fix is written in the private fork of the advisory, and the advisory is published together with
it, with credit under whatever name you give, or none if you would rather. There is no bounty and
nothing to send you.

## Which versions get a fix

There is no release yet. `main` is what is fixed, and building from it is the only way to have the
fix. Once there are releases, the newest one gets it; there is no older line to backport onto and
none is planned.
