# Java runtimes the panel fetches itself

As of 2026-08-23. Design and build record for `crate::java`: the panel lays a missing Java runtime
down in `<data_dir>/runtimes/java-<major>/` on its own. **A machine needs no Java before CraftPanel
does.**

Every statement about Adoptium, Azul and Amazon was measured on 2026-08-23 with the call printed
beside it. Every statement about this tree carries `file:line` **and the identifier that stands
there**, because three rebuilds have already moved every line in this module once: when the number
is wrong the name is still searchable.

The operator's half — the switch, the admin page, the two buttons — is `docs/JAVA-OPERATOR.md`.
This document is the engine.

**What was there before.** `crates/craftpanel/src/settings/runtimes.rs` finds Java, it never
installs any: `candidates()` (`:119`) reads `<data_dir>/runtimes/*`, `$JAVA_HOME`, `/usr/lib/jvm`,
`/usr/java`, `/opt/java`, `/opt/jdk` and whatever `java` on `PATH` resolves to, and `read_home()`
(`:152`) reads each one's `release` file for `IMPLEMENTOR` and `JAVA_VERSION`.
`Manager::java` (`crates/craftpanel/src/servers/manager.rs:1048`) looks in
`<data_dir>/runtimes/java-<major>/bin/java` **first** and falls back to what is installed. Nothing
in the tree ever wrote into `runtimes/`. `crate::java` is that writer.

---

## 1. Temurin, and not Zulu or Corretto

The source is the Adoptium API, one call:

```
GET https://api.adoptium.net/v3/assets/latest/{major}/hotspot
    ?architecture={x64|aarch64}&image_type=jre&os=linux&vendor=eclipse
```

`adoptium::url` (`java/adoptium.rs:89`) builds exactly that. The answer is an array;
`[0].binary.package` carries `link`, `checksum` (sha256), `size` and `name`, `[0].release_name` and
`[0].version.semver` name the build. `adoptium::parse` (`:106`) reads those six, and
`[0].version.major` to pick the asset out of the array, and nothing else at all. Measured for
linux/x64, `image_type=jre`, on 2026-08-23:
`8 → jdk8u502-b07` 41 851 657 B, `17 → jdk-17.0.20.1+1` 46 640 574 B,
`21 → jdk-21.0.12.1+1` 52 059 408 B, `25 → jdk-25.0.4.1+1` 61 718 288 B. aarch64 is published as
well (`21` there: 51 149 460 B). `/v3/info/available_releases` answers `8, 11, 17, 21, 25` as its
LTS line; of those, this panel ever asks for four — 8, 17, 21 and 25 (`MAJORS`,
`java/inventory.rs:16`), which are the four `default_major()` (`settings/runtimes.rs:96`) can name.

**Java 8 is the reason.** `default_major()` asks for Java 8 for everything up to 1.16, and the
distributions are walking away from it: Debian 13, the machine this was written on, offers
`openjdk-21-jre-headless` and `openjdk-25-jre-headless` and no `openjdk-8-jre-headless` at all
(`apt-cache policy`: *Candidate: (none)*). A vendor that stops at 17 leaves those servers with
nothing, and 1.16 and older are a large part of what people put on a panel like this.

The two obvious alternatives were measured, not dismissed:

* **Azul Zulu** builds Java 8 for Linux and has an open API, but the list call
  (`api.azul.com/metadata/v1/zulu/packages/?…&latest=true`) returns `download_url` and
  `package_uuid` **and no checksum** — re-measured on 2026-08-23 for `java_version=8`,
  `java_package_type=jre`: nine fields, none of them a hash. The sha256 sits behind a second call
  per package (`/metadata/v1/zulu/packages/{uuid}` → `sha256_hash`). Two calls where one does, and
  a shape in which forgetting the second one silently means "downloaded unverified".
* **Amazon Corretto** publishes no JRE for Linux at all: `corretto.aws/downloads/latest/
  amazon-corretto-8-x64-linux-jre.tar.gz` answers **404**, only the `-jdk` variant exists (302,
  100+ MB). And `downloads/latest_checksum/…` returns a 32-character **MD5**
  (`18514d20593901cdb30b8fa5d2898e1b`), which is not a hash we want a runtime's identity to rest on.

Adoptium hands over link, sha256 and size in the same answer, for both architectures, for every
major we can ask for. That is the whole argument; it is not brand loyalty.

`JreVendor` (`model.rs:899`) knows three names — `temurin`, `corretto`, `graal` — and
`vendor_of()` (`settings/runtimes.rs:195`) reads a plain OpenJDK `release` file as Temurin,
because Temurin *is* Eclipse's build of exactly that and it is the least wrong of the three. What
we fetch is a Temurin in the strict sense, so the list stays honest.

## 2. A JRE, not a JDK

A server runs, it does not compile. Measured on the same API, x64, 2026-08-23:

| major | `image_type=jre` | `image_type=jdk` |
|---|---|---|
| 8 | 41.9 MB | 103.5 MB |
| 21 | 52.1 MB | 207.5 MB |

Four times the bytes and four times the disk for tools no part of this panel calls: `jcmd`,
`jstack`, `jmap`, `javac` appear nowhere in `crates/`. What it costs is exactly those tools — an
operator who wants a heap dump analysed on the machine installs a JDK himself, and the panel then
finds it like any other system runtime (`Source::System`). `-XX:+HeapDumpOnOutOfMemoryError` is a
VM flag and works on a JRE.

## 3. Fetched once, and never behind the operator's back

`Runtimes::lay_out` (`java/mod.rs:128`) short-circuits on `present(major)` (`:85`): if
`<data_dir>/runtimes/java-<major>` already reads as a Java runtime, `install()` returns it with
`fresh: false` and touches no network. **Nothing in this panel asks Adoptium about a runtime that
is standing there unless a person pressed a button.** There is no timer, no "check for updates",
no version file that could go stale. Every automatic fetch goes through one function,
`java::report::lay_out` (`java/report.rs:11`), and that one has exactly two callers: the
`install_java` operation the start path raises (`Manager::lay_out_java`,
`servers/manager.rs:1111`) and the create path (`java_before_the_first_start`, `:716`).

**The one exception is deliberate, and it is a button.** `Runtimes::reinstall` (`java/mod.rs:106`)
runs `replace()` (`:138`) directly and never calls `present()`, so it asks Adoptium for the newest
build of that major even when a perfectly good one is on disk. It has exactly one caller,
`Inventory::start` (`java/inventory.rs:91`), which is `POST /admin/java-runtimes/{major}` — the
"fetch again" button on the admin page, administrator-only, and refused outright while a server is
running on that major (`undisturbed`, `:215`). The full reasoning is `docs/JAVA-OPERATOR.md` §5.
Automatic: never. On request: always, even for a major that is already there.

Why nothing happens on its own:

* A runtime is not a downloaded file, it is the thing a running server *is*. Replacing it under a
  server that is running means the next restart runs a JVM nobody chose at a moment nobody chose.
  A background job that does that at 04:00 turns "my server was fine yesterday" into a support
  question with no visible cause.
* The panel restarts servers for other reasons (an update, a crash, a schedule). A silently
  updated JVM would ride along with the next of those, so the change and its symptom would be
  separated by days.
* A quarterly Temurin release is 50 MB per major, times as many majors as the machine holds, for
  every installation that never asked. Adoptium pays for that bandwidth.

What replaces a runtime is therefore a decision somebody makes: the button, or deleting
`<data_dir>/runtimes/java-<major>` by hand. The next fetch takes whatever Adoptium calls `latest`
then. The `release` file in the tree says which build is standing there, and the panel reads it for
the runtime list, so the answer to "what am I running" is on disk and not in a database column that
could disagree with it.

Security updates are the honest counter-argument. A JVM that stands for two years carries two years
of CVEs. The answer is not silence: the panel shows the version it laid down and the day it did
(`LaidRuntime`, `java/inventory.rs:39`), and re-fetching is one press away — but the moment stays
the operator's, like every other restart of a server that belongs to somebody else.

## 4. The step up to a newer Java, and why it used to be the quiet failure

`Manager::java` (`servers/manager.rs:1048`) resolves in five steps, and the order is the whole
design:

1. **the managed tree** — `Runtimes::present` (`java/mod.rs:85`), `runtimes/java-<major>`;
2. **exactly this major, wherever it lies** — `binary_of(&here, |found| found == major)`
   (`manager.rs:1058`) over `runtimes::cached` (`settings/runtimes.rs:56`);
3. **fetch it** — if `fetches_java()` (`manager.rs:1082`) reads `java_auto_install` as on,
   `ask_for_java` (`:1092`) raises an `install_java` operation and the start is refused with
   `java_runtime_fetching` (`:1062`), which is a *"come back in a minute"*, not a failure;
4. **a stand-in** — `binary_of(&here, stands_in)` (`:1074`) with
   `runtimes::stands_in_for` (`settings/runtimes.rs:111`);
5. **`java_runtime_missing`** (`manager.rs:1079`), whose sentence names the majors the machine does have
   (`no_java_here`, `manager.rs:3567`).

Two things about step 4 were wrong in the earlier version of this document, and both are now
decided in code:

* **It takes the closest, not the newest.** `binary_of` (`manager.rs:3560`) sorts the fitting
  runtimes by major and takes the first. On a machine with 11 and 21, a server asking for 8 gets
  the 11.
* **It never climbs past the next long-term release.** `stands_in_for(found, wanted)` is
  `found >= wanted && found <= at_most(wanted)`, and `at_most` (`settings/runtimes.rs:115`) is the
  first entry of `LONG_TERM` (`:14`, `8, 11, 17, 21, 25`) above what was asked for. So
  `stands_in_for(11, 8)` is true and `stands_in_for(17, 8)` is false. **On a machine with Java 17
  and Java 25 and nothing else, a 1.12 server that asks for Java 8 is refused** — step 4 finds
  nothing and step 5 says so by name. Above 25 we know of no line yet, so `at_most` answers
  `u32::MAX` and anything is allowed (`stands_in_for(33, 25)`).

The measurement that decided the cap, on this machine (Debian OpenJDK 25.0.4), 2026-08-22:

* Mojang's own manifest for 1.12.2 names `javaVersion: {component: "jre-legacy", majorVersion: 8}`.
  That is the game saying what it was built for, and it is where `default_major()` agrees.
