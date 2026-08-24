#!/usr/bin/env bash
# CraftPanel installer. Run it once to install, run it again to update or remove.
#   curl -fsSL https://<host>/install.sh | bash
set -euo pipefail

REPO="${CRAFTPANEL_REPO:-MCbabel/craftpanel}"
PREFIX="${CRAFTPANEL_PREFIX:-/usr/local/bin}"
CONFIG_DIR="${CRAFTPANEL_CONFIG_DIR:-/etc/craftpanel}"
DATA_DIR="${CRAFTPANEL_DATA_DIR:-/var/lib/craftpanel}"
RUN_DIR="/run/craftpanel"
CGROUP_ROOT="/sys/fs/cgroup/system.slice/craftpanel-games"
SERVICE_USER="craftpanel"
SERVICE_GROUP="craftpanel"
ACCOUNT_PREFIX="craft-"
FETCH_TMP=""
trap on_exit EXIT

BOLD=$'\033[1m'; DIM=$'\033[2m'; GREEN=$'\033[32m'; RED=$'\033[31m'; YELLOW=$'\033[33m'; OFF=$'\033[0m'

say()  { printf '%s\n' "$*"; }
step() { printf '%s==>%s %s\n' "$GREEN" "$OFF" "$*"; }
warn() { printf '%s warning:%s %s\n' "$YELLOW" "$OFF" "$*"; }
die()  { printf '%s error:%s %s\n' "$RED" "$OFF" "$*" >&2; exit 1; }

# What this run has made, newest first. An installation that broke off halfway
# used to leave the service account, its group and two directories standing,
# with nothing on the machine that would ever take them away again — and the
# next run found them and read a half-made installation as a finished one. So
# every one of them is written down as it is made, and a run that ends badly
# takes back exactly that much. What was already here belongs to whatever was
# here before and is not this run's to remove, not even on a failure.
#
# Only do_install arms this. An update and the move from the old name work on an
# installation that already stands, where "what this run made" and "what has to
# stay" are the same thing.
INSTALL_MADE=()
UNDO_READY=""

remember() {
	[ -n "$UNDO_READY" ] || return 0
	INSTALL_MADE=("$1" "${INSTALL_MADE[@]}")
}

# Said once the installation is on the disk in full. What can still go wrong
# after this is a service that will not come up, and that is something to look
# at — with the units, the data and the journal all still there — rather than
# something to throw away.
installation_stands() {
	UNDO_READY=""
	INSTALL_MADE=()
}

undo_partial_install() {
	local made what
	[ "${#INSTALL_MADE[@]}" -gt 0 ] || return 0

	say
	warn "the installation did not go through — taking back what this run made:"
	for made in "${INSTALL_MADE[@]}"; do
		what="${made#*:}"
		case "${made%%:*}" in
			unit)
				systemctl disable --now "${what##*/}" >/dev/null 2>&1 || true
				rm -f "$what"
				say "    $what"
				;;
			file)
				rm -f "$what"
				say "    $what"
				;;
			dir)
				rm -rf "$what"
				say "    $what"
				;;
			user)
				if userdel "$what" >/dev/null 2>&1; then
					say "    the account $what"
				else
					warn "the account $what stays — something is running under it. Remove it with: pkill -u $what && userdel $what"
				fi
				;;
			group)
				# userdel takes the account's own group with it, so by the time
				# this comes round the group is usually gone already. What is
				# worth a word is the other case: it is still here because
				# somebody else was put in it.
				if ! getent group "$what" >/dev/null; then
					:
				elif groupdel "$what" >/dev/null 2>&1; then
					say "    the group $what"
				else
					warn "the group $what stays, somebody else is in it: getent group $what"
				fi
				;;
		esac
	done

	INSTALL_MADE=()
	systemctl daemon-reload >/dev/null 2>&1 || true
	say "${DIM}Nothing that was on this machine before the run has been touched.${OFF}"
	return 0
}

# One place for both: the temporary directory of a download that broke off, and
# the half-made installation behind it.
on_exit() {
	local status=$?
	[ -z "$FETCH_TMP" ] || rm -rf "$FETCH_TMP"
	[ "$status" -eq 0 ] || undo_partial_install
}

# "1 servers" is how a machine counts. Where the plural is not just an s, it is
# given: plural 2 "game process" "game processes".
plural() {
	local count="$1" one="$2" many="${3:-$2s}"
	# `=` and not `-eq`: a count that could not be read is empty, and asking
	# `[ "" -eq 1 ]` fails the test itself, which under `set -e` ends the run
	# over a word in a sentence.
	if [ "$count" = "1" ]; then
		printf '%s %s' "$count" "$one"
	else
		printf '%s %s' "$count" "$many"
	fi
}

# Questions are read from the terminal, not stdin: with `curl … | bash` stdin is
# the script itself. Setting CRAFTPANEL_NONINTERACTIVE=1 answers everything from the
# environment instead, which is what the acceptance test and unattended installs need.
ask() {
	local prompt="$1" default="${2:-}" var="${3:-}" answer

	if [ -n "$var" ] && [ -n "${!var:-}" ]; then
		printf '%s' "${!var}"
		return
	fi
	if [ -n "${CRAFTPANEL_NONINTERACTIVE:-}" ] || [ ! -r /dev/tty ]; then
		printf '%s' "$default"
		return
	fi

	if [ -n "$default" ]; then
		read -r -p "$prompt [$default]: " answer </dev/tty || true
		printf '%s' "${answer:-$default}"
	else
		read -r -p "$prompt: " answer </dev/tty || true
		printf '%s' "$answer"
	fi
}

confirm() {
	local answer=""
	read -r -p "$1 [y/N]: " answer </dev/tty || true
	[[ "${answer,,}" == y* ]]
}

require_root() {
	[ "$(id -u)" -eq 0 ] || die "this installer needs root. Try: curl -fsSL … | sudo bash"
}

# The half of the file name that the machine decides; the released bundles are
# craftpanel-linux-x86_64.tar.gz and craftpanel-linux-aarch64.tar.gz. Not the
# Rust target triple: how the binaries are linked is a build decision that may
# change, and the name a user downloads should not change with it. scripts/release.sh
# writes exactly these two names — the two sides have to keep saying the same thing,
# or this installer downloads nothing and the user only reads "download failed".
detect_arch() {
	case "$(uname -m)" in
		x86_64|amd64)  printf 'x86_64' ;;
		aarch64|arm64) printf 'aarch64' ;;
		*) die "no CraftPanel release is built for $(uname -m) — the published bundles are x86_64 and aarch64. What still works here: build one on this machine with scripts/release.sh and install that, sudo CRAFTPANEL_BUNDLE=<the file it writes into dist/> ./install.sh" ;;
	esac
}

preflight() {
	[ "$(uname -s)" = "Linux" ] || die "CraftPanel runs on Linux only"
	command -v systemctl >/dev/null || die "systemd is required"
	command -v curl >/dev/null || die "curl is required"

	# Java is not a requirement any more: the panel fetches the runtime a game
	# version asks for into $DATA_DIR/runtimes itself (docs/JAVA.md). What is
	# still worth a word is the machine that has neither — no Java here and no
	# way to reach Adoptium — because there the first server would not start.
	if ! java_here && ! adoptium_reachable; then
		warn "no Java on this machine and api.adoptium.net is out of reach — the panel fetches its own runtimes and cannot here. Try: apt install openjdk-21-jre-headless"
	fi

	if [ ! -e /sys/fs/cgroup/cgroup.controllers ]; then
		warn "cgroup v2 is not mounted — per-user CPU and memory limits will not work"
	fi
}

java_here() {
	command -v java >/dev/null && return 0

	local candidate
	for candidate in "$DATA_DIR"/runtimes/java-*/bin/java; do
		if [ -x "$candidate" ]; then
			return 0
		fi
	done
	return 1
}

adoptium_reachable() {
	curl -fsS --max-time 5 -o /dev/null https://api.adoptium.net/v3/info/available_releases 2>/dev/null
}

# The address to print the panel under, and never one that costs the run.
# `hostname -I` is not on every machine — busybox has a hostname without -I, and
# some images have none at all — and under `set -o pipefail` such a pipeline
# fails, which in an assignment ends the script on the spot. That used to happen
# one line before the administrator password was printed. Where the address
# cannot be found, 127.0.0.1 is the honest answer: the panel is reachable there
# in any case, and the operator knows his own addresses better than this does.
machine_address() {
	local addr=""

	addr="$(hostname -I 2>/dev/null | awk '{print $1; exit}')" || addr=""
	if [ -z "$addr" ] && command -v ip >/dev/null; then
		addr="$(ip -4 -o addr show scope global 2>/dev/null | awk '{split($4, a, "/"); print a[1]; exit}')" || addr=""
	fi
	printf '%s' "${addr:-127.0.0.1}"
}

# Empty means "not installed", and every other outcome has to look like that too:
# no binary, a binary that will not run, one that answers something unexpected.
# The `|| true` is for pipefail — a craftpanel that exits non-zero would otherwise
# take the whole run down through set -e.
installed_version() {
	[ -x "$PREFIX/craftpanel" ] || return 0
	"$PREFIX/craftpanel" --version 2>/dev/null | awk '{print $2}' || true
}

# curl that answers with the body and, on a line of its own at the end, the HTTP
# status. Without that line every miss looks alike, and they are not alike: 404
# is GitHub saying there is no such thing, 000 is curl saying it never got that
# far. The `|| true` is there because a curl that gave up still wrote the status
# line, and that line is the whole point of asking.
github_api() {
	curl -sL --max-time 20 -H 'Accept: application/vnd.github+json' -w '\n%{http_code}' "$1" 2>/dev/null || true
}

# The newest published release. Every way this can go wrong used to end in the
# one sentence "could not reach GitHub" — and the likeliest of them by far, a
# repository whose releases page is simply still empty, then sent the very person
# who had just run the one-liner off to look at his network. releases/latest
# answers 404 both for a repository that is not there and for one that has
# published nothing, so telling those two apart takes a second question, and it
# is only ever asked on the way to an error message.
latest_version() {
	local answer code tag

	answer="$(github_api "https://api.github.com/repos/$REPO/releases/latest")"
	code="${answer##*$'\n'}"

	case "$code" in
		200)
			# Anchored on the field name and read off one long line, so it does not
			# depend on GitHub pretty-printing its JSON one field to a line — which is
			# the only reason the old `grep | sed` ever picked the right string.
			tag="$(printf '%s' "${answer%$'\n'*}" | tr -d '\n' | sed -nE 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v?([^"]+)".*/\1/p')" || true
			[ -n "$tag" ] ||
				die "GitHub named the newest release of $REPO without a version in it. Name the one you want and nothing is asked: CRAFTPANEL_VERSION=1.2.3 bash install.sh"
			printf '%s' "$tag"
			;;
		404)
			answer="$(github_api "https://api.github.com/repos/$REPO")"
			[ "${answer##*$'\n'}" != "404" ] ||
				die "there is no repository $REPO on GitHub. CRAFTPANEL_REPO says which one to install from; left alone it is MCbabel/craftpanel"
			die "$REPO has published no release yet — GitHub answers fine, there is simply nothing there to download. Until the first one is out, build a bundle on this machine and install that: scripts/release.sh, then sudo CRAFTPANEL_BUNDLE=<the file it writes into dist/> ./install.sh"
			;;
		403|429)
			die "GitHub is rate-limiting this machine and will not say what the newest release is. Wait an hour, or name the version and skip the question: CRAFTPANEL_VERSION=1.2.3 bash install.sh"
			;;
		000)
			die "GitHub could not be reached — no network, no name resolution, or a proxy in the way. A machine that stays offline installs from a bundle built elsewhere: sudo CRAFTPANEL_BUNDLE=<the .tar.gz> ./install.sh"
			;;
		*)
			die "GitHub answered $code when asked for the newest release of $REPO. Name the version to install and it is not asked again: CRAFTPANEL_VERSION=1.2.3 bash install.sh"
			;;
	esac
}

