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
   write, move or chown somewhere it was not meant to is in scope. It is published as an asset of
   each release, next to the bundles and signed with them, so what reaches a machine is a tagged,
   tested and attested file rather than the current tip of the default branch.

## Installing it on purpose

The advertised way in is `curl … | sudo bash`, which is a stranger's machine running this as root
on the strength of one address. What that is worth, and what a careful person can do instead:

- **Read the script first.** `curl -fsSL -o install.sh …`, read it, then `sudo bash install.sh`.
  The last line of the file is `main "$@"` and everything above it only defines functions and sets
  variables, so a download that breaks off halfway installs nothing at all rather than half of
  something.
- **The address in the README is a release asset**,
  `https://github.com/MCbabel/craftpanel/releases/latest/download/install.sh`, which redirects to
  the installer attached to the newest release. It went through `ci.yml` on the tagged commit, it
  was uploaded by the run that published the release, and it carries the same attestation the
  bundles carry. It used to end in `HEAD` — the tip of the default branch at the moment you fetched
  it — and that was the one thing in this project a push could put on a stranger's machine as root
  with no tag, no test and no signature in between, while everything else went through the release.
  The cost of the change is named rather than hidden: a fix to the installer now needs a release,
  because there is no longer a channel that reaches people faster than one.
- **To pin the script, name the release** —
  `…/releases/download/v1.2.3/install.sh`. A stricter address still exists and always will: a
  40-character commit id under `raw.githubusercontent.com` serves those exact bytes or nothing,
  because the name is a hash of the content. A release asset is not that; whoever holds this
  repository can delete one and upload another under the same URL, which is equally true of the
  bundles and is what the attestation, not the address, is there for.
- **`CRAFTPANEL_VERSION=1.2.3`** installs exactly that release rather than whatever
  `releases/latest` names today, which is how you install a version you have already looked at.
  Note that it pins the bundle and not the script: the installer asks GitHub for the newest release
  whichever release the installer itself came out of, so going back to a version before a bad one
  takes both halves — the older script's address *and* the variable.
  **`CRAFTPANEL_REPO=owner/name`** decides which repository is downloaded from; unset it is
  `MCbabel/craftpanel`, and it is worth knowing the variable exists if only to see that it is not
  set behind your back.
- **A bundle with no published checksum is not installed.** The installer used to warn and carry
  on, so a 404 or a timeout on the small `.sha256` file alone was enough to run unchecked bytes as
  root. It stops now. `CRAFTPANEL_NO_CHECKSUM=yes` is the one way past it, it has to be typed, and
  it does not apply to a sum that fails to match — that always ends the run.
- **The sum that was checked is printed**, `checksum verified: sha256 <the number>`, so there is
  something to hold against the release page. Words like "checksum verified" over a number nobody
  is shown are not worth much.

**What the checksum does not prove.** It is published beside the bundle, by the same job, and it
travels over the same URL. Whoever could change one could change the other. It catches an upload
that broke off and a download that went wrong — not a bundle that was swapped for another. It is
said here plainly rather than left to be inferred from the word "verified".

`install.sh.sha256` is published the same way and is worth exactly the same, with one difference
that cuts the other way: nothing checks it for you. The installer checks the bundle; the installer
*is* the script, so it is in no position to check itself. That sum is there for a person holding
the file against the release notes before running it, and for nobody else.

**What proves where a file came from.** Every bundle in a release, and `install.sh` beside them,
carries a build provenance attestation, made in the run that published it. The signature is
[Sigstore](https://www.sigstore.dev)'s, with a certificate that lives for minutes and whose subject
GitHub's OIDC provider fills in rather than the workflow: this repository,
`.github/workflows/release.yml`, the commit, the run. The signed statement is stored under the
repository, looked up by the digest of the file, and written to the public transparency log — not
laid down beside the asset, which is the whole difference from the checksum. Checking it wants the
[GitHub CLI](https://cli.github.com), signed in to some GitHub account:

```bash
gh attestation verify craftpanel-linux-x86_64.tar.gz \
  --repo MCbabel/craftpanel \
  --signer-workflow MCbabel/craftpanel/.github/workflows/release.yml
```

Without `--signer-workflow` the check asks only that some workflow in this repository signed the
file; with it, that this one did. `--deny-self-hosted-runners` may be added — releases are built on
GitHub's own runners and nowhere else. The lookup is by digest, so it works on a file downloaded
months ago just as well as on one fetched a minute ago, and `gh attestation download` fetches the
bundle for a later `--bundle` check on a machine with no network. Put `install.sh` where the
tarball's name is and the same command answers the same question about the script — though not
before you run it, unless you were going to fetch it, check it and run it as three separate acts,
which is the point of the whole section above.

**What the attestation still does not prove.** That the source it was built from is any good: a
signature says who built a thing, never that the thing is sound. That the run was not led astray
from inside — the workflow uses actions written by other people, and one of those turning malicious
would hand back a tampered bundle with a perfectly good attestation on it. That whoever holds this
repository is honest: the signer is a workflow in this same repository, so somebody with push
rights over `.github/workflows/` signs as readily as we do. What they cannot do is sign quietly,
because the certificate names the commit and the run it was made from and the transparency log
keeps the record. And it proves nothing at all to somebody who never checks: `install.sh` does not,
because a fresh machine has no GitHub CLI on it and no account signed in. This is a check for the
careful, made by hand.

**What none of it does for the first fetch.** Moving `install.sh` into the release closed a real
hole — a push to the default branch no longer reaches a stranger's machine as root — but it did not
close the chain, and it would be dishonest to let the word "attested" suggest otherwise. The very
first `curl` rests on two things no signature reaches. TLS: some certificate authority somewhere
can issue for `github.com`, and a machine that trusts a bad root trusts everything served over it,
including the release page, including the sums on it. And GitHub itself: the assets, the API that
says which release is newest, the attestation store and the transparency log are all one company's
infrastructure, reached from a machine that has nothing of ours on it yet to compare against. There
is no key shipped ahead of time here, no fingerprint published anywhere but here, and so no way for
the first fetch to be anything but a trust in GitHub and in the certificate chain of the day. Every
check described above is a check made *afterwards*, by somebody who already has the file — it
narrows who could have tampered and it leaves a public record of it, which is worth having, and it
is not the same thing as not having to trust anyone. Whoever wants less than that trusts a bundle
built from source they have read (`scripts/release.sh`, `CRAFTPANEL_BUNDLE=`), which moves the
question to the git history and the toolchain rather than removing it.

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