* Vanilla 1.12.2 nevertheless **starts** on Java 25 and reaches `Done (0.319s)!`, and so does
  Paper 1.12.2 build 1620. What both print on the way is:
  `WARNING: sun.misc.Unsafe::objectFieldOffset has been called by io.netty…` ·
  `sun.misc.Unsafe::arrayBaseOffset has been called by com.lmax.disruptor…` ·
  `WARNING: Restricted methods will be blocked in a future release unless native access is enabled`.

So the fallback was not an immediate crash, and a document that claimed it were would be wrong. It
was worse than a crash: **it worked until it did not.** The floor those old servers stand on —
`sun.misc.Unsafe`, unrestricted `System::load` — is announced for removal, so the JDK upgrade an
operator does for an unrelated reason is what ends it, months after the panel made the choice. And
when it ends, the stack trace names netty or log4j, never Java, and the panel's console shows a
server that "just stops". A refusal with the word Java in it is the better failure, and with
Temurin fetched per major it is a rare one: step 3 gets there first for every major
`default_major()` can name.

**Step 3 stands in front of step 4, and that is a real consequence.** With `java_auto_install` on —
its default — a stand-in is never used, not even on a machine that has one and no way to reach
Adoptium. §10 says what that looks like.

## 5. Where the bytes may come from, and what the checksum does not say

Five fences stand between the panel and a Java nobody asked for: the checksum, the list of hosts
the link may lead to, the roof on the bytes that are taken, the announcement weighed before the
request goes out, and the modes the archive lies under while it waits to be unpacked. They are
worth keeping apart, because the first one is weaker than it looks and the doc that came before
this one said otherwise.

**The bytes are bound to a checksum.** `checksum::write_capped`
(`crates/craftpanel/src/loaders/checksum.rs:107`) writes to `<dest>.part`, hashes while it writes
and renames only on a match — the same call every loader makes. A mismatch is
`LoaderError::Damaged`, the part file goes, **nothing is unpacked**, and the caller sees
`java_download_damaged`. An answer that carries no checksum at all is refused before a single byte
is fetched (`JavaError::NoChecksum`, `adoptium.rs:118`, code `java_download_unavailable`); there is
no "download it anyway".

What that buys is less than it reads like. **Link and hash come out of the same answer.** Whoever
writes the answer writes both, so the hash proves the bytes are the ones the answer meant — against
a truncated download, a broken mirror, a swapped GitHub asset — and proves nothing whatever about
who wrote the answer. A checksum an attacker chose for a file an attacker chose matches perfectly.

**The link may only lead to hosts named in this tree.** `ORIGINS` (`java/adoptium.rs:10`) is that
list, and it is short because the real chain is short. Followed by hand on 2026-08-23:

```
curl -s 'https://api.adoptium.net/v3/assets/latest/21/hotspot?architecture=x64&image_type=jre&os=linux&vendor=eclipse'
  → link: https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.12.1%2B1/OpenJDK21U-jre_x64_linux_hotspot_21.0.12.1_1.tar.gz
curl -sIL <that link>
  → 302 https://release-assets.githubusercontent.com/github-production-release-asset/…
  → 200 application/octet-stream, 52 059 408 bytes
```

Three hosts, and all three are needed: `api.adoptium.net` answers the question, `github.com`
carries the link Adoptium hands out, `release-assets.githubusercontent.com` is where GitHub sends
the client for the bytes. The same three for 8, 17, 21 and 25 and for both architectures — measured,
not assumed. `objects.githubusercontent.com`, the host GitHub used for release assets before it,
is deliberately **not** in the list: nothing measured today lands there. If GitHub moves its assets
again, the fetch stops with `java_download_untrusted` naming the host it was sent to, and the list
is one line longer.

The scheme is part of the comparison, because the list holds whole origins and not bare names:
`http://github.com` is not `https://github.com`, and `file:///etc/shadow` has no origin at all.
A host that merely ends in one of the names (`github.com.example.invalid`) is a different origin too
(`admits`, `loaders/http.rs:121`, compares `Url::origin().ascii_serialization()` for equality).

Two places do the comparing, and both are needed:

* `Http::admitted` (`loaders/http.rs:51`) weighs the URL before the request is sent, on the
  metadata call (`maybe_fetch`, `:73`) and on the download (`stream`, `:97`) alike. The link the
  API named never reaches the socket if it points elsewhere — the strange host is not contacted,
  not even for a `HEAD`.
* the redirect policy (`loaders/http.rs:37`) weighs **every hop**. reqwest follows up to ten
  redirects on its own and by default it does not care where to; a `302` off the list is broken off
  with `LoaderError::Untrusted` instead of followed. Within the list a redirect is followed as
  before, which is exactly what the github.com → release-assets hop needs.

**This is a client of its own.** `Runtimes::with_base` builds it with `Http::bound_to`
(`java/mod.rs:71`); the loaders keep `Http::new()` and reqwest's default policy, so Modrinth,
PaperMC, Purpur, Leaf, Fabric and Vanilla are untouched by any of this. The origin of whatever base
the panel was given is admitted as well (`adoptium::origins`, `java/adoptium.rs:16`) — in the
running panel that base is the compiled-in `adoptium::BASE` (`main.rs:142` builds `Runtimes::new`)
and nothing reads it from a config file or an environment variable, so in practice it adds
`https://api.adoptium.net` a second time. In the tests it is what lets the fake Adoptium on
`127.0.0.1` serve its own archives while a second fake on a second port stays shut out.

**A ceiling on the bytes that are actually taken.** `release.size` used to be a number for the
progress bar and nothing else, so an answer that announced 1 KiB and then delivered 64 MiB got all
64 MiB written before the checksum said no — a way to fill a shared disk with a single answer.
`ceiling()` (`java/mod.rs:219`) is the announced size plus one megabyte of slack (`ANNOUNCED_SLACK`,
`:45`), and never more than the fixed 128 MiB roof (`ARCHIVE_CEILING`, `:39`) — the roof also covers
an answer that names no size at all. `write_capped` stops **at the chunk that would burst it**
(`loaders/checksum.rs:168`), before that chunk is written, and the `.part` file is removed on the
way out. The roof is twice the largest archive Adoptium ships, rounded up to the next power of two.
Measured on 2026-08-23, the eight builds this panel can ask for — 8, 17, 21, 25, each for x64 and
aarch64 — run from 40 812 850 bytes (8, aarch64) to 61 718 288 bytes (25, x64), so 128 MiB is 2.2×
the largest of them. It was 512 MiB, which is 8.7×, and a roof that far above anything that ever
comes through is not a roof. It is a **JRE** roof, and deliberately: a Temurin 21 *JDK* is 207.5 MB
compressed (§2) and would not fit through it. `adoptium::latest` asks for `image_type=jre` and
nothing in this tree asks for anything else, so the day that changes the roof has to change with
it — which is the point of a number that sits close enough to the truth to notice.

**Between the hash and the unpacking, the file has to stay ours.** The checksum is computed on the
stream as it is written (`collect`, `loaders/checksum.rs:143`), and the unpacker then opens
`archive.tar.gz` a second time **by path** (`unpack.rs:47`): the two look at the same name, not at
the same descriptor, and whatever can write that name in between decides what is unpacked. Under a
loose umask the staging directory stood at `0777` for the whole download and the archive with it.
Three things close that window, and none of them depends on the umask: the staging directory is
created `0700` (`STAGING_MODE`, `java/mod.rs:43`, in `empty_out`, `:287`), the archive is `0600` as
soon as `write_capped` has renamed it into place (`ARCHIVE_MODE`, `:44`, set at `:197`), and the
second open carries `O_NOFOLLOW`, so a symlink put in the archive's place is refused instead of
followed. `runtimes/` above it has to stay `0755` for the game accounts (§11), so the mode of the
staging directory is the whole of the fence — which is why it is set explicitly and tested under
`umask 0000`.

Measured on 2026-08-23 with `setpriv --reuid=65534 --regid=65534 --clear-groups` against a staging
laid out by hand: `runtimes/` at `0755` and the staging at `0700` gives that account
*Permission denied* four times over — listing the directory, putting a symlink over
`archive.tar.gz`, renaming the directory away, reading the file. The same account with `runtimes/`
at `0777` renames the whole staging aside and puts its own `0700` directory in its place in one
command, and then owns whatever the second open reads. **The fence is one level up**, and that is
the reason a world-writable `runtimes/` is refused outright rather than written into (§11): with it
in place, no account but the panel's own and root can reach into the window between the hash and
the open, and both of those are the panel.

**An announcement that is impossible costs nothing at all.** `ceiling()` clamps to the roof, and
clamping is silent: an answer that announced 8 GiB used to get its `GET` sent and the whole roof
pulled off the wire before `write_capped` said no. Nothing about that answer was ever going to end
well, and the size says so in the answer itself, so `fetch()` weighs `release.size` against the roof
**before the request goes out** (`java/mod.rs:173`) and stops with `JavaError::AnnouncedTooLarge`,
code `java_download_announced_oversized`. It is a different sentence from
`java_download_oversized`, on purpose: one says the announcement was impossible and not one byte was
fetched, the other says what arrived did not keep to what was announced. Whoever reads the message
learns which of the two happened.

**What none of this buys, said plainly.** Whoever controls `api.adoptium.net` still decides which
build this panel installs. The host list does not make the source verifiable; it only takes away
the freedom to name an arbitrary machine as the place to fetch from, and to bounce the fetch onward
from there. An attacker in that position can still name any file on `github.com` — the list is a
list of hosts, not of paths — and can still name a checksum that matches it. Adoptium publishes a
detached signature beside each asset (`signature_link`, a `.sig` next to the `.tar.gz`; present in
every answer measured on 2026-08-23), and verifying it is the only thing that would turn "we trust
the answer" into "we checked the build"; it needs Adoptium's public key pinned in this tree and a
verifier to go with it, and it is not built — `adoptium::Package` (`java/adoptium.rs:76`) does not
even deserialise the field. `checksum_link`, the sha256 as a file next to the asset, is not fetched
either: it is a second request to the same trust, which would look like verification and be none.
§13 keeps the whole list of what is not protected.

## 6. The unpacking cannot leave the tree, whatever the archive says