# Set out loud or not at all. `CRAFTPANEL_NO_CHECKSUM=no` reading as "yes, skip
# it" would be a nasty way to end up installing something unchecked, so anything
# that is neither yes nor no stops the run instead of being guessed at.
skipping_checksum() {
	case "${CRAFTPANEL_NO_CHECKSUM:-}" in
		"") return 1 ;;
		y*|Y*|1|true|TRUE) return 0 ;;
		n*|N*|0|false|FALSE) return 1 ;;
		*) die "CRAFTPANEL_NO_CHECKSUM is set to '$CRAFTPANEL_NO_CHECKSUM', which is neither yes nor no. Write yes to install a bundle that has no checksum, or leave it unset" ;;
	esac
}

# The last point at which a stranger who piped this into a root shell can still
# turn the bundle down. A missing sum used to be a warning he read afterwards: a
# 404 or a timed-out request on the .sha256 alone was enough to unpack unchecked
# bytes and install them as root, and nothing asked him first. So a missing sum
# ends the run now. The one honest reason to go on anyway is a bundle of your own
# that never had a sum beside it, and that reason has a name to type rather than
# a default to be caught out by.
#
# The sum is printed and not merely approved of. "checksum verified" against a
# number nobody sees says only that a file matched a file that travelled with it;
# printed, it is something to hold against what the release page shows. It is
# still not a signature — whoever could change the bundle could change the sum
# beside it — and what it catches is a download that went wrong, not a bundle
# that was swapped.
verify_bundle() {
	local tmp="$1" missing="$2" want got

	if [ ! -s "$tmp/sum" ]; then
		skipping_checksum || die "$missing"
		warn "installing the bundle unchecked, because CRAFTPANEL_NO_CHECKSUM says to"
		return 0
	fi

	want="$(awk '{print $1; exit}' "$tmp/sum")"
	got="$(sha256sum "$tmp/bundle.tar.gz" | awk '{print $1}')"
	[ "$want" = "$got" ] ||
		die "the bundle is not what its checksum says it is, and nothing has been installed. published $want, arrived here $got"
	say "  checksum verified: sha256 $got"
}

fetch_binaries() {
	local version="$1" arch tmp missing
	tmp="$(mktemp -d)" || die "no temporary directory could be made. Is there room on this machine, and is TMPDIR writable?"
	# Not a RETURN trap: bash does not scope those to the function, so it would
	# fire again later with $tmp long gone and take `set -u` down with it.
	FETCH_TMP="$tmp"

	# A local bundle covers two cases that both matter: trying a build before it
	# is published, and installing on a machine that cannot reach GitHub.
	if [ -n "${CRAFTPANEL_BUNDLE:-}" ]; then
		[ -f "$CRAFTPANEL_BUNDLE" ] || die "CRAFTPANEL_BUNDLE is set but $CRAFTPANEL_BUNDLE is not there"
		step "installing from $CRAFTPANEL_BUNDLE"
		cp "$CRAFTPANEL_BUNDLE" "$tmp/bundle.tar.gz"
		if [ -f "$CRAFTPANEL_BUNDLE.sha256" ]; then
			cp "$CRAFTPANEL_BUNDLE.sha256" "$tmp/sum"
		fi
		missing="$CRAFTPANEL_BUNDLE has no $CRAFTPANEL_BUNDLE.sha256 beside it, so there is nothing to hold it against — and nothing has been installed. scripts/release.sh writes that file next to the bundle it builds; copy it along. To install this bundle unchecked all the same: CRAFTPANEL_NO_CHECKSUM=yes"
	else
		local base="https://github.com/$REPO/releases/download/v$version"
		# Only now, because a machine this installer has no bundle for can still be
		# served by one built on it — and that path does not need an architecture
		# this release knows.
		arch="$(detect_arch)"
		step "downloading CraftPanel $version for $arch"

		curl -fsSL "$base/craftpanel-linux-$arch.tar.gz" -o "$tmp/bundle.tar.gz" ||
			die "download failed — is craftpanel-linux-$arch.tar.gz attached to release v$version? https://github.com/$REPO/releases/tag/v$version says what is there"

		# Some curl versions leave the half-made output file behind when the request
		# turns out to be a 404, and an empty sum file would go on to fail the
		# comparison rather than say the far more useful "there is no sum here".
		curl -fsSL "$base/craftpanel-linux-$arch.tar.gz.sha256" -o "$tmp/sum" 2>/dev/null ||
			rm -f "$tmp/sum"
		missing="no checksum is published beside craftpanel-linux-$arch.tar.gz in release v$version, so there is nothing to hold the download against — and nothing has been installed. Every release publishes one, so this is an upload that broke off, or something between here and GitHub answering in its place. https://github.com/$REPO/releases/tag/v$version says what is really attached. To install the bundle unchecked all the same: CRAFTPANEL_NO_CHECKSUM=yes"
	fi

	verify_bundle "$tmp" "$missing"

	tar -xzf "$tmp/bundle.tar.gz" -C "$tmp"
	[ -e "$PREFIX/craftpanel" ] || remember "file:$PREFIX/craftpanel"
	[ -e "$PREFIX/craftpanel-helper" ] || remember "file:$PREFIX/craftpanel-helper"
	install -m 0755 "$tmp/craftpanel" "$PREFIX/craftpanel"
	install -m 0755 "$tmp/craftpanel-helper" "$PREFIX/craftpanel-helper"
	rm -rf "$tmp"
	FETCH_TMP=""
}

ensure_accounts() {
	local dir

	if ! getent group "$SERVICE_GROUP" >/dev/null; then
		groupadd --system "$SERVICE_GROUP"
		remember "group:$SERVICE_GROUP"
	fi
	if ! getent passwd "$SERVICE_USER" >/dev/null; then
		useradd --system \
			--gid "$SERVICE_GROUP" --home-dir "$DATA_DIR" --no-create-home \
			--shell /usr/sbin/nologin --comment "CraftPanel service account" "$SERVICE_USER"
		remember "user:$SERVICE_USER"
	fi

	# Asked before they are made, because `install -d` below does not say which
	# of the two it did, and a directory that was already here is somebody's data.
	for dir in "$DATA_DIR" "$DATA_DIR/users" "$DATA_DIR/cache" "$DATA_DIR/runtimes" "$CONFIG_DIR"; do
		[ -d "$dir" ] || remember "dir:$dir"
	done

	# The two hops above the accounts belong to root, and this is the whole reason
	# the helper can be sure what it is walking. Everything below users/<id> is the
	# managed account's own (2770), so it may rename any of it at any time; if the
	# panel owned users/ it could do the same one level higher — move an account
	# aside, put a link in its place, and have root's chown-tree land wherever it
	# chose. Root owning users/ is what nails those names down.
	#
	# $DATA_DIR itself cannot be closed the same way: panel.db lives in it, so the
	# panel has to be able to write there. The sticky bit is the answer — whoever
	# may write in the directory may still only rename or delete what is his own,
	# and users/ is root's. The panel keeps its database, its cache and its
	# backups; it does not keep the ability to move the accounts aside.
	#
	# 0751/1771 rather than anything tighter: a managed account shares no group
	# with the panel and has to walk through both hops to reach its own directory.
	# Traversable is enough — the listing stays closed and each account's own
	# directory is 2770, so knowing a path leads nowhere without owning it.
	#
	# `install -d` puts an existing directory right as well as making a new one,
	# which is how an installation from before this comes along on the next run.
	install -d -o root -g "$SERVICE_GROUP" -m 1771 "$DATA_DIR"
	install -d -o root -g "$SERVICE_GROUP" -m 0751 "$DATA_DIR/users"
	install -d -o "$SERVICE_USER" -g "$SERVICE_GROUP" -m 0750 "$DATA_DIR/cache"
	# The Java runtimes the panel fetches for itself, one directory per major
	# version (docs/JAVA.md). The panel owns it, because the panel is the only
	# thing that writes in there — and 0755 rather than 0750 like the cache above,
	# because a game server runs as its own managed account, shares no group with
	# the panel, and still has to execute runtimes/java-<major>/bin/java. Whether
	# anything is ever fetched into it is not decided here but in the panel
	# settings (java_auto_install, migration 0015); an installation with that
	# switch off keeps the directory and can be filled by hand.
	install -d -o "$SERVICE_USER" -g "$SERVICE_GROUP" -m 0755 "$DATA_DIR/runtimes"
	install -d -m 0755 "$CONFIG_DIR"

	# The service account exists by now, so a journal file that an earlier run
	# left behind as root can go back to it here as well.
	hand_database_back
}

# Whether there is anything to write is do_install's to decide and not this
# function's: do_install is where the questions are asked, and it says out loud
# that a file which is already there is kept and asks nothing. A second guard
# here would be a sentence nobody ever reads — the earlier one was, and it hid
# that the answers were being thrown away.
write_config() {
	local port="$1"
	remember "file:$CONFIG_DIR/config.toml"

	cat >"$CONFIG_DIR/config.toml" <<EOF
bind = "0.0.0.0:$port"
data_dir = "$DATA_DIR"
helper_socket = "$RUN_DIR/helper.sock"
EOF
	chown root:"$SERVICE_GROUP" "$CONFIG_DIR/config.toml"
	chmod 0640 "$CONFIG_DIR/config.toml"
}