The first version of this checked every entry against the disk and then wrote it by path, and that
was wrong in a way worth writing down, because it is the shape most tar extractors have.

**The hole.** `lands_inside()` used to canonicalise the link target component by component, so it
could only see the links that were *already* on the disk when it looked. An archive names its
entries in whatever order it likes, so the door comes after the way through it:

```
1. lib/out  -> "door/../.."      judged: door does not exist yet, so the name stays literal,
                                 the target reads as the tree root, allowed
2. lib/door -> ".."              allowed on its own, and it is what real archives contain
3. lib/out/PLANTED.txt           written: lib/out now really points two levels above the tree
```

`install()` answered `Ok(...)` and left `<data_dir>/runtimes/PLANTED.txt` behind — outside the tree
and outside the staging directory that is swept up afterwards. With more doors it reaches as far as
it likes. **Every design that checks first and writes later has this race**; moving the check later
only makes the archive that beats it longer.

So the writing itself was made unable to leave. `Beneath` (`unpack.rs:196`) holds an open descriptor
on the staging root and lays every entry down relative to it:

* `leaf()` (`:231`) splits the confined name on `/` and refuses an empty step, `.` and `..` itself,
  at the syscall boundary, the way `craftpanel-helper/beneath.rs` does with
  `craftpanel_proto::is_valid_step` — `confine()` already guarantees it, and the writer does not
  take its word for it.
* each directory on the way is `mkdirat` + `openat` with **`O_DIRECTORY | O_NOFOLLOW`**
  (`make_dir()`, `:278`, flags in `DIRECTORY`, `:19`). A step that is a symlink — one the archive
  laid down itself, one second ago — is refused by the **kernel** with `ENOTDIR`/`ELOOP`. Files are
  created with `O_CREAT | O_EXCL | O_NOFOLLOW` (`NEW_FILE`, `:20`) in the descriptor that walk
  reached, links with `symlinkat` into it. No name is ever resolved through a string join, and no
  symlink is ever followed.

**Why `O_NOFOLLOW` per component and not `openat2(RESOLVE_BENEATH)`.** `craftpanel-helper/beneath.rs`
reaches into an account directory that legitimately contains links, so it asks the kernel for
"anywhere under this root" and keeps a per-component walk for kernels before 5.6. Here nothing has
to be followed at all: a real Temurin writes no entry through a directory that is a link (measured
below), so refusing every link on the way is *stricter* than `RESOLVE_BENEATH`, which would still
follow the ones that stay inside. It also needs no kernel newer than `openat` and therefore no
second road with a second behaviour. The rule beneath both files is the same one: the kernel says
no, our code does not predict the future.

Two things stayed, and one of them changed its nature:

* `confine()` (`:374`) still refuses `..`, `/` and any prefix in an entry name.
* a hard link is still refused outright (`:140`) — a Java runtime holds none.
* `lands_inside()` (`:414`) is now **lexical and touches no disk**: it counts how deep the link sits,
  allows leading `..` while that depth lasts, and refuses a `..` that comes **after a name**, because
  that name may be a door and no amount of looking can tell in advance. It is no longer what stops
  the escape — the writer is — it is what keeps an installed runtime free of links that lead out of
  it, and it gives the operator the sentence *"a link out of the runtime directory to …"* instead of
  a kernel errno. What it accepts cannot leave the tree by induction: the `..` steps are taken before
  any name, so they climb real parents, and every name they then descend through is either a real
  directory or a link that passed the same rule.
* a name the archive uses twice is unlinked before it is written again (`unbind`, `:308`), so the
  last entry wins rather than the whole install dying on `EEXIST`; a name it gives to two different
  *kinds* — a file where one of its own directories stands, or a path through one of its own files —
  is refused, and `blocked()` (`:444`) turns the errno into *"under a name it already gave to
  something else"*.
* the ceilings are `MAX_ENTRIES`, `MAX_BYTES`, `MAX_FILE_BYTES` and `MAX_NAME_BYTES` (`:12-15`),
  and every one of them is weighed on a header before the body behind it is read — see below,
  because both the place they were weighed in and the numbers themselves were holes.
* a name may be no more than **16 steps deep** (`MAX_DEPTH`, `:16`), counted in `confine()` while
  it walks the components, so the count is in before a single directory is dug.
* **every directory the writer digs is counted like an entry** and charged `DIRECTORY_BYTES`
  (`:17`) against the byte ceiling — the ones the archive names and the ones only `leaf()` knows
  about, because a name three steps deeper than the last one costs three inodes whether or not any
  header mentions them. `make_dir()` reports whether it created the directory or found it
  (`:278`), `leaf()` adds up what it dug on the way (`:231`), and the loop weighs entries and bytes
  again after the entry has landed (`:177-179`). The two together are what make the ceilings
  multiply out: 4 096 entries of 16 steps each are still at most 4 096 inodes.

Refusing symlinks altogether would have been stricter still, and it is not open to us: the real
`OpenJDK21U-jre_x64_linux_hotspot_21.0.12.1_1.tar.gz`, fetched and listed on 2026-08-23, holds
**145 symlinks among 320 entries** — the whole `legal/` tree is
`legal/<module>/LICENSE → ../java.base/LICENSE` — and **0 hard links**. It also writes **no** entry
through a directory that is a symlink, which is what makes `O_NOFOLLOW` on every component free of
cost. Rejecting links would not be a hardening, it would be a broken feature.

**A real Temurin is six steps deep, and the limit is 16.** Measured on 2026-08-23 by streaming all
eight builds this panel can ask for and listing every name in them: the deepest name in every one of
the eight is **6** steps —
`jdk-21.0.12.1+1-jre/conf/security/policy/limited/default_US_export.policy`,
`jdk8u502-b07-jre/lib/security/policy/limited/US_export_policy.jar`,
`jdk-25.0.4.1+1-jre/conf/security/policy/limited/exempt_local.policy` and the same shape on
aarch64 — and a Temurin 17 *JDK*, the larger shape, is **6** as well. 305 of the 21 JRE's 320
entries are three or four steps deep. So the limit is **twice the deepest measured, rounded up to
the next power of two**, which is the rule the byte ceilings already follow and lands on the same
2.7× headroom they have. Ten steps below every Temurin ever shipped is room for a level Adoptium
adds on a Tuesday. It is not room for a forest.

It was `256`, taken from `MAX_DEPTH` in `craftpanel-helper/src/users.rs:19` on the argument that a
tree this writer lays down must be one the helper can walk back afterwards. That argument is still
true and it never fixed a number: the helper's limit says what it is willing to *walk*, not what
Adoptium *packs*, and borrowing it put 250 unearned steps into a shape that has used six for
seventeen releases. What the 250 were worth, measured on 2026-08-23: an archive of **23 249 bytes**
carrying 512 names of 251 steps each — 1 026 headers, every one of them inside `MAX_ENTRIES`, inside
`MAX_DEPTH`, and declaring **578 bytes** of file between them — was **accepted**, and laid down
**128 515 inodes and 526 397 440 bytes** (502 MiB) of empty directories in 5 seconds. `Unpacked`
answered `578` for it. With a valid `bin/java` and a `release` beside the forest, `install()`
answered `Installed { … fresh: true }` and `runtimes/java-21/bin/java` was there to run: the forest
passed `usable()` and stayed. Before that, `<root>/d/d/…/deep.txt` with 1000 steps was dug in 72 ms
and accepted, and everything that walks a tree afterwards — `files::measure`, `remove_dir_all`, the
helper — inherited it.

### The modes are a rule of ours, and no longer the archive's

The writer used to take `mode & 0o777` out of the tar header. That dropped setuid, setgid and
sticky — measured, `0o4755` came out `0o755` — and kept everything else, group-write and
world-write with it. **The archive is a file from the network, and a file from the network does not
get to say who may write into a runtime.**

Why that mattered more here than in a general-purpose extractor: the `bin/java` this lays down is
handed to `craftpanel-helper`, which starts it under the *unprivileged account of one server*
(`craftpanel-helper/src/main.rs:284`, `.uid(account.uid)`). One tree is shared by every server on
the machine. A world-writable `libjvm.so` in it is code execution in every other user's server, and
users being separated from each other is the promise the whole panel rests on.

Measured on 2026-08-23, with the real
`OpenJDK8U-jre_x64_linux_hotspot_8u502b07.tar.gz` repacked so every directory declares `0o777`,
every program `0o4777` and every other file `0o666` — a shape whoever controls the answer can serve
(§5 says so plainly), since link and hash come out of the same answer:

| | old rule | now |
|---|---|---|
| the 21 directories | 20 × `0o777`, the tree root `0o755` | 21 × `0o755` |
| the 47 programs | `0o777` | `0o755` |
| the 87 other files | `0o666` | `0o644` |
| `lib/amd64/server/libjvm.so` | `0o777`, and uid 65534 appended to it | `0o755`, `Permission denied` |

So the header is asked exactly one question now, and it is the only one it can honestly answer:
**is this a program or is it not?** `granted()` (`unpack.rs:270`) reads `0o111` and returns
`PROGRAM_MODE` `0o755` or `PLAIN_MODE` `0o644` (`:22-24`); directories are `TREE_MODE` `0o755`
whatever they declared, and so is the tree root. Nothing group- or world-writable can come out of
that, and setuid, setgid and sticky have nowhere to enter — not because they are stripped, but
because the number they would sit in is never read.

The cost is one thing and it is small: the eleven `0o444` files a Temurin ships come out `0o644`.
They are read-only against the panel account that owns them, which is not a fence between users;
the fence is the group and other bits, and those are tighter than the archive asked for. The gain
is that a mode on disk no longer depends on a byte that came over the wire.

**Explicitly set, not left to the umask, and that had a hole in it.** `O_CREAT` and `mkdirat` are
both masked by the umask, so the mode is `fchmod`ed after the descriptor exists — the tree root in
`Beneath::open` (`:201`), every file in `Beneath::file` (`:214`), every directory in `make_dir`
(`:278`). The `fchmod` in `make_dir` closes a real gap: only directories the archive *named* were
chmodded before, while the ones `leaf()` creates on the way to a deeper entry — `lib/` in an archive
that only names `lib/libjsig.so` — kept `mkdirat`'s umasked mode. Under a `UMask=0027` in the unit
that is `0o750`, and a game account cannot walk through it. §11 once claimed this loose end was tied
off; it was tied off for half the directories, and now it is tied off for all of them.