write_units() {
	local unit
	for unit in /etc/systemd/system/craftpanel.service /etc/systemd/system/craftpanel-helper.service; do
		[ -e "$unit" ] || remember "unit:$unit"
	done

	cat >/etc/systemd/system/craftpanel-helper.service <<EOF
[Unit]
Description=CraftPanel privileged helper
Before=craftpanel.service

[Service]
Type=simple
ExecStart=$PREFIX/craftpanel-helper
Environment=CRAFTPANEL_HELPER_SOCKET=$RUN_DIR/helper.sock
Environment=CRAFTPANEL_USERS_ROOT=$DATA_DIR/users
Environment=CRAFTPANEL_SUPERVISOR=$PREFIX/craftpanel
Environment=CRAFTPANEL_GROUP=$SERVICE_GROUP
# Games live in a tree of their own, beside the two services rather than
# inside either: systemd kills a service's whole control group when it stops,
# so a tree under craftpanel.service would take every running server down with a
# panel update. It sits under system.slice because the kernel weighs a move
# against the common ancestor of the two groups, and only there is that
# ancestor one this host lets root write. The panel reads the same tree for
# what an account is using, which is why both units name it.
Environment=CRAFTPANEL_CGROUP_ROOT=$CGROUP_ROOT
# The panel runs unprivileged but has to create its own supervisor socket in
# here, so the directory carries the shared group and is group-writable.
Group=$SERVICE_GROUP
RuntimeDirectory=craftpanel
RuntimeDirectoryMode=0771
RuntimeDirectoryPreserve=yes
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF

	cat >/etc/systemd/system/craftpanel.service <<EOF
[Unit]
Description=CraftPanel
After=network.target craftpanel-helper.service
Requires=craftpanel-helper.service

[Service]
Type=simple
User=$SERVICE_USER
Group=$SERVICE_GROUP
ExecStart=$PREFIX/craftpanel serve
Environment=CRAFTPANEL_CONFIG=$CONFIG_DIR/config.toml
Environment=CRAFTPANEL_CGROUP_ROOT=$CGROUP_ROOT
Delegate=yes
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
# The panel opens its own socket for supervisors to report back on, and
# ProtectSystem=strict would otherwise hand it a read-only /run.
ReadWritePaths=$DATA_DIR $RUN_DIR
AmbientCapabilities=
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF

	systemctl daemon-reload
}

# ------------------------------------------------------------------ the games
#
# Both paths that take something away have to get the running servers down
# first: the move to the new name below, and the uninstall further down. It is
# written here once, above both of them, and the uninstall reaches into nothing
# that belongs to the move — that block goes away the day nobody carries the old
# name any more, and the uninstall must not go with it.

STOP_GRACE="${CRAFTPANEL_STOP_GRACE:-300}"

# What this installer put on the machine. Everything else in the games' tree is
# a game; the move adds the two binaries of the old name to the list.
PANEL_BINARIES=("$PREFIX/craftpanel" "$PREFIX/craftpanel-helper")

# What stop_games gave up on, for the caller to name. Empty after a run that got
# everything down.
STUCK_GAMES=()

# Every process in the games' tree, one pid per line. A cgroup file always
# measures zero bytes, so what is in it can only be found by reading it.
cgroup_pids() {
	local root="$1" roll
	[ -d "$root" ] || return 0

	while IFS= read -r roll; do
		cat "$roll" 2>/dev/null || true
	done < <(find "$root" -name cgroup.procs 2>/dev/null || true)
}

# Everything in that tree that is not a supervisor is a game. The supervisor is in
# there with its child (it joins the group before it execs), and telling the two
# apart is what keeps the SIGTERM below off the wrong process.
game_processes() {
	local root="$1" pid exe ours

	while read -r pid; do
		[ -d "/proc/$pid" ] || continue
		exe="$(readlink "/proc/$pid/exe" 2>/dev/null || true)"
		exe="${exe% (deleted)}"
		for ours in "${PANEL_BINARIES[@]}"; do
			if [ "$exe" = "$ours" ]; then
				continue 2
			fi
		done
		printf '%s\n' "$pid"
	done < <(cgroup_pids "$root")
}

# Which server a process belongs to, asked of the process itself: a game runs in
# its own directory, users/<owner>/servers/<server>, and the step after servers/
# is the id the panel knows it by (files/mod.rs, server_dir). Nobody should have
# to decide about a bare pid.
game_label() {
	local pid="$1" cwd id
	cwd="$(readlink "/proc/$pid/cwd" 2>/dev/null || true)"

	case "$cwd" in
		*/servers/*)
			id="${cwd##*/servers/}"
			printf 'server %s' "${id%%/*}"
			;;
		*) printf 'no server directory to read' ;;
	esac
}