The single root directory Temurin ships (`jdk8u502-b07-jre/`) is stripped, and it must be single:
two roots in one archive are refused rather than guessed at (`:123-134`). Anything that is not a
file, a directory or a symlink — a device node, a fifo — is refused as well (`:173`), where it used
to be written as a plain file.

### The ceilings, what each is measured against, and when it is weighed

The loop steps over entries. The archive's own root is one: its name leaves nothing under the tree
to lay down, so it `continue`s. `bytes += entry.size()` stood **after** that `continue`, so the one
entry whose path is exactly the root name was skipped without ever being weighed — while `tar-rs`
still reads its body through the gzip stream on the way to the next header. Demonstrated: a root
entry declaring 3 GiB never reached the ceiling and ended in *"unexpected EOF during skip"*, while
the same entry under the name `<root>/lib/rt.jar` was refused cleanly. The weighing now comes
**before every `continue` in the loop** — the entry count first (`unpack.rs:82`), then the header
type, and then the bytes (`:106`), so nothing is stepped over unweighed and no name has to be
understood before a size is: an extension header is weighed against `MAX_NAME_BYTES`, everything
else against the sum and against `MAX_FILE_BYTES`. All three of those weighings are the same
`weighed()` (`:186`), and there is a fourth, the only one that cannot come first: the directories
the entry dug are counted after it has landed (`:177-179`), because how many of them were already
there is not something a header can say. That one can overshoot by what a single name can dig, which
`MAX_DEPTH` holds to 15.

That closes the one door. It closed neither the class nor the numbers, and the numbers were the
larger hole: **2 GiB for a tree of 190 MiB is not a ceiling, it is an invitation.** Measured on
2026-08-23 by streaming all eight builds this panel can ask for and adding up what their headers
declare:

| build | archive | entries | directories | unpacked | largest single file | deepest | longest name |
|---|---|---|---|---|---|---|---|
| 8 x64 | 41 851 657 | 157 | 20 | 109 797 721 | 65 731 230 `lib/rt.jar` | 6 | 67 |
| 17 x64 | 46 640 574 | 322 | 63 | 141 553 758 | 84 533 676 `lib/modules` | 6 | 85 |
| 21 x64 | 52 059 408 | 320 | 62 | 165 240 383 | 101 049 043 `lib/modules` | 6 | 85 |
| 25 x64 | 61 718 288 | 326 | 61 | 199 841 004 | 99 447 348 `lib/modules` | 6 | 78 |
| 8 aarch64 | 40 812 850 | 157 | 20 | 108 515 487 | 65 731 225 | 6 | 67 |
| 17 aarch64 | 45 989 327 | 321 | 63 | 142 586 054 | 84 532 918 | 6 | 83 |
| 21 aarch64 | 51 149 460 | 319 | 62 | 165 053 972 | 101 049 183 | 6 | 85 |
| 25 aarch64 | 60 479 792 | 325 | 61 | 198 485 044 | 99 447 379 | 6 | 78 |

A whole runtime is **190.6 MiB** at its largest and one file of it is **96.4 MiB** at its largest,
and seventeen majors took the tree from 104.7 MiB (Java 8) to 190.6 MiB (Java 25) — 82 % of growth
across seventeen releases. So each ceiling is **twice the largest measured, rounded up to the next
power of two**, and not one of them is a number somebody felt was about right — with two that keep a
wider margin and give their reason underneath the table rather than in place of one:

| | ceiling | largest measured | headroom |
|---|---|---|---|
| `MAX_BYTES` (`unpack.rs:13`) | 512 MiB | 190.6 MiB — Java 25, x64 | 2.7× |
| `MAX_FILE_BYTES` (`:14`) | 256 MiB | 96.4 MiB — `lib/modules` of Java 21, aarch64 | 2.7× |
| `MAX_NAME_BYTES` (`:15`) | 4 KiB | 85 bytes | `PATH_MAX` |
| `MAX_ENTRIES` (`:12`) | 4 096 | 387 — Java 25, x64: 326 entries and 61 directories | 10.6× |
| `MAX_DEPTH` (`:16`) | 16 | 6 — all eight of them | 2.7× |
| `ARCHIVE_CEILING` (`java/mod.rs:39`) | 128 MiB | 58.9 MiB — Java 25, x64 | 2.2× |

**What an entry costs, measured, because the number that used to stand here was wrong.** This
section said an entry costs *a header and a syscall, not a gigabyte*, and rested `MAX_ENTRIES` on
it. An entry that names a directory — or a name one step deeper than the one before it, which no
header has to mention at all — costs an **inode and a directory block**. On the ext4 that `/var/tmp`
sits on, measured on 2026-08-23: **128 515 directories took 526 397 440 bytes**, which is 4 096
bytes each to within a rounding error, and one inode each out of the finite number the filesystem
was formatted with. The tar bodies behind all of them came to 578 bytes. Taken up to the 4 096
headers that were allowed — 2 048 names, because a name that long needs a `././@LongLink` in front
of it — that is **514 048 inodes and 1.96 GiB** out of an archive of about 93 KB, and the byte
ceilings would not have seen a byte of it, because only file bodies were counted and directories
were free.

So a directory is charged what it costs: **one against `MAX_ENTRIES` and `DIRECTORY_BYTES` = 4 KiB
against `MAX_BYTES`** (`unpack.rs:17`), whether the archive named it or `leaf()` had to dig it. A
real runtime pays 61 directories — 249 856 bytes against a ceiling of 512 MiB, and 387 of the 4 096
counted, which is the 10.6× in the table. `MAX_ENTRIES` stays at 4 096 rather than the 1 024 the
rule would give, and the reason is measured too: the JRE is the small shape.
`OpenJDK17U-jdk_x64_linux_hotspot_17.0.20.1_1.tar.gz`, listed on 2026-08-23, is **543 entries and 88
directories, 631 counted** — 1 024 would be 1.6× a shape Adoptium ships today, and 4 096 is 6.5× it.
What the count had to be wide against was never the count but the product, and the product is
counted now. It was 40 000 once, which was 103× the JRE and 63× the JDK.

`MAX_NAME_BYTES` keeps a wider margin than the rest, and for a reason that does survive being
measured: it is `PATH_MAX`. A name longer than that cannot be created on Linux at all, so a header
announcing more is announcing something no filesystem could hold, and being generous costs four
kilobytes of memory rather than four kilobytes of ceiling.

**Both byte ceilings are weighed on the header, before the body behind it is read.** `entry.size()`
comes out of the tar header; the sum (`unpack.rs:106`) and the single file (`:108`) are checked
against it before `beneath.file()` copies a byte, and the sum is a `saturating_add`, so a header
naming `u64::MAX` cannot wrap it back to nothing. What the single file is worth, measured: an
archive of 1 528 994 bytes declaring one `release` file of 1 572 864 056 bytes used to install
**successfully** — the file landed in `runtimes/java-21/release`, `install()` answered `Ok`, and
every `discover()` afterwards read it whole, peak RSS **+1 514 MiB**. With only a sum to answer to,
2 GiB in one file was a legal runtime.

**And a name is weighed before the tar crate gets to it.** `tar-rs` reads a GNU long name, a GNU
long link and a pax `x` record to the end **inside** `next_entry()`, into a `Vec`, before it hands
the caller anything: a `././@LongLink` declaring 3 GiB is an allocation no per-entry ceiling can
see, and it costs the archive 3 MB of zeros to write. Measured against the old code, with the
stream ceiling at 2 GiB as the only brake: **peak RSS +2 065 MiB, 56 seconds**, and only then the
refusal — from an archive of 3 MB, far below the download roof. A lower stream ceiling would only
have made that number lower; it would not have made it small. So the loop asks for **raw entries**
(`entries()?.raw(true)`, `:79`): the tar crate hands over every physical header and reads nothing
behind it, and the long name, the long link and the pax records are read here instead — after
`MAX_NAME_BYTES` has been weighed against the header (`:88`) and through a `take()` that cannot
exceed it (`held_aside`, `:259`). The same archive now costs **41 MiB of peak RSS**, which is the
test binary and not the archive, and the refusal names the header that lied. The largest thing the
unpacker holds in memory at once is one such name, 4 KiB of it; every body moves through the 8 KiB
buffer of `io::copy`. The byte ceilings bound the **disk** now, and nothing else has to.

Raw entries pay for themselves twice more. A pax `x` record may override an entry's size and
`tar-rs` honours it, so the size we weigh and the size it skips over could disagree — two parsers,
one archive, which is how the interesting tar attacks are built; raw entries never let them differ.
And the long names are ours to bound rather than ours to hope about: `spoken_for()` (`:252`) names
the four header types that carry them, `shorn()` (`:266`) takes the name up to its `NUL`, and the
next member gets it. No Temurin needs any of this — none of the eight builds has a name over 100
bytes or a pax member at all — but a name of 101 bytes would make GNU `tar` write a `././@LongLink`
tomorrow, and an unpacker that then refused the runtime would be a worse bug than the one being
closed here.

`Capped` (`unpack.rs:352`) stays underneath all of it, counting what is really read out of the
decoder and failing the read that would take it past `MAX_BYTES`. It is a backstop now rather than
the only brake: with every header weighed before its body, the stream can outgrow the ceiling only
through the 512 bytes of header and the padding behind each entry. It sets a flag rather than only
returning an error, because by the time that error comes back out of the tar crate it is an ordinary
read failure that reads like a corrupt archive; `tree_capped` (`:37`, the check at `:57`) turns the
flag back into *"more than … bytes in the stream itself, whatever its headers say"*.

### The `release` file is read with a ceiling of its own

The unpacker's ceilings are not the whole story, because `runtimes/java-<major>/release` is read by
`settings::runtimes::read_home` (`settings/runtimes.rs:152`) on **every** `discover()`, and that
file does not have to have come through the unpacker — anything that can write into `runtimes/` can
put one there by hand. It was `std::fs::read_to_string` with no ceiling at all: the 1.5 GiB file
above was read whole every time the runtime list was built, and the panel called that a Java 21.

`release_of` (`:170`) opens the file and reads through `take(RELEASE_CEILING + 1)`; a file that
gives back more than the ceiling is not a `release` file and the directory is not a runtime. The
number is 64 KiB against a measured 483 bytes (8), 1 638 (17), 1 628 (21) and 1 615 (25) — 40× the
largest, and deliberately not two-times-and-round-up like the ceilings above, because here the
ceiling *is* the harm: what it bounds is one allocation of that size, so generosity costs 64 KiB
and tightness would cost a runtime the day a `MODULES` line grows.

### Two tar shapes that are not Temurin's, and are not attacks either

`git archive` and `tar --format=pax` write a `pax_global_header` in front of everything, and
`tar czf x.tgz .` writes every name with a leading `./` and a `./` entry of its own. Neither is what
Adoptium ships today. Both went through this unpacker as *archives*: the global header became the
first root name and collided with the real one, and the `./` entry fell into `confine()`'s
*"an entry without a name"* and took the whole install down with it. That is a feature that breaks
silently the day Adoptium changes how it packs, so both are stepped over now — the global header by
its **entry type** (`spoken_for()`, `unpack.rs:252`, which names the pax `g` and `x` records
alongside the two GNU long-name types) and not by its name, the `./` because `confine()` returns an
empty path for a name made of nothing but `.` and the loop skips what names nothing. An entry whose
name is genuinely empty is still refused, and it is the same sentence as before.

Stepped over is not ignored: the pax records are weighed against `MAX_NAME_BYTES` before they are
read and the `./` against `MAX_BYTES` before it is skipped, neither sets the root name, and an
archive that really does hold two root directories is refused exactly as it was — behind a
`pax_global_header` and behind a `./` alike, which is what
`stepping_over_the_two_of_them_steps_over_no_second_root` holds down.

## 7. Whole, or not at all

Everything happens in `<data_dir>/runtimes/.java-<major>.new/` (`Runtimes::staging`,
`java/mod.rs:214`) — the archive, then `tree/` beside it, behind a `0700` of its own (§5). The name
is fixed, not random, so **leftovers from an earlier failure are found at the start of the next
attempt** and either put back or swept up (`stand_back_up`, `:250`, then `empty_out`, `:287`).
`settings::runtimes::discover` looks for `bin/java` directly under each entry of `runtimes/`, so the
staging directory is invisible to it while it fills — everything inside it sits one level deeper
than `discover` looks.

Only when `tree/bin/java` is a file, is executable, reads as a Java runtime and reads as the major
that was asked for (`usable()`, `:226`) does the tree move into place. `swap_in` (`:328`) does it in
three renames, all of them inside `runtimes/` and therefore inside one filesystem and therefore
atomic one by one:

```
1. .java-<major>.new/tree      -> .java-<major>.new/ready       the checked tree, ready to move in
2. java-<major>                -> .java-<major>.new/previous    what stood there, if anything
3. .java-<major>.new/ready     -> java-<major>                  the new one takes the name
```

If the third fails, the second is undone (`:343`) and the error names the path. **`Manager::java`
never sees half a runtime**, and that matters more than it looks: it tests for `bin/java` alone, so
a broken tree with the right name would be picked and the server would fail to start with nothing
pointing at Java.

**A crash between the renames leaves a name that says what to do.** The machine can lose power
between steps 2 and 3, and then `java-<major>` is simply gone while the runtime sits under
`previous`. So the next attempt begins with `stand_back_up` (`:250`): if `java-<major>` is missing
and the staging directory holds a `ready` or a `previous` — in that order, the checked new tree
before the old one — it is renamed back into place before anything is swept. Only then is the rest
of the wreckage removed. Three tests hold the three cases down (§12), and all three assert that
Adoptium was not asked at all: what is already on the disk is not fetched again.

**What is put back has to be ours, though.** A rescued tree is code that will be started, and the
staging directory it was found in has a name anybody who can write into `runtimes/` could guess. So
`ours_alone` (`:270`) is asked about both the staging directory and the tree inside it: a directory,
owned by the account the panel runs as, and not writable by group or others. Anything else is
logged and swept, and the install goes and fetches instead. That is a cheap check and it is not a
proof — between the check and the rename the same writer could act again — but the only account
that could exploit it is one that can already write into `runtimes/`, and that account owns the
runtimes anyway.

The staging directory is removed whether the attempt worked or not (`:150`).

## 8. One download, however many callers

Two people creating a 1.20 server in the same second both need Java 21. `Runtimes` holds a map of
`major → Job` (`java/mod.rs:63`), a `Job` holds a `tokio::sync::Mutex` and the shared `Progress`.
The second caller waits on the mutex, and when it gets its turn the runtime is already there, so it
returns without touching the network (`fresh: false`). The entry is taken out of the map by the last
caller that holds it (`retire`, `:121`, on an `Arc::strong_count` of two: the map and itself), so a
`watch()` that is never followed by an `install()` leaves one entry per major standing until the
next install for that major clears it.

In the process, not in the database: the panel process is the only thing that writes into
`runtimes/`, so there is nothing for a second *live* process to race with — what a *dead* one may
have left is §7's business. **That makes one thing binding, and it is wired that way:** `Runtimes`
is built once (`main.rs:141`) and the same `Arc` is handed to `Manager` (`:175`) and to `Inventory`
(`:145`). A fresh `Runtimes` per request would have a fresh job map and would download the same
50 MB twice.

## 9. What the caller can show

`Step` (`ops/store.rs:47`) is how this panel reports progress and `watch_between`
(`backups/mod.rs:975`) how it is filled: an atomic counter the work writes and a poller reads every
300 ms (`PROGRESS_POLL`, `:37`). `java::Progress` (`java/progress.rs:47`) is that counter, and
`java::report` is the bridge between it and an operation row.

`total()` is the size the API announced, `done()` the bytes so far, `stage()` one of
`waiting/asking/downloading/unpacking/done`, and `share()` (`progress.rs:66`) folds the two halves
onto one line — download to 0.9 (`DOWNLOAD_SHARE`, `:3`), unpacking the rest, the same split
`move_tree` uses (`UNPACK_SHARE`, `files/archive.rs:21`, applied at `:519`). The unpacking counts
the compressed bytes it reads back out of the archive (`Counted`, `unpack.rs:339`), which is why
`done()` starts over when the stage turns and why `total()` stays what it was.

`watch(major)` (`java/mod.rs:94`) hands out the handle **before** `install(major)` is awaited, so the
caller can start its poller without a race, and two callers on one download watch the same numbers.
`report::lay_out` (`java/report.rs:11`) does exactly that pairing for an operation: it announces the
`installing_java` phase, spawns a 300 ms ticker over `share()` and `done()` into the operation row,
and stops it when the install returns. `report::blame` (`:27`) turns a `JavaError` into the
operation's `error_step` — `Filesystem` for a write or for a directory in the way (`Unreachable`,
`Exposed`), `Internal` for a dead task, `Download` for
everything else.

`Stage::as_str()` returns a token, not a sentence, and so does `JavaError::code()`
(`java/error.rs:65`):

| code | when |
|---|---|
| `java_download_unsupported` | the machine is neither x64 nor aarch64; the message names `apt install` |
| `java_download_unavailable` | Adoptium builds no such major, or named no checksum |
| `java_download_damaged` | the bytes do not match the sha256 that came with the link |
| `java_download_untrusted` | the link, or a redirect on the way, leads off the hosts in `ORIGINS` |
| `java_download_oversized` | the download ran past the size announced for it, or past the 128 MiB roof |
| `java_download_announced_oversized` | Adoptium announced more than the 128 MiB roof, so nothing was fetched |
| `java_download_failed` | Adoptium unreachable, refused, or answering in a shape we do not read |
| `java_archive_rejected` | the tar climbs out, holds a hard link, two roots, or is too large |
| `java_runtime_incomplete` | unpacked, but no runnable `bin/java` of the right major |
| `java_runtime_unreachable` | a directory on the way to `bin/java` shuts game accounts out and is not ours to open; the message names the path, the mode, the owner and the `chmod` |
| `java_runtime_exposed` | `runtimes/` may be written by every account on the machine, so nothing is laid into it; the message names the `chmod o-w` |
| `java_runtime_unwritable` | `<data_dir>/runtimes/` cannot be written |
| `internal` | the unpacking task died |

The codes the *panel* adds around this engine are not in that list and are not in this module:
`java_runtime_fetching` and `java_runtime_missing` on the start path (§4), and
`java_major_unknown`, `java_runtime_not_here`, `java_runtime_in_use`, `java_install_running` from
`java::Inventory` (`docs/JAVA-OPERATOR.md` §6).

**Which of these got words, and which did not.** The five stage tokens are translated in both
catalogues (`admin.runtimes.stage-*`, mapped in `web/src/pages/admin/Runtimes.vue:404`
`stageLabel`). The error **codes are not**: the admin page shows the English sentence the backend
wrote, straight out of the row (`failureOf`, `web/src/pages/admin/runtimes.ts:34`, into the
`Admonition` at `Runtimes.vue:189`), and so does the start path's refusal. That is a known gap and
not an accident of this document: `JavaError`'s sentences are written to be read by an operator, and
nobody has yet decided that they are worth a catalogue entry each, in two languages. Whoever
decides they are writes the words into `web/src/locales/en-US` and `de-DE` and keys them by `code()`
— there is no English to inherit from here.

## 10. A machine with no network

`Http` (`loaders/http.rs:11-13`) gives up after 5 s on connect, 15 s on the metadata call and 30 s
without bytes on the download. An unreachable Adoptium is `LoaderError::Unreachable` — *"Adoptium is
not reachable: the connection could not be opened"*, code `java_download_failed`. The staging
directory is removed on the way out, nothing lands under `java-<major>`, and the next attempt starts
from nothing.