# SIGTERM to the game's process group, and never anything harder: what saves the
# world is the shutdown hook the game runs on that signal, and a SIGKILL in the
# middle of it is the one thing this function exists to avoid. The panel is
# still up while this runs, so every supervisor can hand its last console lines
# over and end by itself instead of waiting for a panel that is already gone.
#
# What outsits the grace is left in STUCK_GAMES; what to do about it is the
# caller's to say, because the two callers disagree. The move gives up, and the
# uninstall asks — somebody who is removing the panel wants it gone.
stop_games() {
	local root="$1" pids=() pid waited=0
	STUCK_GAMES=()

	mapfile -t pids < <(game_processes "$root")
	if [ "${#pids[@]}" -eq 0 ]; then
		say "  no Minecraft server is running"
		return 0
	fi

	step "asking $(plural "${#pids[@]}" "running game process" "running game processes") to save and stop"
	for pid in "${pids[@]}"; do
		kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
	done

	while [ "$waited" -lt "$STOP_GRACE" ]; do
		mapfile -t pids < <(game_processes "$root")
		if [ "${#pids[@]}" -eq 0 ]; then
			say "  every server has saved and stopped"
			return 0
		fi
		sleep 2
		waited=$((waited + 2))
	done

	STUCK_GAMES=("${pids[@]}")
	return 1
}

# The games are down by the time this runs, so whatever is still in the tree is
# a supervisor that could not reach the panel, and it holds no world open. The
# last rmdir is the answer: it only goes through on an empty tree, which is why
# both callers ask about it rather than assume.
take_down_cgroup() {
	local root="$1" pid waited=0
	[ -d "$root" ] || return 0
	step "taking down $root"

	while read -r pid; do
		kill -TERM "$pid" 2>/dev/null || true
	done < <(cgroup_pids "$root")

	while [ "$waited" -lt 30 ] && [ -n "$(cgroup_pids "$root")" ]; do
		sleep 1
		waited=$((waited + 1))
	done

	find "$root" -mindepth 1 -depth -type d -exec rmdir {} + 2>/dev/null || true
	rmdir "$root" 2>/dev/null
}

# ------------------------------------------------------------- the old name
#
# A machine set up before the rename carries mcpanel in eight places: the data
# directory, the config, the two units with their drop-ins, the service account,
# the group, one system account per panel user, and the control group the games
# run in. Everything from here to do_upgrade moves such an installation across.
#
# Every step asks first whether it still has anything to do, so a second run
# finds nothing and changes nothing, and a run that broke off halfway carries on
# where it stopped.

OLD_NAME="mcpanel"
OLD_DISPLAY="MCPanel"
OLD_ENV_PREFIX="MCPANEL_"
OLD_ACCOUNT_PREFIX="mcp-"
OLD_CONFIG_DIR="/etc/$OLD_NAME"
OLD_DATA_DIR="/var/lib/$OLD_NAME"
OLD_RUN_DIR="/run/$OLD_NAME"
OLD_CGROUP_ROOT="/sys/fs/cgroup/system.slice/$OLD_NAME-games"
OLD_KEEP="upgrade-from-$OLD_NAME"
SQL_RUNNER=""
PANEL_BINARIES+=("$PREFIX/$OLD_NAME" "$PREFIX/$OLD_NAME-helper")

old_installation() {
	local marker
	for marker in \
		"/etc/systemd/system/$OLD_NAME.service" \
		"/etc/systemd/system/$OLD_NAME-helper.service" \
		"$OLD_CONFIG_DIR/config.toml" \
		"$OLD_DATA_DIR" \
		"$OLD_CGROUP_ROOT" \
		"$PREFIX/$OLD_NAME" \
		"$PREFIX/$OLD_NAME-helper"
	do
		if [ -e "$marker" ]; then
			return 0
		fi
	done

	getent passwd "$OLD_NAME" >/dev/null && return 0
	getent group "$OLD_NAME" >/dev/null && return 0
	[ -n "$(old_accounts)" ] && return 0
	[ -n "$(old_groups)" ] && return 0
	return 1
}

# name:home per line. The id in the account name is the panel id in lower case
# while the directory keeps its own spelling, so the new home is the old one with
# the data directory swapped — never one built from the name.
old_accounts() {
	getent passwd | awk -F: -v prefix="$OLD_ACCOUNT_PREFIX" \
		'index($1, prefix) == 1 { print $1 ":" $6 }'
}

old_groups() {
	getent group | awk -F: -v prefix="$OLD_ACCOUNT_PREFIX" 'index($1, prefix) == 1 { print $1 }'
}

renamed() {
	printf 'craft-%s' "${1#"$OLD_ACCOUNT_PREFIX"}"
}

# Everything that could make the move stop in the middle is asked here, before a
# single service is stopped: a missing tool, a name that is already taken, a data
# directory on a mount of its own. Failing in this function costs nothing.
upgrade_preflight() {
	local tool entry name

	for tool in usermod groupmod; do
		command -v "$tool" >/dev/null || die "the move needs $tool (shadow-utils). Nothing has been touched."
	done

	if [ -f "$OLD_DATA_DIR/panel.db" ] || [ -f "$DATA_DIR/panel.db" ]; then
		pick_sql_runner
		[ -n "$SQL_RUNNER" ] ||
			die "the move has to write in panel.db and found neither sqlite3 nor python3. Install one of them (apt install sqlite3) and run this again — nothing has been touched."
	fi

	# An empty $DATA_DIR is what ensure_accounts leaves behind when an earlier run
	# of this installer got that far and no further; it may go. One with anything
	# else in it is a second installation, and picking between two is not a
	# decision an installer should make on its own.
	if [ -d "$OLD_DATA_DIR" ] && [ -d "$DATA_DIR" ]; then
		rmdir "$DATA_DIR/cache" "$DATA_DIR/users" "$DATA_DIR" 2>/dev/null || true
		if [ -e "$DATA_DIR" ]; then
			die "$OLD_DATA_DIR and $DATA_DIR both hold data. Move or remove one of them by hand — see docs/UPGRADE.md."
		fi
	fi

	# A rename inside one directory is what the move of the data is, and a mount of
	# its own turns that into a copy of every world on the machine.
	if [ -d "$OLD_DATA_DIR" ] &&
		[ "$(stat -c %d "$OLD_DATA_DIR")" != "$(stat -c %d "$(dirname "$OLD_DATA_DIR")")" ]; then
		die "$OLD_DATA_DIR is a filesystem of its own, so it cannot be renamed. Mount it at $DATA_DIR instead and run this again."
	fi

	if getent passwd "$SERVICE_USER" >/dev/null && getent passwd "$OLD_NAME" >/dev/null; then
		die "both $OLD_NAME and $SERVICE_USER exist as system accounts. Decide which one owns the files and remove the other — see docs/UPGRADE.md."
	fi

	if getent group "$SERVICE_GROUP" >/dev/null && getent group "$OLD_NAME" >/dev/null; then
		die "both $OLD_NAME and $SERVICE_GROUP exist as groups. Decide which one owns the files and remove the other — see docs/UPGRADE.md."
	fi

	while IFS= read -r entry; do
		[ -n "$entry" ] || continue
		name="$(renamed "${entry%%:*}")"
		if getent passwd "$name" >/dev/null; then
			die "${entry%%:*} should become $name, and $name already exists. Nothing has been touched."
		fi
	done < <(old_accounts)

	while IFS= read -r entry; do
		[ -n "$entry" ] || continue
		name="$(renamed "$entry")"
		if getent group "$name" >/dev/null; then
			die "the group $entry should become $name, and $name already exists. Nothing has been touched."
		fi
	done < <(old_groups)
}

stop_old_services() {
	local unit
	for unit in "$OLD_NAME.service" "$OLD_NAME-helper.service"; do
		if [ ! -f "/etc/systemd/system/$unit" ] && ! systemctl is-active --quiet "$unit"; then
			continue
		fi
		step "stopping $unit"
		systemctl disable --now "$unit" >/dev/null 2>&1 || true
		if systemctl is-active --quiet "$unit"; then
			die "$unit would not stop. Nothing has been moved. See: systemctl status $unit"
		fi
	done
}

# A cgroup directory cannot be renamed — the kernel answers EPERM to rename(2) on
# cgroup2 — so the old tree is taken down instead. Nothing is lost with it: the
# panel writes every ready account's ceilings back at start (auth/users.rs,
# rewrite_limits), and the helper makes the directory it needs on the way to a
# spawn. Both use the new root because both units name it.
move_cgroup() {
	take_down_cgroup "$OLD_CGROUP_ROOT" ||
		die "$OLD_CGROUP_ROOT still holds processes. See who: cat $OLD_CGROUP_ROOT/*/cgroup.procs"
}

move_data() {
	[ -d "$OLD_DATA_DIR" ] || return 0
	step "moving $OLD_DATA_DIR → $DATA_DIR"
	mv -T -- "$OLD_DATA_DIR" "$DATA_DIR" || die "moving the data directory failed. Nothing else has been changed."
}

# usermod -l and groupmod -n and nothing else: uid and gid stay as they are, and
# so every file in the tree that just moved keeps its owner without a single
# chown. -d without -m for the same reason — the directory is already where the
# new entry says it is.
rename_service_account() {
	local args

	if getent group "$OLD_NAME" >/dev/null; then
		step "renaming the group $OLD_NAME → $SERVICE_GROUP (gid unchanged)"
		groupmod -n "$SERVICE_GROUP" "$OLD_NAME" || die "renaming the group $OLD_NAME failed."
	fi

	if getent passwd "$OLD_NAME" >/dev/null; then
		step "renaming the account $OLD_NAME → $SERVICE_USER (uid unchanged)"
		args=(-l "$SERVICE_USER" -d "$DATA_DIR")
		if [ "$(getent passwd "$OLD_NAME" | cut -d: -f5)" = "$OLD_NAME service account" ]; then
			args+=(-c "CraftPanel service account")
		fi
		usermod "${args[@]}" "$OLD_NAME" ||
			die "renaming the account $OLD_NAME failed — is something still running as $OLD_NAME? Check with: ps -u $OLD_NAME"
	fi
}

rename_managed_accounts() {
	local entries=() groups=() entry name home new comment args
	mapfile -t entries < <(old_accounts)
	mapfile -t groups < <(old_groups)

	if [ "${#entries[@]}" -gt 0 ]; then
		step "renaming ${#entries[@]} managed account(s) $OLD_ACCOUNT_PREFIX… → craft-…"
	fi

	for entry in "${entries[@]}"; do
		name="${entry%%:*}"
		home="${entry#*:}"
		new="$(renamed "$name")"
		case "$home" in
			"$OLD_DATA_DIR"/*) home="$DATA_DIR${home#"$OLD_DATA_DIR"}" ;;
		esac

		args=(-l "$new" -d "$home")
		comment="$(getent passwd "$name" | cut -d: -f5)" || comment=""
		if [ "$comment" = "$OLD_NAME managed account" ]; then
			args+=(-c "craftpanel managed account")
		fi

		usermod "${args[@]}" "$name" ||
			die "renaming $name to $new failed. Check with: ps -u $name"
	done

	# useradd made a group of the same name beside each account, and that group is
	# what a game process runs with.
	for entry in "${groups[@]}"; do
		getent group "$entry" >/dev/null || continue
		new="$(renamed "$entry")"
		groupmod -n "$new" "$entry" || die "renaming the group $entry to $new failed."
	done
}

# One rule for every file that came from the old installation: the token changes,
# the setting does not. It catches the paths as well, because the only thing that
# differs between /var/lib/mcpanel and /var/lib/craftpanel is the name.
retune() {
	local file="$1"
	sed -i \
		-e "s/$OLD_ENV_PREFIX/CRAFTPANEL_/g" \
		-e "s/$OLD_DISPLAY/CraftPanel/g" \
		-e "s/$OLD_NAME/craftpanel/g" \
		-e "s/$OLD_ACCOUNT_PREFIX/craft-/g" \
		"$file" || die "rewriting $file failed."
}

move_config() {
	install -d -m 0755 "$CONFIG_DIR"

	if [ -f "$OLD_CONFIG_DIR/config.toml" ]; then
		if [ -f "$CONFIG_DIR/config.toml" ]; then
			warn "$CONFIG_DIR/config.toml already exists — $OLD_CONFIG_DIR/config.toml is left for you to compare"
		else
			step "moving the configuration"
			mv -- "$OLD_CONFIG_DIR/config.toml" "$CONFIG_DIR/config.toml" ||
				die "moving $OLD_CONFIG_DIR/config.toml failed."
			retune "$CONFIG_DIR/config.toml"
		fi
	fi

	if [ -f "$CONFIG_DIR/config.toml" ]; then
		chown root:"$SERVICE_GROUP" "$CONFIG_DIR/config.toml"
		chmod 0640 "$CONFIG_DIR/config.toml"
	fi
	rmdir "$OLD_CONFIG_DIR" 2>/dev/null || true
}

# The units themselves are written fresh by write_units — they are the
# installer's own and nothing is lost by making them again. The drop-ins are not:
# those are the operator's, so they move over one by one, keeping their order and
# their content bar the name.
move_units() {
	local old new
	for old in "$OLD_NAME" "$OLD_NAME-helper"; do
		new="craftpanel${old#"$OLD_NAME"}"
		move_dropins "$old.service.d" "$new.service.d"

		if [ -f "/etc/systemd/system/$old.service" ]; then
			install -d -o root -g root -m 0700 "$DATA_DIR/$OLD_KEEP"
			mv -- "/etc/systemd/system/$old.service" "$DATA_DIR/$OLD_KEEP/$old.service" ||
				die "moving /etc/systemd/system/$old.service aside failed."
		fi
	done

	rm -f "/etc/systemd/system/multi-user.target.wants/$OLD_NAME.service" \
		"/etc/systemd/system/multi-user.target.wants/$OLD_NAME-helper.service"
	write_units
}

move_dropins() {
	local from="/etc/systemd/system/$1" to="/etc/systemd/system/$2" conf name
	[ -d "$from" ] || return 0
	install -d -m 0755 "$to"

	for conf in "$from"/*; do
		[ -e "$conf" ] || continue
		name="$(basename "$conf")"
		name="${name//"$OLD_NAME"/craftpanel}"
		if [ -e "$to/$name" ]; then
			warn "$to/$name already exists — $conf stays where it is"
			continue
		fi
		step "moving the drop-in $conf → $to/$name"
		mv -- "$conf" "$to/$name" || die "moving $conf failed."
		case "$name" in
			*.conf) retune "$to/$name" ;;
		esac
	done

	rmdir "$from" 2>/dev/null || warn "$from is not empty and stays behind"
}

# No column holds a system account name: the panel works it out from the panel id
# every time it needs it (craftpanel-proto, system_username), so the accounts
# renamed above are already the ones the panel will look for. What the database
# does keep in plain text is the helper's last complaint about an account, and
# that sentence carries the old name and the old path around with it — along with
# two settings whose default was written before the rename.
patch_database() {
	local db="$DATA_DIR/panel.db"
	[ -f "$db" ] || return 0
	[ -n "$SQL_RUNNER" ] || pick_sql_runner
	[ -n "$SQL_RUNNER" ] || die "panel.db needs sqlite3 or python3, and neither is here. Everything else has moved, see docs/UPGRADE.md."
	step "putting the old name right in panel.db"

	run_sql "$db" "UPDATE users \
		SET system_error_message = replace(replace(system_error_message, '$OLD_NAME', 'craftpanel'), '$OLD_ACCOUNT_PREFIX', 'craft-') \
		WHERE system_error_message LIKE '%$OLD_NAME%' OR system_error_message LIKE '%$OLD_ACCOUNT_PREFIX%'"

	if has_table "$db" mail_settings; then
		run_sql "$db" "UPDATE mail_settings SET from_name = 'CraftPanel' WHERE from_name = '$OLD_NAME'"
	fi

	# Only the untouched default. A folder that already exists in somebody's Drive
	# is found by the id stored beside the account, so it keeps its name and its
	# backups either way (drive/mod.rs, folder).
	if has_table "$db" drive_settings; then
		run_sql "$db" "UPDATE drive_settings SET folder_name = 'craftpanel-backups' WHERE folder_name = '$OLD_NAME-backups'"
	fi

	hand_database_back
}

# Opening panel.db as root can leave the journal beside it owned by root, and the
# panel does not run as root. SQLite takes those two files away again when the
# last connection closes, so this is for the run that does not get that far —
# and every path that opens the database goes through here afterwards: the
# counting in look_at_kept_data as much as the writing above. Where there is no
# service account yet the files stay as they are and ensure_accounts comes back
# to them, because a chown to a name that does not exist puts nothing right.
hand_database_back() {
	local db="$DATA_DIR/panel.db"
	[ -f "$db" ] || return 0
	getent passwd "$SERVICE_USER" >/dev/null || return 0

	chown "$SERVICE_USER:$SERVICE_GROUP" "$db" "$db-wal" "$db-shm" 2>/dev/null || true
	return 0
}

# sqlite3 is not on every machine, and python3 carries the same library on nearly
# all of them. One of the two is asked for in the preflight, long before anything
# moves.
pick_sql_runner() {
	if command -v sqlite3 >/dev/null; then
		SQL_RUNNER="sqlite3"
	elif command -v python3 >/dev/null && python3 -c 'import sqlite3' 2>/dev/null; then
		SQL_RUNNER="python3"
	fi
}

has_table() {
	local db="$1" name="$2" found
	found="$(sql_value "$db" "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = '$name'")" ||
		die "reading $db failed."
	[ "$found" = "1" ]
}

sql_value() {
	case "$SQL_RUNNER" in
		sqlite3) sqlite3 "$1" "$2" ;;
		python3) python3 -c 'import sqlite3, sys; print(sqlite3.connect(sys.argv[1]).execute(sys.argv[2]).fetchone()[0])' "$1" "$2" ;;
		*) return 1 ;;
	esac
}

run_sql() {
	local db="$1" statement="$2" worked=1
	case "$SQL_RUNNER" in
		sqlite3) sqlite3 "$db" "$statement" >/dev/null && worked=0 ;;
		python3) python3 -c 'import sqlite3, sys; c = sqlite3.connect(sys.argv[1]); c.execute(sys.argv[2]); c.commit()' "$db" "$statement" && worked=0 ;;
	esac
	[ "$worked" -eq 0 ] ||
		die "this statement did not go through: $statement — everything else has moved, see docs/UPGRADE.md."
}

# One machine can carry both: the installation people use and a second one put
# there to try something. So the move happens only when somebody has said so —
# CRAFTPANEL_UPGRADE=yes, or a typed yes at the terminal. Enter means no. It stops
# other people's Minecraft servers and takes their installation's name away, and
# nothing that does that may be one keystroke away from happening by accident.
#
# The question is also not worth asking once CraftPanel is installed here: two
# accounts of the same purpose cannot both own the files, so upgrade_preflight
# refuses that move anyway. Asking every update run would be asking for an answer
# that could not be carried out.
wants_upgrade() {
	local installed="${1:-}" answer

	say
	say "${BOLD}An installation under the old name $OLD_NAME is on this machine.${OFF}"

	if [ -n "${CRAFTPANEL_UPGRADE:-}" ]; then
		case "${CRAFTPANEL_UPGRADE,,}" in
			y*|j*) return 0 ;;
		esac
		say "${DIM}CRAFTPANEL_UPGRADE=$CRAFTPANEL_UPGRADE says not to move it, so it is left alone.${OFF}"
		return 1
	fi

	if [ -n "$installed" ] || kept_data; then
		say "${DIM}CraftPanel is installed here too, and one machine cannot have both under"
		say "one set of names — so $OLD_NAME is left alone and this run does not ask."
		say "docs/UPGRADE.md says how to move it by hand.${OFF}"
		return 1
	fi

	say "Moving it would ${BOLD}stop every Minecraft server it is running${OFF} — they are told to"
	say "save and shut down, and they stay stopped afterwards — and would then ${BOLD}rename"
	say "that installation${OFF}: data directory, configuration, both services and every"
	say "account, from $OLD_NAME to craftpanel. Worlds, database, uids and passwords are"
	say "kept, but nothing is called $OLD_NAME any more."
	say "${DIM}Answering no leaves it running and installs CraftPanel beside it.${OFF}"
	say

	if [ -n "${CRAFTPANEL_NONINTERACTIVE:-}" ] || [ ! -r /dev/tty ]; then
		say "This run answers no questions, so $OLD_NAME is left alone."
		say "${DIM}Run it again with CRAFTPANEL_UPGRADE=yes to move it.${OFF}"
		return 1
	fi

	read -r -p "Stop those servers and rename that installation? [y/N]: " answer </dev/tty || true
	case "${answer,,}" in
		y*|j*) return 0 ;;
	esac
	say "Leaving $OLD_NAME where it is."
	say "${DIM}To move it later: CRAFTPANEL_UPGRADE=yes bash install.sh${OFF}"
	return 1
}

do_upgrade() {
	local version="$1"
	say
	upgrade_preflight
	# The data has not moved yet, so the line to read is the one beside it under
	# the old name. This was the only way in that wrote a version stamp at the
	# end without ever having read one, which made the move the single path on
	# which a step backwards was neither refused nor noticed.
	refuse_to_go_backwards "$version" "$OLD_DATA_DIR"
	# Before a single service goes down: a download that fails here leaves an
	# installation that is still running.
	fetch_binaries "$version"

	stop_games "$OLD_CGROUP_ROOT" ||
		die "still running after ${STOP_GRACE}s: ${STUCK_GAMES[*]}. Nothing has been moved. Stop those servers from the panel and run this again, or give them longer with CRAFTPANEL_STOP_GRACE=<seconds>."
	stop_old_services
	move_cgroup
	move_data
	rename_service_account
	rename_managed_accounts
	move_config
	move_units
	patch_database
	ensure_accounts

	rm -rf "$OLD_RUN_DIR"
	rm -f "$PREFIX/$OLD_NAME" "$PREFIX/$OLD_NAME-helper"

	step "starting CraftPanel"
	systemctl enable --now craftpanel-helper.service >/dev/null 2>&1 || true
	systemctl enable --now craftpanel.service >/dev/null 2>&1 || true

	sleep 2
	if ! systemctl is-active --quiet craftpanel.service; then
		die "everything was moved, but CraftPanel did not start. See: journalctl -u craftpanel -n 50"
	fi
	version_stamp "$version"

	say
	say "${GREEN}${BOLD}CraftPanel is running under its new name.${OFF}"
	say
	say "  Data      $DATA_DIR"
	say "  Config    $CONFIG_DIR/config.toml"
	say "  Services  craftpanel.service, craftpanel-helper.service"
	say "  Accounts  $SERVICE_USER, and craft-… for every panel user"
	say
	say "${DIM}Your usernames and passwords are unchanged."
	say "The Minecraft servers are stopped — start them again from the panel."
	say "The old unit files are kept in $DATA_DIR/$OLD_KEEP; docs/UPGRADE.md says what happened.${OFF}"
	say
}

# ------------------------------------------------- installing over what stayed
#
# Uninstalling asks whether the data should go too, and "no" is meant to be a
# decision one can come back from: install again later and carry on with the
# same worlds, the same accounts and the same passwords (keep_data says as
# much). What is on the machine at that point is $DATA_DIR with panel.db, the
# backups, the playit keys, the Drive tokens and the fetched runtimes;
# $CONFIG_DIR/config.toml; and every craft-… account with the uid the database
# has written down. What is gone is the two units and the two programs — which
# is why installed_version() cannot answer this question: it asks a program that
# has just been deleted.
#
# So the data itself is the marker. The service account is not one: an install
# that breaks off while downloading leaves that behind as well, with no database
# anywhere, and the next run would take a half-made installation for a kept one.

kept_data() {
	[ -s "$DATA_DIR/panel.db" ] || [ -f "$CONFIG_DIR/config.toml" ]
}

# A value out of config.toml, read where the panel reads it: a plain key stands
# at the top of the file, the keys of a table stand under its header. A key that
# is not in the file is not answered here either, and the caller falls back to the
# same default the panel itself would use (config.rs).
#
# This is not a TOML parser and must never be taken for one. It knows four
# shapes — a table header, a comment, a number, a string in either kind of quote
# — because that is what the installer writes and what a hand-edited file looks
# like. Anything else it answers with nothing at all, which the callers can
# handle; the one thing it must not do is answer wrongly. It did once: a comment
# behind the header turned "[ports] # the game ports" into the table
# "ports#thegameports", the range fell back to 25565-25700 without a word, and
# the panel went for ports that another installation on the same machine held.
config_value() {
	local key="$1" wanted="${2:-}"
	[ -f "$CONFIG_DIR/config.toml" ] || return 0

	awk -v key="$key" -v wanted="$wanted" -v quote="'" '
		/^[[:space:]]*\[/ {
			table = $0
			sub(/#.*/, "", table)
			gsub(/[][[:space:]"]/, "", table)
			next
		}
		table == wanted && $0 ~ "^[[:space:]]*\"?" key "\"?[[:space:]]*=" {
			sub(/^[^=]*=[[:space:]]*/, "")
			if (match($0, /^"[^"]*"/)) { print substr($0, 2, RLENGTH - 2); next }
			if (match($0, "^" quote "[^" quote "]*" quote)) { print substr($0, 2, RLENGTH - 2); next }
			if (match($0, /^[0-9]+/)) { print substr($0, 1, RLENGTH); next }
		}
	' "$CONFIG_DIR/config.toml" 2>/dev/null | tail -1 || true
}

# Two numbers, typed at a terminal, and the next thing that happens to them is a
# download. What the panel refuses when it writes them (auth/settings.rs, check)
# is refused here as well, while nothing has been fetched or made yet.
sensible_port_range() {
	local from="$1" to="$2"

	case "$from" in "" | *[!0-9]*) die "\"$from\" is not a port number." ;; esac
	case "$to" in "" | *[!0-9]*) die "\"$to\" is not a port number." ;; esac
	if [ "$from" -lt 1024 ] || [ "$to" -gt 65535 ] || [ "$from" -gt "$to" ]; then
		die "the port range runs upwards and stays between 1024 and 65535, and $from to $to does not."
	fi
}

# The range the servers' own ports come out of is not in config.toml and never
# was read from there: it is a setting of the panel, one row in panel.db
# (panel_settings, migration 0002), and the panel takes it from there every time
# it hands a port out (servers/manager.rs, next_free_port). So an installation
# that is already here is asked, not told.
read_port_pool() {
	local db="$DATA_DIR/panel.db"
	POOL_FROM=""
	POOL_TO=""

	[ -s "$db" ] || return 0
	[ -n "$SQL_RUNNER" ] || pick_sql_runner
	[ -n "$SQL_RUNNER" ] || return 0
	has_table "$db" panel_settings || return 0

	POOL_FROM="$(sql_value "$db" 'SELECT port_pool_from FROM panel_settings WHERE id = 1' 2>/dev/null || true)"
	POOL_TO="$(sql_value "$db" 'SELECT port_pool_to FROM panel_settings WHERE id = 1' 2>/dev/null || true)"
}

# Migrations only ever run forwards. Data that a newer panel has already migrated
# carries versions an older build does not know, and sqlx then refuses to open
# the database at all rather than guess: the service would not come up, `admin
# create` would fail the same way, and the sentence the operator gets talks about
# migrations instead of about what to do. Nothing in panel.db says which version
# wrote it, so the installer leaves a line of its own beside the data and reads it
# back before it fetches anything.
version_stamp() {
	[ -d "$DATA_DIR" ] || return 0

	# A line for the next run, and never worth more than the run it belongs to:
	# this is written after the panel is up, where a full disk must not turn a
	# finished installation into a script that ends without a word.
	if ! printf '%s\n' "$1" >"$DATA_DIR/installed-version" 2>/dev/null; then
		warn "could not write $DATA_DIR/installed-version — the next run will not be able to tell which version this data belongs to"
		return 0
	fi
	chmod 0644 "$DATA_DIR/installed-version" || true
}

# The directory is given rather than assumed: on the move from the old name the
# data still lies in $OLD_DATA_DIR when the question has to be answered, and
# afterwards it is too late to refuse anything.
refuse_to_go_backwards() {
	local wanted="$1" where="${2:-$DATA_DIR}" last
	[ -f "$where/installed-version" ] || return 0
	last="$(head -1 "$where/installed-version" | tr -d '[:space:]')" || last=""
	[ -n "$last" ] || return 0

	# Two release numbers can be put in order; "local", or the "test" of a bundle
	# built by hand, cannot — and saying nothing beats saying something wrong.
	case "$last$wanted" in *[!0-9.]*) return 0 ;; esac
	[ "$(printf '%s\n%s\n' "$last" "$wanted" | sort -V | tail -1)" = "$wanted" ] && return 0

	die "the data in $where was last used by CraftPanel $last and this is $wanted. Database migrations only run forwards, so $wanted would refuse to open panel.db and the panel would not start. Install $last or newer, or move $where aside first."
}

# Counted out of the database, not claimed, with the two readers the move under
# the old name already uses. A machine that has neither is told so instead of
# being guessed at: an empty FOUND_ADMINS means "not known", and on a "not known"
# nothing is made and nothing is deleted.
FOUND_USERS=""
FOUND_ADMINS="0"
POOL_FROM=""
POOL_TO=""
FOUND_ADMIN_NAMES=""
FOUND_SERVERS=""
FOUND_READY=""