**What such a machine looks like from a server's point of view depends on the switch, and the
earlier version of this document had it backwards.** With `java_auto_install` on — the default —
step 3 of §4 stands in front of the stand-in step, so a start that finds no runtime of exactly the
right major does *not* fall back to a near one. It raises an `install_java` operation and refuses
with `java_runtime_fetching`; the operation then runs, cannot reach Adoptium, and fails with
`java_download_failed` in the operation's error row. A machine with Java 11 installed and a 1.12
server on it, which would have started before this feature, does not start now. The presses do not
stack while the fetch is queued or running — `Manager::power` asks `guard_write`
(`servers/manager.rs:844`) first and a server with an open operation is `server_busy` — but nothing
counts the failures either: once the row is `failed` the next press raises a new `install_java`, and
`insert` (`ops/store.rs:137`) has no unique index to bump against, because
`0016_operations_installing_java.sql` writes one only for the two backup kinds. An operator who
keeps pressing collects one failed row per press. Turning the switch off
(`docs/JAVA-OPERATOR.md` §2) restores exactly the old behaviour: the stand-in is used, and if there
is none the refusal is `java_runtime_missing` with the majors the machine has named in it.

Server *creation* is the gentler half: `java_before_the_first_start`
(`servers/manager.rs:705`) fetches the runtime as part of the create operation between 0.65 and 0.90
of the bar, and a failure there is a `tracing::warn!` and nothing else (`:717`). The server is
created; it will complain when it is started.

An air-gapped installation is not locked out either — unpack a Temurin tarball into
`<data_dir>/runtimes/java-21/` by hand, so that `bin/java` and `release` sit directly under it, and
`discover()` reads it as `Source::Managed`, which is the first place `Manager::java` looks. The
directory is a plain convention, not a registry. Mind §11's modes while doing it: a game account has
to be able to walk in and execute.

The one moment a human is watching is the installer, so that is where it is said: `install.sh:177-179`
warns when the machine has **neither** a `java` nor a reachable `api.adoptium.net`, and names the
package to install. When either is there, it says nothing — a warning that fires on a healthy
machine is a warning nobody reads.

## 11. What the installer does, and what it deliberately does not

`ensure_accounts` (`install.sh:275`) creates the directory (`:326`):

```
install -d -o "$SERVICE_USER" -g "$SERVICE_GROUP" -m 0755 "$DATA_DIR/runtimes"
```

* **The panel owns it**, because the panel is the only thing that writes there. `$DATA_DIR` itself
  is `root:craftpanel 1771` (`:315`), so the panel could create `runtimes/` on its first fetch
  anyway; doing it here makes the mode deterministic instead of leaving it to a umask.
* **0755, not 0750 like the `cache/` beside it** (`:317`). A game server runs as its own managed
  account, which shares no group with the panel, and it has to execute
  `runtimes/java-<major>/bin/java`. `0750` would make every server that needs a fetched runtime fail
  to start, and the message would be about a missing file, not about a mode.
* **It is in `ensure_accounts`**, which `do_install`, `do_update` and `do_upgrade` all call
  (`:1320`, `:1432`, `:997`), and `install -d` puts an existing directory right as well as making a
  new one. So an installation from before this feature gets the directory on its next run, and a
  directory somebody chmodded gets its mode back — the same repeatability the other steps have.
* Nothing is added to the units. `ReadWritePaths=$DATA_DIR` (`:411`) already covers it and
  `ProtectSystem=strict` does not forbid executing from a path it makes writable.

**The umask was the loose end, and it is tied off — now for every directory.** The tree that
becomes `java-<major>/` used to take its mode from the panel's umask, so a `UMask=0027` in the unit
— a reasonable-looking hardening — would have taken every fetched runtime out of reach of the game
accounts, and the message would have been about a missing file rather than about a mode. Every
directory and every file is `fchmod`ed after it is created (§6), including the ones no entry names
and `leaf()` makes on the way, which was the half that was still missed. The staging directory and
the archive in it are the other half, and they go the other way — `0700` and `0600`, explicitly,
because nobody but the panel has business there while it fills (§5).

**`runtimes/` itself is checked on every install, not only when the panel made it.** The mode the
installer sets can be changed afterwards, and a `runtimes/` at `0700` is the quiet version of this
whole failure: the tree below it comes out `0755`, the install reports success, and the game account
gets `Permission denied` on `bin/java` with nothing in the message about a directory two levels up.
So `make_reachable` (`java/mod.rs:295`) runs before every attempt (`lay_out`, `:130`; `replace`,
`:147`) and asks four questions in this order:

1. **Is it there?** If not, it is created with `0755` (`REACHABLE`, `:40`) and the rest follows.
2. **May every account write into it?** If `o+w` is set (`WRITABLE_BY_ANYONE`, `:42`), the install
   stops with `JavaError::Exposed`, code `java_runtime_exposed`, and the message names `chmod o-w`.
   A world-writable `runtimes/` makes every other check here worthless: the tree can be replaced the
   moment it stands, and the staging directory can be swapped while it fills — which is exactly the
   assumption §5 leans on when it calls the staging's `0700` the whole of the fence. Group-writable
   is left alone: the group is the panel's own `$SERVICE_GROUP`, and no game account is in it.
3. **Can a stranger get through it?** If `o+x` is missing (`PASSABLE`, `:41`) and the panel owns the
   directory, the mode is put right — `mode | 0755`, so nothing else the operator set is taken away.
4. **And if it still cannot?** `strangers_pass` (`:314`) refuses with `JavaError::Unreachable`, code
   `java_runtime_unreachable`, before Adoptium is asked for anything. That is the case of a
   `runtimes/` at `0700` that belongs to another account: the panel does not silently take a
   directory the operator gave away, it names the path, the mode, the owner's uid and the
   `chmod o+rx` that fixes it.

`strangers_pass` then asks the last question of `<data_dir>` itself (`:311`), which the panel does
not own either — `root:craftpanel 1771`, whose last digit is the `o+x` a game account walks through,
and whose leading `1` is the sticky bit that keeps the accounts out of each other's entries. A data
directory at `0750` fails the same way, with the same sentence — an install that would produce a
runtime nobody can start does not happen.

**The way a game account reaches `bin/java`, walked end to end** on 2026-08-23 with a real
`jdk8u502-b07-jre` laid down by this writer:

```
/var/lib/craftpanel   1771 root:craftpanel      other may enter, not list
  runtimes            0755 craftpanel           other may enter and list
    java-8            0755 craftpanel           the tree root, TREE_MODE
      bin             0755 craftpanel
        java          0755 craftpanel
```

`setpriv --reuid=65534 --regid=65534 --clear-groups .../bin/java -version` — an account in no group
of the panel's, standing in for one of the machine's own local users — printed
`openjdk version "1.8.0_502" … OpenJDK 64-Bit Server VM (Temurin)(build 25.502-b07, mixed mode)`.
The same account writing into the tree got `Permission denied` for `libjvm.so`, for `bin/java` and
for a symlink over `libjvm.so`. Read and execute for everyone, write for the owner: both halves
measured, from the outside, on the same tree.

**The installer carries no switch for this**, and that is a decision. The panel has one:
`panel_settings.java_auto_install` (`migrations/0015_java_auto_install.sql`,
`PanelSettings::java_auto_install`), the first switch in that table that starts *open*, with its
reasons written at the head of the migration, and its control is the toggle in
`web/src/pages/admin/settings/Java.vue:12`, which reaches the settings page through
`web/src/pages/admin/settings/sections.ts:15`. An environment variable in `install.sh` would be a
second answer to the same question, in a place the operator only visits when installing, and the two
would drift the first time somebody changed one of them. What the installer prepares is the
directory; whether anything is fetched into it is the panel's own setting.

## 12. What is tested, and against what

Against a fake Adoptium (`java/harness.rs`, axum on `127.0.0.1:0`, the same shape as
`drive/harness.rs` and `content/harness.rs`), never against the network — with one exception marked
`#[ignore]`, like the seven network tests that were already there:
`live_java_8_comes_down_from_adoptium_and_runs` (`java/tests.rs:542`) fetches the real 40 MB,
verifies the real sha256, unpacks it and **runs `bin/java -version`**, asserting the output says
`OpenJDK` and `1.8.0`. Java 8 because it is the smallest and the reason Temurin was chosen.

The malicious archives are built in the tests: an entry climbing out with `..`, an absolute path
aimed at a file the test then proves was never written, a symlink to `/etc/shadow`, a chain of
symlinks through a link the archive itself laid down, a hard link, and two root directories. Beside
them the honest failures: a wrong checksum, a missing checksum, no build for that major, no
`bin/java`, a `bin/java` without the execute bit, no `release` file, and a Java 17 archive answering
a request for Java 21. Every one of them goes through `refused()` (`java/tests.rs:33`), which
asserts that `java-<major>/` does not exist afterwards and that the staging directory is gone.

The archives that beat the old check live next to the writer they now bounce off, in
`unpack.rs`'s own tests, and they all run through `install()` and then **walk the whole data
directory looking for a file named `PLANTED.txt`** (`planted_below`, `unpack.rs:478`), so the
assertion is "nothing anywhere outside", not "the one place I thought of is empty":

* `the_door_that_the_archive_opens_after_the_way_out_plants_nothing` is the attack of §6 word for
  word — `lib/out → door/../..`, then `lib/door → ..`, then `lib/out/PLANTED.txt`.
* `the_same_way_out_with_the_door_laid_down_first_plants_nothing_either` is the same three entries
  in the other order, which is the whole point: the order stopped mattering.
* `doors_stacked_one_behind_the_other_get_no_further_than_one` is the one that survives a lexical
  check — `lib/a → ..`, `lib/a/c → ..`, `lib/a/c/e → ..`, then a file under it. Each link is
  innocent where the archive puts it and each is a level further out on the disk.
* `not_even_a_door_that_leads_back_inside_is_walked_through` writes through a link that stays in the
  tree, and is refused too, which is the strictness `RESOLVE_BENEATH` would not have.
* `a_file_that_the_archive_hides_behind_one_of_its_own_files_is_refused` and
  `a_file_that_the_archive_lays_over_one_of_its_own_directories_is_refused` are the two collisions
  `blocked()` names.
* `the_shape_a_real_temurin_carries_is_laid_down_whole` is the other side: `legal/java.xml/LICENSE →
  ../java.base/LICENSE` **named before its target exists**, `lib/server/libjsig.so → ../libjsig.so`,
  and the modes come out `0755` and `0644` — the `0444` the archive asks for included, because
  read-only against the owner is not a fence between users (§6).

**The modes are held from both ends, and one test walks everything.** `an_archive_of_loose_modes()`
(`unpack.rs:887`) offers what a hostile answer would: `0o777` and `0o707` directories, `0o4755` and
`0o777` and `0o2711` programs, `0o666` and `0o444` files, and a `lib/` no entry names.

* `the_only_thing_the_archive_is_believed_about_is_whether_it_is_a_program` is `granted()` alone:
  six modes with an execute bit anywhere become `0755`, six without become `0644`.
* `no_mode_the_archive_offers_group_or_others_is_taken` names each of those entries on disk, and
  `a_setuid_bit_in_the_archive_does_not_survive_the_unpacking` is the same point for `0o4755` on the
  launcher itself.
* `every_step_of_the_installed_tree_is_walked_and_none_of_it_is_loose` takes the list from the
  disk instead: `harness::nothing_is_loose` (`java/harness.rs:366`) walks the whole installed tree
  and holds **every** file and directory to it — nothing group- or world-writable, no setuid, setgid
  or sticky, directories exactly `0755`, files `0644` or `0755`. An exception added later has
  nowhere to hide, because no test names the entries it is meant to cover.
  `a_temurin_shaped_archive_leaves_nothing_loose_either` and
  `the_shape_a_real_temurin_carries_is_laid_down_whole` run the same walk over the honest shapes,
  and so does the live test, over the real 40 MB.
* `the_modes_are_the_writers_own_and_never_the_umasks` is the umask half, and it needs a process of
  its own: `umask` is process-wide and the tests run in threads, so setting it here would colour
  every other test in the binary. It re-runs this one test through `current_exe()` with
  `--exact`, the child sets `umask(0o077)`, unpacks and walks, and asserts `runtimes/` above the
  tree is still `0755`. The parent fails unless the child's output says `1 passed`, so a renamed
  test cannot quietly stop running.

Run against the writer as it stood before, all of them fail; the walk fails first and says
`…/java-21/release is 666, which group or others may write`.

**The counter-proof, run on 2026-08-23.** With the descriptor writer taken out again and everything
else left standing, `doors_stacked_one_behind_the_other_get_no_further_than_one` fails with
`planted ["…/runtimes/PLANTED.txt"]` — the check alone does not hold. With both the writer and the
lexical rule taken out, the plain §6 attack plants the same file. And with only the lexical rule
taken out, the two door tests fail on the *wording* of the refusal while nothing at all is written
outside: the kernel refuses either way, which is exactly the division of labour the two are meant to
have. Beside the fakes, the real
`OpenJDK21U-jre_x64_linux_hotspot_21.0.12.1_1.tar.gz` was unpacked through the new writer and
compared with what GNU `tar -xzf` makes of it: **319 entries identical in type, mode, size, name and
link target, 112 files identical by md5**, and the `bin/java` it lays down prints
`Temurin-21.0.12.1+1`.

Two of the tests run **two** fake Adoptiums, because that is what the attack needs: the one the
panel was told to ask, and a second one on a second port standing in for the stranger. In
`a_link_that_leads_off_adoptiums_own_hosts_is_never_even_asked_for` the first answers with a `link`
into the second; in `a_redirect_onto_another_host_is_broken_off_instead_of_followed` the link stays
at home and the `302` points away. Both assert `served() == 0` on the second fake — the stranger was
not asked for a byte — and both assert the refusal names the host it would not go to.
`a_redirect_that_stays_on_a_host_we_trust_is_followed` is the other half of that: a `302` inside the
list installs the runtime, so what is refused is the host and not the redirect.

The ceiling is tested from both ends.
`a_download_that_runs_past_the_size_it_announced_is_cut_off_where_it_bursts` announces 1 024 bytes,
serves 16 MiB and then reads `Progress::done()` — the bytes that actually came off the wire — and
asserts they stayed under 4 MiB. Beside it,
`a_stream_that_grows_past_its_ceiling_stops_at_the_chunk_that_bursts_it`
(`loaders/checksum.rs:297`) counts the chunks the stream was asked for: 5 of 1 024 against a ceiling
of 4 096 bytes, which is the difference between stopping where it bursts and stopping at the end.
The third end is the announcement: `a_size_no_runtime_could_have_is_refused_before_a_byte_is_asked_for`
announces 8 GiB and asserts `served() == 0` and `Progress::total() == 0` — the download endpoint of
the fake was never called and the bar never left `asking`. `the_ceiling_follows_the_announced_size_and_stops_at_the_fixed_roof`
is `ceiling()` on its own, including the answer that names no size.

The ceilings inside the unpacker are tested where they are cheap to test.
`a_size_is_weighed_before_the_entry_declaring_it_is_stepped_over` builds the same 3 GiB declaration
three times — once as the root entry, once as a `./`, once as a `pax_global_header` — and asserts
each is refused for the size it announced and not for the read that would follow it; with the
weighing moved back behind the `continue` the first of them fails with *"unexpected EOF during
skip"*, which is the attack as it was reported. The third of them is refused by `MAX_NAME_BYTES`
rather than by `MAX_BYTES` now, because a pax header is metadata and never becomes unpacked bytes,
and the test says which sentence it expects from which shape.
`a_name_the_tar_crate_would_swallow_whole_is_weighed_before_it_gets_to` appends a 64 KiB
`././@LongLink` to an otherwise ordinary archive under the real ceiling and asserts the refusal
names *"65536 bytes of name"* — before the fix the same archive was read to the end and refused as
*"members found describing a future member"*, which is to say after the tar crate had it all in
memory. `the_stream_is_weighed_too_where_every_header_of_it_is_honest` gives `tree_capped` a
1 024-byte ceiling and an archive whose two entries declare 66 bytes between them and sit flat under
the root, so no directory is dug and nothing but the stream can burst: nothing lies, and the headers
and their padding still burst it, which is the one thing `Capped` is left to catch. Beside it,
`a_directory_the_archive_never_names_is_counted_and_charged_for_all_the_same` is the same two
entries with the launcher back under `bin/`, and reads the count and the price off `Unpacked`:
**3 entries** for two names, and `DIRECTORY_BYTES + 66` bytes for two bodies of 66.
`the_stream_is_cut_at_the_read_that_bursts_the_ceiling_and_says_it_was` is the reader on its own:
four bytes through a ceiling of six, then the read that would make ten, which errors and raises the
flag.
`a_name_and_a_target_too_long_for_a_header_are_taken_from_the_ones_that_carry_them` installs a
runtime whose licence sits under a 120-byte directory name and whose `lib/there` points at a
120-byte target, so both the `././@LongLink` and the `@LongLink` for the target have to be read,
bounded, and handed to the member behind them — the path a real Temurin does not use today and
would use the day a name of its own passes 100 bytes.

`java::attack` holds the four that were reported, and each of them fails loudly on the code as it
was. `a_file_larger_than_any_file_a_temurin_holds_is_never_written` serves an archive whose
`release` is 300 MiB of padding behind a valid header — three times the largest file any Temurin
ships — and asserts the install is refused, that nothing is left under `runtimes/`, and that fewer
than 128 KiB of the archive were pulled through before the refusal; on the old code the install
**succeeded** and peaked at 341 MiB of RSS.
`a_name_that_announces_gigabytes_is_refused_before_the_tar_crate_reads_it` puts a `././@LongLink`
announcing 3 GiB in front of an 8 MiB payload of incompressible noise and asserts the same
128 KiB — the old code pulled all 8 MiB into a `Vec` first, and with the payload as large as the
announcement it pulled 2 065 MiB and took 56 seconds.
`a_release_file_of_gigabytes_is_no_release_file_and_is_never_read_whole` writes a valid `release`
by hand and then grows it to 1 572 864 056 bytes of hole: `read_home` used to answer *Java 21* after
reading all of it (peak RSS 1 514 MiB, and every `discover()` did it again), and answers `None` now.
Beside it, `a_release_file_is_read_up_to_a_ceiling_and_a_longer_one_is_no_release_file`
(`settings/runtimes.rs:250`) puts one file of exactly 64 KiB and one of 64 KiB + 1 through
`read_home`.

The fourth is the forest, and it is measured rather than believed.
`a_forest_of_directories_out_of_a_handful_of_names_is_refused_before_it_is_dug` builds the reported
archive word for word — **512 names of 250 steps each**, 23 249 bytes of gzip — hands it straight to
`unpack::tree` so that nothing sweeps the wreckage before it can be counted, and then **walks what
was dug**: `dug_into` (`java/attack.rs:127`) counts the entries under the tree and adds up
`st_blocks × 512` for every one of them. It asserts the refusal says *"nested deeper than 16"*, and
that what lies there afterwards is **3 inodes and 12 288 bytes** — `bin/`, `bin/java` and `release`,
which are the entries in front of the forest. On the code as it was: **128 515 inodes, 526 397 440
bytes, and `Ok`**. `a_forest_with_a_launcher_beside_it_is_still_a_forest_and_is_not_installed` is
the same archive through `install()`, where the launcher and the `release` file would have carried
it past `usable()`: the install is refused and `runtimes/` is left holding nothing at all.
`the_directories_between_the_names_are_counted_the_way_the_names_themselves_are` is the other half,
and it is the one a shallower `MAX_DEPTH` alone would not catch: **1 024 names of 14 steps each**,
every one of them **exactly at `MAX_DEPTH`** and well inside `MAX_ENTRIES` at 1 026 headers, digging
14 directories apiece. It asserts the refusal says *"entries and directories"* and that **4 098**
inodes were dug where the archive asked for 15 360. With the counting taken back out and the depth
left at 16, that archive is accepted: 15 363 inodes, 62 926 848 bytes, and an `Unpacked` reporting
**1 090 bytes** for 60 MiB of disk.

The two tar shapes have four tests.
`a_pax_global_header_in_front_is_stepped_over_and_the_runtime_lands` and
`the_leading_dot_that_plain_tar_writes_is_stepped_over_too` install a runtime out of an archive in
either shape — the second one with the names written into the header by hand, because the
tar crate's own builder strips a leading `./` before it writes it.
`stepping_over_the_two_of_them_steps_over_no_second_root` puts a real second root behind each of the
two and asserts both are still refused with *"two root directories"*. And
`a_name_is_kept_only_if_every_step_of_it_leads_further_in` keeps `confine("")` an error while
`confine(".")` and `confine("./")` come back empty.

**The way through to `bin/java` is tested from outside the panel's own account.** Where the test
runs as root it starts the unpacked launcher under uid 65534 with `a_stranger_runs`
(`java/tests.rs:653`) and reads the version out of it — an account in no group of the panel's, which
is what a game account is.