look_at_kept_data() {
	local db="$DATA_DIR/panel.db"
	if [ ! -s "$db" ]; then
		FOUND_USERS=0
		FOUND_SERVERS=0
		return 0
	fi

	pick_sql_runner
	if [ -z "$SQL_RUNNER" ]; then
		FOUND_ADMINS=""
		warn "panel.db is here, but reading it needs sqlite3 or python3 and neither is on this machine. Nothing in it is touched: no account is made, none is deleted, and the counts below stay empty. Install sqlite3 and run this again to see them."
		return 0
	fi

	has_table "$db" users || return 0

	# Every count is asked for in a way that cannot end the run: a database that
	# is locked by something else, or damaged, is a reason to say nothing about
	# it and carry on with the data untouched — not a reason to stop between two
	# questions with no sentence on the screen.
	if ! FOUND_USERS="$(sql_value "$db" 'SELECT count(*) FROM users')"; then
		FOUND_USERS=""
		FOUND_ADMINS=""
		warn "panel.db is here and could not be read — is something else holding it open? Nothing in it is touched: no account is made, none is deleted, and the counts below stay empty."
		return 0
	fi
	FOUND_ADMINS="$(sql_value "$db" "SELECT count(*) FROM users WHERE role = 'admin'")" || FOUND_ADMINS=""
	FOUND_READY="$(sql_value "$db" "SELECT count(*) FROM users WHERE system_state = 'ready'")" || FOUND_READY=""
	if [ -n "$FOUND_ADMINS" ] && [ "$FOUND_ADMINS" -gt 0 ]; then
		FOUND_ADMIN_NAMES="$(sql_value "$db" "SELECT group_concat(username, ', ') FROM (SELECT username FROM users WHERE role = 'admin' ORDER BY username LIMIT 6)")" ||
			FOUND_ADMIN_NAMES=""
		[ "$FOUND_ADMINS" -gt 6 ] &&
			FOUND_ADMIN_NAMES="$FOUND_ADMIN_NAMES and $((FOUND_ADMINS - 6)) more"
	fi
	if has_table "$db" servers; then
		FOUND_SERVERS="$(sql_value "$db" 'SELECT count(*) FROM servers')" || FOUND_SERVERS=""
	fi

	# Read as root, and the journal beside it belongs to the panel again.
	hand_database_back
}

# The database keeps a uid, but nothing is ever done through that number: panel
# and helper look an account up by its name (craft-<id>) every single time, so a
# kept account fits back in as it stands, whatever uid it carries. The one thing
# nobody looks at again is an account the database calls ready whose craft-… user
# is gone — at start the panel finishes the ones it left `provisioning` and writes
# the limits for the ready ones, and asks after none of them. That shows itself
# first when a server is started, so it is worth a word here.
say_if_accounts_are_missing() {
	local here
	[ -n "$FOUND_READY" ] || return 0
	here="$(panel_accounts | wc -l)" || here=0
	[ "$FOUND_READY" -gt "$here" ] || return 0

	warn "the database calls $(plural "$FOUND_READY" account) ready, and $here of them are on this machine. The panel does not look at that again when it starts, and a server whose owner is missing will not start. \"UPDATE users SET system_state = 'provisioning'\" on those rows has the panel make the accounts again on its next start."
}

do_install() {
	local version="$1"

	# From here down to installation_stands, everything this run makes is written
	# down as it is made. A run that does not reach the end takes its own
	# leavings with it and leaves everything else exactly as it found it.
	UNDO_READY=yes

	say
	say "${BOLD}Installing CraftPanel${OFF}"
	say "${DIM}A panel for running Minecraft servers on this machine.${OFF}"
	say

	local kept=""
	if kept_data; then
		kept=yes
		# Both before anything is fetched or written: a step backwards is refused
		# while the machine is still untouched, and what is here is counted before
		# the first question, so no question is asked that the data answers.
		refuse_to_go_backwards "$version"

		say "${BOLD}The data of an earlier installation is still here.${OFF}"
		say "${DIM}It is taken up as it stands — worlds, database, backups, keys, runtimes and"
		say "every craft-… account with the uid it has. Nothing is deleted.${OFF}"
		say
		look_at_kept_data
		local found=""
		[ -n "$FOUND_SERVERS" ] && found="$(plural "$FOUND_SERVERS" server)"
		[ -n "$FOUND_USERS" ] &&
			found="${found:+$found, }$(plural "$FOUND_USERS" "panel account"), $(plural "$FOUND_ADMINS" administrator) among them"
		[ -n "$found" ] && say "  Database  $found"
		say "  Data      $DATA_DIR${DIM}$([ -d "$DATA_DIR" ] && printf ', %s' "$(size_of "$DATA_DIR")")${OFF}"
		say_if_accounts_are_missing
		say
	fi

	local port pool_start="" pool_end="" set_pool="" bind keeping_config=""
	if [ -f "$CONFIG_DIR/config.toml" ]; then
		keeping_config=yes
		# Asked and applied, or not asked at all — and here it is the file that
		# decides. Writing it again would throw away every line somebody put in it
		# by hand, so the address is read out loud, with the place to change it,
		# and nothing is asked twice.
		bind="$(config_value bind)"
		bind="${bind:-127.0.0.1:8080}"
		port="${bind##*:}"

		say "Keeping the settings that are here, rather than asking for them again:"
		say "  Web interface  $bind"
		say "${DIM}It stands in $CONFIG_DIR/config.toml — change it there and restart craftpanel.service.${OFF}"
		if [ -n "${CRAFTPANEL_PORT:-}" ]; then
			warn "CRAFTPANEL_PORT does nothing while that file is there"
		fi
		if [ -n "$(config_value start ports)$(config_value end ports)" ]; then
			warn "the [ports] table in $CONFIG_DIR/config.toml is read by nobody. An older installer wrote it and the panel never looked at it: the range below is the one the servers really get their ports from, and it is a setting of the panel. The table can go."
		fi
	else
		port="$(ask 'Web interface port' '8080' CRAFTPANEL_PORT)"
		bind="0.0.0.0:$port"
	fi

	# The panel keeps this one itself, in panel.db. Over a database that is
	# already here it is therefore read and not asked: what the operator set in
	# the panel is his, and an installer that overwrote it would move the range
	# out from under servers that hold addresses in the old one. Where the
	# database is new, the answer is applied below with `admin ports` — the same
	# way the panel writes it, checks and all.
	if [ -s "$DATA_DIR/panel.db" ]; then
		read_port_pool
		pool_start="$POOL_FROM"
		pool_end="$POOL_TO"
		if [ -n "$pool_start" ]; then
			say "  Server ports   $pool_start to $pool_end${DIM}, as the panel has them set${OFF}"
		fi
		if [ -n "${CRAFTPANEL_POOL_START:-}${CRAFTPANEL_POOL_END:-}" ]; then
			warn "CRAFTPANEL_POOL_START and CRAFTPANEL_POOL_END do nothing over a database that is already here — that range is set in the panel, under Administration → Settings"
		fi
	else
		pool_start="$(ask 'First port for Minecraft servers' '25565' CRAFTPANEL_POOL_START)"
		pool_end="$(ask 'Last port for Minecraft servers' '25700' CRAFTPANEL_POOL_END)"
		sensible_port_range "$pool_start" "$pool_end"
		set_pool=yes
	fi

	# Asked only where one is going to be made. On a database that was kept the
	# name is taken already, `admin create` refuses it (auth/cli.rs, install), and
	# the whole install used to break off there — over data that was perfectly
	# fine. Whoever wants a second administrator makes one in the panel, or with
	# that same command afterwards.
	local admin=""
	if [ "$FOUND_ADMINS" = "0" ]; then
		admin="$(ask 'Administrator username' 'admin' CRAFTPANEL_ADMIN)"
	fi
	say

	# The download before the accounts, and not the other way round: a release
	# that was never published for this architecture, or a machine that cannot
	# reach GitHub, is far and away the likeliest way an installation ends early.
	# In this order it ends before an account, a group or a directory has been
	# made, and there is nothing to take back.
	fetch_binaries "$version"
	ensure_accounts
	[ -n "$keeping_config" ] || write_config "$port"
	write_units

	if [ -n "$set_pool" ]; then
		step "setting $pool_start to $pool_end aside for Minecraft servers"
		CRAFTPANEL_CONFIG=$CONFIG_DIR/config.toml \
			runuser -u "$SERVICE_USER" -- "$PREFIX/craftpanel" admin ports \
			--from "$pool_start" --to "$pool_end" ||
			die "could not set the port range. The panel would have handed its servers ports out of 25565 to 25700, which is not what you asked for."
	fi

	local password=""
	if [ -n "$admin" ]; then
		step "creating the administrator account"
		password="$(CRAFTPANEL_CONFIG=$CONFIG_DIR/config.toml \
			runuser -u "$SERVICE_USER" -- "$PREFIX/craftpanel" admin create --username "$admin" --print-password)" ||
			die "could not create the administrator account"

		# Out the moment it exists, and not only in the summary at the end. The
		# password is shown once and lives nowhere else — the panel keeps a hash
		# of it — so everything between here and the end of the run is a way to
		# lose an account that already exists. It was: one line that asked this
		# machine for its address ended the run whenever `hostname -I` was not
		# there, after the account had been made and before it was named.
		say
		say "${BOLD}The administrator account has been created.${OFF}"
		say "  Username  $admin"
		say "  Password  ${BOLD}$password${OFF}"
		say "${DIM}Write it down now. The panel still has to start, and if that goes wrong"
		say "this is the only place the password was ever printed.${OFF}"
	elif [ -z "$FOUND_ADMINS" ]; then
		say "No administrator is made: what is in panel.db could not be read."
	else
		say "No administrator is made: the database has $(plural "$FOUND_ADMINS" administrator) already."
		[ -n "${CRAFTPANEL_ADMIN:-}" ] &&
			warn "CRAFTPANEL_ADMIN does nothing here — one is only made where there is none"
	fi

	# The installation is on the disk in full and the password has been printed.
	# What can still go wrong is a service that will not come up, and that is
	# read in the journal — with the units and the data still in place — rather
	# than swept away.
	installation_stands

	# `|| true` on both: systemctl says nothing here that is not asked again two
	# lines down, and an enable that comes back non-zero must not end the run
	# behind the password that has just been shown.
	systemctl enable --now craftpanel-helper.service >/dev/null 2>&1 || true
	systemctl enable --now craftpanel.service >/dev/null 2>&1 || true

	sleep 1
	if ! systemctl is-active --quiet craftpanel.service; then
		die "CraftPanel did not start. See: journalctl -u craftpanel -n 50"
	fi
	version_stamp "$version"

	local shown
	shown="$(machine_address)"
	case "$bind" in 127.*|localhost:*|"[::1]"*) shown="127.0.0.1" ;; esac

	say
	if [ -n "$kept" ]; then
		say "${GREEN}${BOLD}CraftPanel is running, on the data that was already here.${OFF}"
	else
		say "${GREEN}${BOLD}CraftPanel is running.${OFF}"
	fi
	say
	say "  Address   http://$shown:$port"
	if [ -n "$pool_start" ]; then
		say "  Ports     $pool_start to $pool_end for Minecraft servers"
	fi
	if [ -n "$kept" ]; then
		local taken=""
		[ -n "$FOUND_SERVERS" ] && taken="$(plural "$FOUND_SERVERS" server)"
		[ -n "$FOUND_USERS" ] && taken="${taken:+$taken and }$(plural "$FOUND_USERS" account)"
		[ -n "$taken" ] && say "  Taken up  $taken, with the worlds and the passwords they have"
	fi
	if [ -n "$password" ]; then
		say "  Username  $admin"
		say "  Password  ${BOLD}$password${OFF}"
	elif [ -n "$FOUND_ADMIN_NAMES" ]; then
		say "  Sign in   $FOUND_ADMIN_NAMES"
	fi
	say

	if [ -n "$password" ]; then
		say "${DIM}Write the password down; it is not shown again."
	else
		say "${DIM}The usernames and passwords are the ones you had. For a forgotten one:"
		say "  CRAFTPANEL_CONFIG=$CONFIG_DIR/config.toml runuser -u $SERVICE_USER -- $PREFIX/craftpanel admin passwd --username NAME --print-password"
		say "and to let somebody choose his own, a link to send him:"
		say "  CRAFTPANEL_CONFIG=$CONFIG_DIR/config.toml runuser -u $SERVICE_USER -- $PREFIX/craftpanel admin reset-link --username NAME --base-url http://$shown:$port"
	fi
	say "Run this installer again to update or remove CraftPanel.${OFF}"
	say
}

do_update() {
	local current="$1" latest="$2"

	if [ "$current" = "$latest" ]; then
		say "CraftPanel $current is already the newest version."
		confirm "Reinstall it anyway?" || return 0
	fi

	refuse_to_go_backwards "$latest"

	step "updating $current → $latest"
	# Not only the binaries: an update is also where an installation from an
	# older layout is put right. `ensure_accounts` makes nothing that is already
	# there — it sets the owner and the mode, which is what moved.
	ensure_accounts
	fetch_binaries "$latest"
	write_units
	systemctl restart craftpanel-helper.service
	systemctl restart craftpanel.service

	sleep 1
	if systemctl is-active --quiet craftpanel.service; then
		version_stamp "$latest"
		say "${GREEN}Updated to $latest.${OFF} Running Minecraft servers were not interrupted."
	else
		die "CraftPanel did not come back up. See: journalctl -u craftpanel -n 50"
	fi
}

# ------------------------------------------------------------ taking it away
#
# Three things stand between "remove it" and a machine that is really rid of it:
# the servers that are still running, an honest count of what the data directory
# holds, and the accounts. The order below is what makes the first one work —
# the games go while the panel is still up, so every supervisor can hand over
# its last console lines and end by itself.

# The accounts this panel made, and only those. Two things have to agree: the
# name carries the prefix the helper builds from the panel id
# (craftpanel-proto, system_username) and the home directory lies in this
# panel's users directory (craftpanel-helper users.rs, create). The prefix on
# its own would not do — a userdel that hits somebody else's account is worse
# than anything it could have cleaned up.
panel_accounts() {
	# `|| true` because a getent that comes back empty-handed leaves with 2, and
	# under `set -o pipefail` counting no accounts would end the run instead of
	# printing a nought.
	{ getent passwd || true; } | awk -F: -v prefix="$ACCOUNT_PREFIX" -v home="$DATA_DIR/users/" \
		'index($1, prefix) == 1 && index($6, home) == 1 { print $1 }'
}

# The prefix without the home directory: not ours to remove, and not ours to
# pass over in silence either.
foreign_accounts() {
	{ getent passwd || true; } | awk -F: -v prefix="$ACCOUNT_PREFIX" -v home="$DATA_DIR/users/" \
		'index($1, prefix) == 1 && index($6, home) != 1 { print $1 " (home " $6 ")" }'
}

account_busy() {
	local name="$1"
	if command -v pgrep >/dev/null; then
		pgrep -u "$name" >/dev/null 2>&1
		return
	fi
	[ -n "$(ps -u "$name" -o pid= 2>/dev/null)" ]
}

# The `|| true` is not decoration: a starting point that is not there makes find
# leave with 1, `set -o pipefail` hands that on, and counting nothing would end
# the run instead of printing a nought.
count_of() {
	find "$@" 2>/dev/null | wc -l || true
}

# du is the only honest answer to "how much is this": no total is kept anywhere,
# and a world grows between two of its own backups.
size_of() {
	local path="$1"
	[ -e "$path" ] || return 0
	du -sh "$path" 2>/dev/null | awk '{print $1}' || true
}

dropin_files() {
	count_of /etc/systemd/system/craftpanel.service.d \
		/etc/systemd/system/craftpanel-helper.service.d -type f -name '*.conf'
}

# Measured, never claimed. "Your Minecraft servers and the database" names two
# of the eight things in there, and the other six are the ones nobody thinks of
# until they are gone.
DATA_SERVERS=0
DATA_HOMES=0
DATA_ARCHIVES=0
DATA_KEYS=0
DATA_DRIVE=0
DATA_RUNTIMES=0
DATA_ACCOUNTS=0
DATA_DROPINS=0

measure_data() {
	DATA_SERVERS="$(count_of "$DATA_DIR/users" -mindepth 3 -maxdepth 3 -type d -path '*/servers/*')"
	DATA_HOMES="$(count_of "$DATA_DIR/users" -mindepth 1 -maxdepth 1 -type d)"
	DATA_ARCHIVES="$(count_of "$DATA_DIR/backups" -type f -name '*.tar.zst')"

	# The keys are deliberately not in panel.db: migrations 0008 and 0012 keep
	# them in files of their own, so a copy of the database — for a bug report,
	# for a backup — carries no way in to anybody's playit or Google account.
	# Which is why they are counted here, on the disk, and not with a query.
	DATA_KEYS="$(count_of "$DATA_DIR/playit" -mindepth 2 -maxdepth 2 -type f -name secret)"
	if [ -f "$DATA_DIR/playit/secret" ]; then
		DATA_KEYS=$((DATA_KEYS + 1))
	fi

	DATA_DRIVE="$(count_of "$DATA_DIR/drive" -mindepth 2 -maxdepth 2 -type f -name refresh_token)"
	DATA_RUNTIMES="$(count_of "$DATA_DIR/runtimes" -mindepth 1 -maxdepth 1 -type d -name 'java-*')"
	DATA_ACCOUNTS="$(panel_accounts | wc -l)" || DATA_ACCOUNTS=0
	DATA_DROPINS="$(dropin_files)"
}

nothing_left() {
	[ ! -e "$DATA_DIR" ] && [ ! -e "$CONFIG_DIR" ] && [ "$DATA_ACCOUNTS" -eq 0 ] &&
		! getent passwd "$SERVICE_USER" >/dev/null
}

# What show_data and keep_data say about the accounts is what remove_accounts
# does, and no longer a line of its own: remove_accounts takes the service
# account whenever it is there, whether or not a single craft-… account stands
# beside it, so both of these name it on the same terms. Counting the two
# together read "0 system accounts craft… and craftpanel" on a machine that had
# only the service account — a sentence that was wrong twice over.
accounts_here() {
	local both=""

	if [ "$DATA_ACCOUNTS" -gt 0 ]; then
		both="$(plural "$DATA_ACCOUNTS" "managed account") $ACCOUNT_PREFIX…, each with a group of its own"
	fi
	if getent passwd "$SERVICE_USER" >/dev/null; then
		both="${both:+$both, and }the service account $SERVICE_USER with its group"
	fi
	printf '%s' "$both"
}

show_data() {
	local size accounts

	say "${BOLD}What is left, counted just now:${OFF}"
	say

	if [ "$DATA_HOMES" -gt 0 ]; then
		say "  Worlds        $(plural "$DATA_SERVERS" server) in $(plural "$DATA_HOMES" "user home") under $DATA_DIR/users"
	fi
	if [ "$DATA_ARCHIVES" -gt 0 ]; then
		size="$(size_of "$DATA_DIR/backups")"
		say "  Backups       $(plural "$DATA_ARCHIVES" archive), $size in $DATA_DIR/backups"
	fi
	if [ -f "$DATA_DIR/panel.db" ]; then
		say "  Database      panel.db — every panel user with his password, the settings, the backup list"
	fi
	if [ "$DATA_KEYS" -gt 0 ]; then
		say "  playit        $(plural "$DATA_KEYS" key) — without its key a server's public address is gone and has to be claimed again"
	fi
	if [ "$DATA_DRIVE" -gt 0 ]; then
		say "  Drive         $(plural "$DATA_DRIVE" "connected Google account") — allowing this panel in again is a trip through the browser"
	fi
	if [ "$DATA_RUNTIMES" -gt 0 ]; then
		size="$(size_of "$DATA_DIR/runtimes")"
		say "  Java          $(plural "$DATA_RUNTIMES" "fetched runtime"), $size in $DATA_DIR/runtimes"
	fi
	accounts="$(accounts_here)"
	if [ -n "$accounts" ]; then
		say "  Accounts      $accounts"
	fi
	if [ -f "$CONFIG_DIR/config.toml" ]; then
		say "  Config        $CONFIG_DIR/config.toml"
	fi
	if [ "$DATA_DROPINS" -gt 0 ]; then
		say "  Drop-ins      $(plural "$DATA_DROPINS" file) of yours under /etc/systemd/system/craftpanel*.service.d"
	fi
	if [ -d "$DATA_DIR" ]; then
		size="$(size_of "$DATA_DIR")"
		say "  Altogether    $size in $DATA_DIR"
	fi
}

# userdel and nothing more clever: no -r, because the home directories go with
# the data directory two lines later, and -r on an account whose home has been
# replaced by a link would follow it. An account with a process still in it
# stays and gets named — a name taken away while its processes run leaves a uid
# nobody can look up.
ACCOUNTS_LEFT=()

# Whether the control group went with the rest. A tree that still holds a
# process cannot be removed, and "Everything removed." must not say it was.
CGROUP_GONE=1