* `a_runtimes_directory_that_was_already_shut_is_opened_before_java_lands_behind_it` puts a `0700`
  `runtimes/` there first, the way an earlier release left it, and asserts `0755` afterwards and a
  launcher the stranger can run. This is the case the older writer got wrong: it only set the mode
  on a directory it had created itself, so an install into an existing `runtimes/` reported success
  and produced a `bin/java` nobody could reach.
* `a_runtimes_directory_that_belongs_to_someone_else_stops_the_install_and_says_why` gives that
  directory to another account and asserts `java_runtime_unreachable`, the `chmod o+rx` in the
  message, the mode and the uid in it, that the directory is left exactly as it was, and that
  Adoptium was not asked (`upstream.asked() == 0`).
* `a_data_directory_a_game_account_cannot_enter_is_named_and_not_written_into` is the same for
  `<data_dir>` itself at `0750`.
* `the_staging_shuts_everyone_else_out_while_the_bytes_come_down` and
  `the_archive_on_its_way_in_takes_the_writers_mode_and_not_the_umasks` run under `umask 0000` in a
  child process of their own (`under_a_umask_of_its_own`, `:668`, the same trick the unpacker's
  umask test uses) and read the modes while the download is held open: `0700` on the staging,
  `0600` on `archive.tar.gz`, `0755` on `runtimes/` and on the finished tree.
* `a_runtimes_directory_anyone_may_write_into_is_refused_rather_than_filled` sets it to `0777` and
  asserts `java_runtime_exposed`, the `chmod o-w` in the message, and that Adoptium was not asked.
* `an_entry_nested_deeper_than_any_runtime_is_refused_before_it_is_dug` and
  `an_entry_at_the_deepest_step_still_allowed_is_laid_down` stand on either side of `MAX_DEPTH`:
  a name of 15 steps of `d/` under the root, which is 17 deep, is refused with *"nested deeper than
  16 directories"*, and one of 14, which is 16 deep, is laid down and read back. Between them,
  `a_runtime_twice_as_deep_as_any_temurin_is_still_laid_down_whole` installs a tree of `2 × 6` steps
  and reads the file at the bottom of it, so the headroom over the six Adoptium actually ships is
  the assertion and not the prose.

The interrupted swap of §7 has three, and each one starts by writing a runtime into the staging
directory the way a crash would have left it (`java/tests.rs:567`, `a_runtime_on_disk`):

* `a_runtime_that_a_crash_left_standing_aside_is_put_back_before_anything_is_swept` — only
  `previous` is there and `java-21` is gone: the old runtime comes back, `fresh` is false, and the
  fake Adoptium's `asked()` is `0`.
* `a_crash_with_the_new_tree_already_standing_by_finishes_the_swap` — `ready` and `previous` are
  both there: `ready` wins, because it is the tree that already passed `usable()`.
* `a_runtime_that_stands_where_it_belongs_is_not_pushed_aside_by_what_a_crash_left` — `java-21`
  stands and `previous` is a leftover: the one in place stays, the leftover is swept.

Two more say what is *not* put back:
`a_tree_left_in_a_staging_anyone_could_write_to_is_swept_up_rather_than_installed` (the staging
chmodded `0777`) and
`a_tree_another_account_left_in_the_staging_is_swept_up_rather_than_installed` (the staging and the
tree chowned to uid 65534, so it needs root and says so when it is not).
Both assert `fresh`: the rescue was refused and the runtime came down from Adoptium instead.

The rest of the module's tests are listed where they belong: `java/inventory.rs` and
`api/runtimes.rs` in `docs/JAVA-OPERATOR.md` §8, and the resolver's own
`a_runtime_stands_in_only_up_to_the_next_long_term_release` in `settings/runtimes.rs:328`.

### The machine the tests run on says nothing

`candidates()` used to read a constant of its own: `/usr/lib/jvm`, `/usr/java`, `/opt/java`,
`/opt/jdk`, plus `$JAVA_HOME` and whatever `java` on the `PATH` resolves to — on whatever machine it
happened to run. Two tests in `servers::manager` want *"no Java 8 here"* and never made it so, they
assumed it. This machine has 21 and 25 and no 8, so both passed; `ubuntu-latest` carries Temurin 8,
11, 17 and 21 pre-installed, so on the runner the resolver found a real Java 8, nothing was fetched,
and both fell — `a_new_server_gets_the_java_it_will_need_before_anybody_presses_start` on the
`InstallingJava` it never saw, and on the refusal that never came in
`a_start_that_finds_no_java_fetches_it_and_says_so_instead_of_dying`. Reproduced here by
bind-mounting a Temurin 8 over `/usr/lib/jvm`: **50 passed, 2 failed**, exactly those two.

**The places are handed in now.** `Search` (`settings/runtimes.rs:20`) carries all three of them as
data — the roots, `$JAVA_HOME`, the `PATH` — and `discover()`, `cached()` and `candidates()` take
one instead of reading the machine. `Search::system()` (`:27`) is the same four roots in the same
order with the same two environment variables behind them, so nothing about a running panel moved.
`Config` carries it (`config.rs:18`, `#[serde(skip)]`: no key in `config.toml`, nothing for an
operator to set wrong), `Inventory` is handed one, and `Runtimes::present()` asks with
`Search::nowhere()` — it only ever looks at what the panel laid down itself, and a managed runtime
is read out of `<data_dir>/runtimes/` and takes its `(major, vendor)` slot before any system one is
even considered.

**In a test build the default is `Search::nowhere()`** (`Default for Search`, `:66`). Every harness
that builds a `Config::default()` is blind to the machine without knowing that any of this exists,
and a test that wants a system Java plants one and hands in `Search::under([…])`. `$JAVA_HOME` and
the `PATH` are muted the same way, and for the reason no test may set them either:
`std::env::set_var` is process-wide and would colour every other test in the binary, exactly as
`umask` does.

`a_test_looks_at_none_of_the_places_the_panel_looks_at_on_a_machine` (`settings/runtimes.rs:283`) is
the guard. **The list comes from `Search::system()` — from the constant, not from the test** — and
none of those roots may be in what a `Config` carries in a test, and `$JAVA_HOME` and the `PATH`
must be `None` there. Then it plants a Temurin 8 in a scratch directory and finds it three times
over, once through a root, once through `$JAVA_HOME`, once through the `PATH`, so that the emptiness
means blindness and not a resolver that finds nothing anywhere. With the default put back to
`Search::system()` it fails with *"/usr/lib/jvm is searched in a test"*.

**A phase is caught and no longer hoped for.** The first of the two tests had a second bet in it: it
slept 25 ms at a time, 200 times over, and hoped to see `InstallingJava` — which a fake Adoptium on
loopback can run through in less than one of those sleeps. The fake is shut instead
(`FakeAdoptium::shut`, `java/harness.rs:118`): the download waits at the door, the run cannot leave
the phase, the test reads it, and then the gate is opened again. That door also replaced the 300 ms
and 500 ms sleeps in `the_size_and_the_stand_are_readable_while_the_download_runs` and
`the_staging_shuts_everyone_else_out_while_the_bytes_come_down`, which were hoping to catch a stage
and a staging directory the same way. The test that was 0.6 s of sleeping is 0.08 s now.

**Run on 2026-08-23** on this machine (Java 21 and 25), on the same machine with a Temurin 8
bind-mounted over `/usr/lib/jvm` and `$JAVA_HOME` and the `PATH` pointing at it, and on one with
`/usr/lib/jvm` emptied and `/usr/bin/java` bound over with `/dev/null`: **1 350 passed** in each of
the three.

## 13. What is not protected here

Short, and without softening, because §5 and §6 are long enough to read like a wall:

* **Whoever controls `api.adoptium.net` controls which build this panel installs.** The answer names
  the link and the hash together; matching one against the other proves nothing about who wrote
  them. No signature is verified, no key is pinned, `signature_link` is not even parsed.
* **Whoever controls that answer can name any file on `github.com`** or on
  `release-assets.githubusercontent.com`. `ORIGINS` is a list of hosts, not of paths or of
  repositories.
* **TLS and the system trust store are the whole transport story.** A certificate authority that
  mis-issues for one of those three hosts is not caught by anything in this module.
* **The archive decides the file names inside the tree.** It cannot leave the tree and it cannot
  choose the modes, but a runtime whose `bin/java` is a program of the attacker's choosing is
  exactly what a compromised answer delivers, and the panel will run it as a game account.
* **`usable()` reads a `release` file and an execute bit.** It does not know what a Temurin is; a
  tarball that carries a plausible `release` and a runnable `bin/java` passes.
* **Nothing re-checks a runtime after it is laid down.** Modes are set once, at unpacking time. Root
  on the machine, or anything that can write into `<data_dir>/runtimes/`, can change the tree
  afterwards and the panel will not notice.
* **A runtime the panel did not lay down is trusted on sight.** The air-gapped way in of §10 —
  unpack a Temurin into `runtimes/java-21/` by hand — goes through no part of §6: whatever modes,
  links and files that tarball carried are what the game accounts get. `discover()` asks for
  `bin/java` and a `release` file, and that is the whole examination.
* **The checks on the directories are checks, not locks.** `make_reachable` and `ours_alone` read a
  mode and an owner and then act on what they read; anything that can write into `runtimes/` between
  the two can make the answer stale. What they defend against is a mis-set mode and a leftover, not
  an attacker who already has the panel's own account — and against root nothing here defends.
* **One process is assumed.** Two panels on one `<data_dir>` would race in `runtimes/` with nothing
  but rename atomicity between them; the panel does not take a lock file, and the installer does not
  stop a second instance from being pointed at the same directory.
* **The stand-in cap is a rule of thumb, not a compatibility matrix.** `LONG_TERM` is a list in a
  source file; a game version that breaks on the release after its own is not something this panel
  can know.
* **The 128 MiB roof and the unpacker's ceilings bound one install, not the disk.** Nothing here
  counts what all the majors together take up, and nothing sweeps a runtime that is no longer used.
  The inodes are bounded per install now (4 096 plus the 15 a last name can dig) and not across
  them, and `files::measure` still adds up file bytes only, so what a runtime costs in inodes is a
  number the operator is never shown.