remove_accounts() {
	local accounts=() name
	ACCOUNTS_LEFT=()
	mapfile -t accounts < <(panel_accounts)

	if [ "${#accounts[@]}" -gt 0 ]; then
		step "removing $(plural "${#accounts[@]}" "managed account") $ACCOUNT_PREFIX…"
	fi

	for name in "${accounts[@]}"; do
		if account_busy "$name" || ! userdel "$name" >/dev/null 2>&1; then
			ACCOUNTS_LEFT+=("$name")
			continue
		fi
		# useradd made a group of the same name beside the account and userdel
		# takes it with it, unless somebody else was put in it in the meantime.
		if getent group "$name" >/dev/null; then
			groupdel "$name" >/dev/null 2>&1 || true
		fi
	done

	if getent passwd "$SERVICE_USER" >/dev/null; then
		step "removing the service account $SERVICE_USER"
		if account_busy "$SERVICE_USER" || ! userdel "$SERVICE_USER" >/dev/null 2>&1; then
			ACCOUNTS_LEFT+=("$SERVICE_USER")
		fi
	fi

	if getent group "$SERVICE_GROUP" >/dev/null; then
		groupdel "$SERVICE_GROUP" >/dev/null 2>&1 ||
			warn "the group $SERVICE_GROUP stays, somebody is still in it: getent group $SERVICE_GROUP"
	fi
}

remove_data() {
	local strays=() name

	remove_accounts
	step "deleting $DATA_DIR and $CONFIG_DIR"
	rm -rf "$DATA_DIR" "$CONFIG_DIR"
	rm -rf /etc/systemd/system/craftpanel.service.d /etc/systemd/system/craftpanel-helper.service.d
	systemctl daemon-reload

	say
	if [ "${#ACCOUNTS_LEFT[@]}" -eq 0 ] && [ "$CGROUP_GONE" -eq 1 ]; then
		say "${GREEN}${BOLD}Everything removed.${OFF}"
		say "${DIM}The data, the configuration, both units and their drop-ins, $RUN_DIR, the"
		say "control group, every $ACCOUNT_PREFIX… account with its group, and $SERVICE_USER.${OFF}"
	else
		say "${BOLD}Removed:${OFF} the data, the configuration, both units with their drop-ins"
		say "and $RUN_DIR."
		say
		warn "not removed:"
		if [ "$CGROUP_GONE" -ne 1 ]; then
			say "    $CGROUP_ROOT  ${DIM}— something is still in it${OFF}"
		fi
		for name in "${ACCOUNTS_LEFT[@]}"; do
			say "    $name  ${DIM}— something of it is still running${OFF}"
		done
		if [ "${#ACCOUNTS_LEFT[@]}" -gt 0 ]; then
			say "${DIM}Their home directories are deleted and the accounts are not, and an account"
			say "pointing at nothing is not removed. End what runs under them and finish it:"
			say "  pkill -u <name> && userdel <name>${OFF}"
		fi
	fi

	mapfile -t strays < <(foreign_accounts)
	if [ "${#strays[@]}" -gt 0 ]; then
		say
		warn "these carry the $ACCOUNT_PREFIX prefix but live outside $DATA_DIR/users, so they were left alone:"
		for name in "${strays[@]}"; do
			say "    $name"
		done
	fi

	if [ "$DATA_DRIVE" -gt 0 ]; then
		say
		say "${DIM}Backups that went to Google Drive lie in their owners' own Drive folders and"
		say "stay there. This machine has no say over them any more — whoever wants them"
		say "gone deletes the folder in his Drive.${OFF}"
	fi
}

keep_data() {
	local kept="" accounts

	if [ "$DATA_SERVERS" -gt 0 ]; then
		kept="$(plural "$DATA_SERVERS" "server with its world" "servers with their worlds")"
	fi
	if [ "$DATA_ARCHIVES" -gt 0 ]; then
		kept="${kept:+$kept, }$(plural "$DATA_ARCHIVES" backup)"
	fi
	if [ -f "$DATA_DIR/panel.db" ]; then
		kept="${kept:+$kept, }panel.db with every user and his password"
	fi
	if [ "$DATA_KEYS" -gt 0 ]; then
		kept="${kept:+$kept, }$(plural "$DATA_KEYS" "playit key")"
	fi
	if [ "$DATA_DRIVE" -gt 0 ]; then
		kept="${kept:+$kept, }$(plural "$DATA_DRIVE" "connected Drive account")"
	fi
	if [ "$DATA_RUNTIMES" -gt 0 ]; then
		kept="${kept:+$kept, }$(plural "$DATA_RUNTIMES" "Java runtime")"
	fi

	say
	say "${BOLD}Kept, exactly as counted above:${OFF}"
	say "  $DATA_DIR"
	if [ -n "$kept" ]; then
		say "  ${DIM}$kept${OFF}"
	fi
	if [ -f "$CONFIG_DIR/config.toml" ]; then
		say "  $CONFIG_DIR/config.toml"
		say "  ${DIM}the address the panel listens on and where its data lies${OFF}"
	fi
	accounts="$(accounts_here)"
	if [ -n "$accounts" ]; then
		say "  $accounts"
		say "  ${DIM}with the uids they have now, and their home directories with them${OFF}"
	fi
	if [ "$DATA_DROPINS" -gt 0 ]; then
		say "  $(plural "$DATA_DROPINS" "drop-in file") under /etc/systemd/system/craftpanel*.service.d"
	fi
	say
	if [ "$CGROUP_GONE" -eq 1 ]; then
		say "${BOLD}Gone:${OFF} both programs, both units, $RUN_DIR and the control group."
	else
		say "${BOLD}Gone:${OFF} both programs, both units and $RUN_DIR."
	fi
	say
	say "${DIM}Installing CraftPanel again puts them back and takes the rest up as it"
	say "stands: the same worlds in the same directories, and every account with the uid"
	say "it has now, so not one file changes owner. You log in with the accounts that are"
	say "in panel.db already, the administrator among them, with the passwords they have"
	say "now. $CONFIG_DIR/config.toml is read as it lies.${OFF}"
}

# The games first and the services after: a supervisor whose panel is already
# gone cannot hand its last console lines over, and would have to be shot down
# instead of ending by itself.
uninstall_games() {
	local pid

	if stop_games "$CGROUP_ROOT"; then
		return 0
	fi

	say
	warn "these have not stopped within ${STOP_GRACE}s:"
	for pid in "${STUCK_GAMES[@]}"; do
		say "    pid $pid  $(game_label "$pid")"
	done
	say
	say "${DIM}A kill lands wherever the server happens to be. Caught in the middle of a save"
	say "it loses everything since the last one, and can leave the world damaged. Left"
	say "alone the worlds are safe — but the panel that could stop them is about to be"
	say "gone, and afterwards nothing but a kill by hand will get them down.${OFF}"

	if confirm "Kill them"; then
		step "killing $(plural "${#STUCK_GAMES[@]}" "game process" "game processes")"
		for pid in "${STUCK_GAMES[@]}"; do
			kill -KILL -- "-$pid" 2>/dev/null || kill -KILL "$pid" 2>/dev/null || true
		done
		sleep 2
		mapfile -t STUCK_GAMES < <(game_processes "$CGROUP_ROOT")
	fi

	if [ "${#STUCK_GAMES[@]}" -gt 0 ]; then
		warn "still running, and nothing here will stop them now: ${STUCK_GAMES[*]}"
		say "${DIM}Deleting the data below takes their files out from under them, and they run"
		say "on until somebody ends them: kill -TERM -- -<pid>${OFF}"
	fi
}

do_uninstall() {
	say
	warn "This removes CraftPanel from this machine."
	say "${DIM}Running Minecraft servers are asked to save and stop first. What lies on the"
	say "disk is counted and shown to you before a single question about deleting it.${OFF}"
	confirm "Continue?" || return 0

	uninstall_games

	step "stopping services"
	systemctl disable --now craftpanel.service >/dev/null 2>&1 || true
	systemctl disable --now craftpanel-helper.service >/dev/null 2>&1 || true
	rm -f /etc/systemd/system/craftpanel.service /etc/systemd/system/craftpanel-helper.service
	systemctl daemon-reload

	if [ "${#STUCK_GAMES[@]}" -eq 0 ]; then
		take_down_cgroup "$CGROUP_ROOT" || {
			CGROUP_GONE=0
			warn "$CGROUP_ROOT stays, something is still in it: cat $CGROUP_ROOT/*/cgroup.procs"
		}
	else
		CGROUP_GONE=0
		warn "$CGROUP_ROOT stays for as long as those game processes run"
	fi
	rm -rf "$RUN_DIR"

	step "removing programs"
	rm -f "$PREFIX/craftpanel" "$PREFIX/craftpanel-helper"

	step "counting what is in $DATA_DIR"
	measure_data
	say

	if nothing_left; then
		say "Nothing else of CraftPanel is on this machine."
		say
		return 0
	fi

	show_data
	say
	if confirm "Delete all of it? This cannot be undone"; then
		remove_data
	else
		keep_data
	fi
	say
}

# The two programs are gone and the data is not — which is exactly what an
# uninstall that was told to keep the data leaves behind. installed_version()
# cannot see such a machine: it asks $PREFIX/craftpanel for its version, and
# that is the program the uninstall has just deleted. So the data answers
# instead (kept_data), and both ways out of this state stay open. Only one of
# them used to: whoever had said "keep" and then thought better of it was given
# an installation he had not asked for, with no way back to the uninstall.
offer_kept_data() {
	local latest="$1"

	say
	say "${BOLD}CraftPanel is not installed on this machine, and its data is still here.${OFF}"
	say "${DIM}The programs, the units and the control group are gone. What stands is what an"
	say "uninstall was told to keep — install again to take it up, or remove it now.${OFF}"
	say
	measure_data
	show_data
	say
	say "  1) Install CraftPanel $latest again and take all of it up"
	say "  2) Remove what is left — the data, the accounts, everything"
	say "  3) Nothing, quit"
	say
	case "$(ask 'What would you like to do?' '1' CRAFTPANEL_ACTION)" in
		1) do_install "$latest" ;;
		2) do_uninstall ;;
		*) say "Nothing changed." ;;
	esac
}

main() {
	require_root

	# Before anything looks in $DATA_DIR: the panel reads data_dir out of
	# config.toml, so a configuration that names another directory is the one that
	# counts — for the units written here, for the data that is taken up again,
	# and for what an uninstall would delete.
	local configured_data_dir current latest
	configured_data_dir="$(config_value data_dir)"
	if [ -n "$configured_data_dir" ] && [ "$configured_data_dir" != "$DATA_DIR" ]; then
		warn "$CONFIG_DIR/config.toml says data_dir = $configured_data_dir, so that is the directory this installation uses"
		DATA_DIR="$configured_data_dir"
	fi

	preflight

	current="$(installed_version)"
	if [ -n "${CRAFTPANEL_BUNDLE:-}" ]; then
		latest="${CRAFTPANEL_VERSION:-local}"
	else
		# latest_version says itself what went wrong, in the words of the thing that
		# actually went wrong, and ends the run there.
		latest="${CRAFTPANEL_VERSION:-$(latest_version)}"
	fi

	# Before install and before update, because an installation under the old name
	# is neither. Saying no to the move leads on to the ordinary paths below, which
	# is how a second, fresh installation beside the old one stays possible — and
	# why no is the default. $current tells wants_upgrade whether that second
	# installation is already standing here.
	if old_installation && wants_upgrade "$current"; then
		do_upgrade "$latest"
		return
	fi

	if [ -z "$current" ]; then
		if kept_data; then
			offer_kept_data "$latest"
		else
			do_install "$latest"
		fi
		return
	fi

	say
	say "${BOLD}CraftPanel $current is installed.${OFF}  Newest available: $latest"
	say
	say "  1) Update"
	say "  2) Uninstall"
	say "  3) Nothing, quit"
	say
	case "$(ask 'What would you like to do?' '1' CRAFTPANEL_ACTION)" in
		1) do_update "$current" "$latest" ;;
		2) do_uninstall ;;
		*) say "Nothing changed." ;;
	esac
}

main "$@"
